use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcbl_plugin_api::native::abi::{self, EventWake};
use paperconnect_protocol::{Command, Event, Peer, Preferences, Room, SequencedEvent, Snapshot};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::online::{self, EasyTierStartOptions, EasyTierStartRequest, easytier, paperconnect};

const COMMANDS: usize = 32;
const EVENTS: usize = 128;
const EVENTS_PER_BATCH: usize = 16;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

struct Request {
    id: u64,
    command: Vec<u8>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct PendingEvents {
    sequence: u64,
    gap: bool,
    events: VecDeque<SequencedEvent>,
}

pub struct Events {
    pending: Mutex<PendingEvents>,
    context: usize,
    wake: Option<unsafe extern "C" fn(*mut c_void)>,
}

impl Events {
    fn new(wake: EventWake) -> Self {
        Self {
            pending: Mutex::default(),
            context: wake.context as usize,
            wake: wake.notify,
        }
    }

    fn publish(&self, event: Event) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        if pending.events.len() >= EVENTS {
            pending.events.pop_front();
            pending.gap = true;
        }
        pending.sequence += 1;
        let sequence = pending.sequence;
        pending.events.push_back(SequencedEvent { sequence, event });
        drop(pending);
        if let Some(wake) = self.wake {
            // SAFETY: the host leases the callback until destroy has joined the worker.
            unsafe { wake(self.context as *mut c_void) };
        }
    }

    pub fn read_batch(&self, capacity: usize) -> Result<Vec<u8>, usize> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.events.is_empty() && !pending.gap {
            return Ok(Vec::new());
        }

        #[derive(serde::Serialize)]
        struct View<'a> {
            gap: bool,
            events: Vec<&'a SequencedEvent>,
        }

        let mut count = pending.events.len().min(EVENTS_PER_BATCH);
        loop {
            let output = serde_json::to_vec(&View {
                gap: pending.gap,
                events: pending.events.iter().take(count).collect(),
            })
            .expect("concrete PaperConnect events are JSON serializable");
            if output.len() > abi::MAX_EVENT_BATCH && count > 1 {
                count /= 2;
                continue;
            }
            if output.len() > capacity {
                return Err(output.len());
            }
            pending.events.drain(..count);
            pending.gap = false;
            return Ok(output);
        }
    }
}

pub struct Session {
    inbox: mpsc::Sender<Request>,
    pending: Arc<Mutex<BTreeMap<u64, CancellationToken>>>,
    stop: CancellationToken,
    worker: Option<std::thread::JoinHandle<()>>,
    pub events: Arc<Events>,
}

impl Session {
    pub fn start(wake: EventWake) -> std::io::Result<Self> {
        let (inbox, receiver) = mpsc::channel(COMMANDS);
        let stop = CancellationToken::new();
        let events = Arc::new(Events::new(wake));
        let pending = Arc::new(Mutex::new(BTreeMap::new()));
        let worker = std::thread::Builder::new()
            .name("paperconnect-native".into())
            .spawn({
                let stop = stop.clone();
                let events = events.clone();
                let pending = pending.clone();
                move || match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(run(receiver, pending, &events, stop)),
                    Err(error) => events.publish(Event::Failed {
                        request: 0,
                        message: error.to_string(),
                    }),
                }
            })?;
        Ok(Self {
            inbox,
            pending,
            stop,
            worker: Some(worker),
            events,
        })
    }

    pub fn submit(&self, id: u64, command: &[u8]) -> i32 {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.contains_key(&id) {
            return abi::INVALID_ARGUMENT;
        }
        let token = self.stop.child_token();
        pending.insert(id, token.clone());
        match self.inbox.try_send(Request {
            id,
            command: command.to_owned(),
            cancel: token,
        }) {
            Ok(()) => abi::OK,
            Err(error) => {
                pending.remove(&id);
                match error {
                    mpsc::error::TrySendError::Full(_) => abi::BUSY,
                    mpsc::error::TrySendError::Closed(_) => abi::CLOSED,
                }
            }
        }
    }

    pub fn cancel(&self, id: u64) -> i32 {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(cancel) = pending.get(&id) else {
            return abi::INVALID_HANDLE;
        };
        cancel.cancel();
        abi::OK
    }

    pub fn stop(mut self) -> i32 {
        self.stop.cancel();
        // Joining also drops the native Tokio runtime and all its background tasks.
        self.worker
            .take()
            .map_or(abi::OK, |worker| match worker.join() {
                Ok(()) => abi::OK,
                Err(_) => abi::INTERNAL,
            })
    }
}

async fn run(
    mut inbox: mpsc::Receiver<Request>,
    pending: Arc<Mutex<BTreeMap<u64, CancellationToken>>>,
    events: &Events,
    stop: CancellationToken,
) {
    let mut state = Snapshot::default();
    let mut refresh = tokio::time::interval(Duration::from_secs(2));
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            request = inbox.recv() => {
                let Some(request) = request else { break };
                run_request(&request, &mut state, events).await;
                if let Ok(mut map) = pending.lock() {
                    map.remove(&request.id);
                }
            }
            _ = refresh.tick(), if state.room.is_some() => refresh_status(&mut state, events).await,
        }
    }
    if let Err(error) = easytier::stop().await {
        tracing::error!(%error, "PaperConnect session cleanup failed");
    }
}

async fn run_request(request: &Request, state: &mut Snapshot, events: &Events) {
    let decoded = serde_json::from_slice::<Command>(&request.command);
    let changes_room = matches!(&decoded, Ok(Command::Host { .. } | Command::Join { .. }));
    let result = match decoded {
        Ok(command) => {
            tokio::select! {
                biased;
                _ = request.cancel.cancelled() => {
                    if changes_room && state.room.is_none() { let _ = easytier::stop().await; }
                    events.publish(Event::Cancelled { request: request.id });
                    return;
                }
                result = tokio::time::timeout(COMMAND_TIMEOUT, apply(command, state, events)) => {
                    match result {
                        Ok(result) => result,
                        Err(_) => {
                            if changes_room && state.room.is_none() { let _ = easytier::stop().await; }
                            Err("联机命令超时".into())
                        }
                    }
                }
            }
        }
        Err(error) => Err(error.to_string()),
    };
    match result {
        Ok(()) => events.publish(Event::Completed {
            request: request.id,
        }),
        Err(error) => events.publish(Event::Failed {
            request: request.id,
            message: error.chars().take(2048).collect(),
        }),
    }
}

async fn apply(command: Command, state: &mut Snapshot, events: &Events) -> Result<(), String> {
    let is_host = matches!(&command, Command::Host { .. });
    match command {
        Command::Snapshot => events.publish(Event::Snapshot {
            snapshot: state.clone(),
        }),
        Command::DetectPorts => {
            let ports = tokio::task::spawn_blocking(crate::process::udp_ports)
                .await
                .map_err(|error| error.to_string())??;
            events.publish(Event::Ports { ports });
        }
        Command::CheckNat => {
            state.nat = Some(check_nat().await);
            events.publish(Event::Snapshot {
                snapshot: state.clone(),
            });
        }
        Command::Host { code, preferences } | Command::Join { code, preferences } => {
            if state.room.is_some() {
                return Err("请先退出当前房间".into());
            }
            let room = match start_room(&code, preferences, is_host).await {
                Ok(room) => room,
                Err(error) => {
                    let _ = easytier::stop().await;
                    return Err(error);
                }
            };
            state.room = Some(room);
            events.publish(Event::RoomChanged {
                room: state.room.clone(),
            });
            refresh_status(state, events).await;
        }
        Command::RetryTransport => {
            let room = state
                .room
                .as_mut()
                .filter(|room| !room.is_host)
                .ok_or("当前没有来宾联机会话")?;
            room.discovery_port_occupied = online::paperconnect_guest::retry_transport().await?
                == online::PaperConnectClientState::DiscoveryPortOccupied;
            events.publish(Event::RoomChanged {
                room: state.room.clone(),
            });
        }
        Command::Stop => {
            easytier::stop().await?;
            *state = Snapshot::default();
            events.publish(Event::Snapshot {
                snapshot: state.clone(),
            });
        }
    }
    Ok(())
}

async fn check_nat() -> paperconnect_protocol::Nat {
    use ::easytier::common::stun::{StunInfoCollector, StunInfoCollectorTrait};

    let collector = StunInfoCollector::new_with_default_servers();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    loop {
        let info = collector.get_stun_info();
        if info.udp_nat_type != 0
            || info.tcp_nat_type != 0
            || tokio::time::Instant::now() >= deadline
        {
            return paperconnect_protocol::Nat {
                udp: info.udp_nat_type,
                tcp: info.tcp_nat_type,
            };
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn start_room(code: &str, preferences: Preferences, is_host: bool) -> Result<Room, String> {
    let room = paperconnect::room_from_code(code)?;
    let player = preferences.player.trim();
    if player.len() > 96 || preferences.peers.len() > 16 {
        return Err("玩家名或节点设置超出限制".into());
    }
    let player = if player.is_empty() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        format!("Player-{}", &id[..8])
    } else {
        player.to_owned()
    };
    let hostname = if is_host {
        format!("paper-connect-server-{}", paperconnect::pick_listen_port()?)
    } else {
        format!("bmcbl-client-{player}")
    };
    easytier::start(EasyTierStartRequest {
        network_name: room.network_name,
        network_secret: room.network_secret,
        peers: preferences.peers,
        hostname: Some(hostname),
        player_name: player.clone(),
        game_port: preferences.game_port.unwrap_or(7551),
        options: Some(EasyTierStartOptions {
            disable_p2p: Some(preferences.disable_p2p),
            ipv4: None,
        }),
    })
    .await?;
    let discovery_port_occupied = if is_host {
        false
    } else {
        let server = paperconnect::probe_server().await?;
        online::paperconnect_guest::start(server, player).await?
            == online::PaperConnectClientState::DiscoveryPortOccupied
    };
    let status = easytier::status().await?;
    Ok(Room {
        code: room.room_code,
        is_host,
        game_port: status.as_ref().and_then(|status| status.game_port),
        virtual_ip: status.as_ref().and_then(|status| status.ipv4.clone()),
        hostname: status.as_ref().map(|status| status.hostname.clone()),
        game_host: status.and_then(|status| status.game_host),
        discovery_port_occupied,
    })
}

async fn refresh_status(state: &mut Snapshot, events: &Events) {
    let players = paperconnect::players()
        .into_iter()
        .map(|entry| entry.player)
        .take(128)
        .collect::<BTreeSet<_>>();
    let previous = state.players.iter().cloned().collect::<BTreeSet<_>>();
    if players != previous {
        events.publish(Event::PlayersChanged {
            joined: players.difference(&previous).cloned().collect(),
            left: previous.difference(&players).cloned().collect(),
        });
        state.players = players.into_iter().collect();
    }
    if let Ok(peers) = easytier::peers().await {
        let next = peers
            .into_iter()
            .take(128)
            .map(|peer| {
                let hostname: String = peer.hostname.chars().take(96).collect();
                let id = format!("{}|{hostname}", peer.ipv4.as_deref().unwrap_or_default());
                let view = Peer {
                    id: id.clone(),
                    hostname,
                    latency_ms: peer.latency_ms,
                    protocol: peer.protocol,
                    relay: peer.via_hostname,
                };
                (id, view)
            })
            .collect::<BTreeMap<_, _>>();
        let previous = state
            .peers
            .iter()
            .map(|peer| (&peer.id, peer))
            .collect::<BTreeMap<_, _>>();
        let present = next
            .values()
            .filter(|peer| previous.get(&peer.id).copied() != Some(*peer))
            .cloned()
            .collect::<Vec<_>>();
        let left = previous
            .keys()
            .filter(|id| !next.contains_key(**id))
            .map(|id| (*id).clone())
            .collect::<Vec<_>>();
        if !present.is_empty() || !left.is_empty() {
            state.peers = next.into_values().collect();
            events.publish(Event::PeersChanged { present, left });
        }
    }
    for abandoned in easytier::take_abandoned_connectors().into_iter().take(16) {
        events.publish(Event::NetworkError {
            message: format!("节点重试失败：{}", abandoned.url),
        });
    }
}
