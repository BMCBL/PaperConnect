use std::cell::RefCell;
use std::collections::VecDeque;

use bmcbl_plugin_api::r#async::Job;
use bmcbl_plugin_api::native::{self, InstanceHandle, ModuleHandle, RequestId};
use bmcbl_plugin_api::prelude::*;
use paperconnect_protocol::{Batch, Command, Event, Preferences, Snapshot};

mod page;
mod room;
mod updates;

use updates::Update;

struct PaperConnect;

#[derive(Default)]
struct PageState {
    config: Config,
    connection: Connection,
    pending: Option<Command>,
    submit: Option<Job<RequestId>>,
    read: Option<Job<Vec<u8>>>,
    request: Option<RequestId>,
    cancel_job: Option<Job<()>>,
    early_result: Option<(RequestId, String)>,
    update: Update,
    snapshot: Snapshot,
    ports: Option<Vec<u16>>,
    message: String,
    activity: VecDeque<String>,
    room_code: String,
    advanced: bool,
    port_input: Option<String>,
    sequence: u64,
}

#[derive(Default)]
enum Connection {
    #[default]
    Idle,
    Loading(Job<ModuleHandle>),
    Creating(ModuleHandle, Job<InstanceHandle>),
    Ready(InstanceHandle),
}

thread_local! {
    static PAGE: RefCell<PageState> = RefCell::default();
}

impl PageState {
    fn command_pending(&self) -> bool {
        self.pending.is_some() || self.submit.is_some() || self.request.is_some()
    }

    fn start(&mut self, command: Command) -> PluginResult<()> {
        if self.command_pending() {
            return Err(plugin_error("busy", "正在执行上一条联机命令。"));
        }
        let activity = match &command {
            Command::Host { .. } => "正在创建房间",
            Command::Join { .. } => "正在加入房间",
            Command::Stop => "正在断开联机",
            Command::RetryTransport => "正在重试本机游戏发现",
            Command::DetectPorts => "正在检测游戏 UDP 端口",
            Command::CheckNat => "正在检测 NAT 类型",
            Command::Snapshot => "正在刷新会话",
        };
        match self.connection {
            Connection::Ready(instance) => {
                self.submit = Some(send(instance, &command)?);
            }
            Connection::Idle => {
                let digest = self
                    .config
                    .module_digest()
                    .ok_or_else(|| plugin_error("native-component", page::NATIVE_SETUP))?;
                self.connection = Connection::Loading(native::load(digest)?);
                self.pending = Some(command);
            }
            Connection::Loading(_) | Connection::Creating(_, _) => {
                return Err(plugin_error("busy", "联机组件正在启动。"));
            }
        }
        self.message = String::new();
        self.record_activity(activity);
        Ok(())
    }

    fn complete(&mut self, event: &PluginEvent) -> PluginResult<bool> {
        let mut changed = self.advance_update(event)?;
        let connection = std::mem::take(&mut self.connection);
        self.connection = match connection {
            Connection::Loading(job) if job.is_ready(event) => {
                changed = true;
                let module = ready(job)?;
                Connection::Creating(module, native::create_instance(module, &[])?)
            }
            Connection::Creating(module, job) if job.is_ready(event) => {
                changed = true;
                let instance = match ready(job) {
                    Ok(instance) => instance,
                    Err(error) => {
                        let _ = native::unload(module);
                        return Err(error);
                    }
                };
                Connection::Ready(instance)
            }
            other => other,
        };
        if let Connection::Ready(instance) = self.connection
            && let Some(command) = self.pending.take()
        {
            self.submit = Some(send(instance, &command)?);
        }
        if self.submit.as_ref().is_some_and(|job| job.is_ready(event)) {
            self.request = Some(ready(self.submit.take().expect("matched submit job"))?);
            if let Some((id, message)) = self.early_result.take() {
                self.finish_command(id.0, message);
            }
            changed = true;
        }
        if self
            .cancel_job
            .as_ref()
            .is_some_and(|job| job.is_ready(event))
        {
            ready(self.cancel_job.take().expect("matched cancel job"))?;
            changed = true;
        }
        if self.read.as_ref().is_some_and(|job| job.is_ready(event)) {
            let bytes = ready(self.read.take().expect("matched read job"))?;
            if !bytes.is_empty() {
                self.apply_batch(&bytes)?;
                changed = true;
            }
        }
        if let Connection::Ready(instance) = self.connection
            && self.read.is_none()
            && (self.request.is_some() || self.snapshot.room.is_some())
        {
            self.read = Some(native::read_events(instance, 10_000)?);
        }
        Ok(changed)
    }

    fn apply_batch(&mut self, bytes: &[u8]) -> PluginResult<()> {
        let batch: Batch = serde_json::from_slice(bytes)
            .map_err(|error| plugin_error("native-events", error.to_string()))?;
        let mut gap = batch.gap;
        for item in batch.events {
            gap |= self.sequence != 0 && item.sequence != self.sequence + 1;
            self.sequence = item.sequence;
            match item.event {
                Event::Completed { request } => self.finish_command(request, String::new()),
                Event::Failed { request, message } => self.finish_command(request, message),
                Event::Cancelled { request } => self.finish_command(request, "操作已取消。".into()),
                Event::Ports { ports } => self.ports = Some(ports),
                Event::Snapshot { snapshot } => {
                    if self.snapshot.room.is_some() && snapshot.room.is_none() {
                        self.record_activity("已断开联机");
                    }
                    self.snapshot = snapshot;
                }
                Event::RoomChanged { room } => {
                    if let Some(room) = &room {
                        if self.snapshot.room.is_none() {
                            self.record_activity(if room.is_host {
                                "房间已创建"
                            } else {
                                "已加入房间"
                            });
                            if room.is_host
                                && let Err(error) =
                                    bmcbl_plugin_api::write_clipboard_text(&room.code)
                            {
                                self.message = error.message;
                            }
                        }
                    } else {
                        self.record_activity("已断开联机");
                    }
                    self.snapshot.room = room;
                }
                Event::PlayersChanged { joined, left } => {
                    self.snapshot
                        .players
                        .retain(|player| !left.contains(player));
                    self.snapshot.players.extend(joined);
                }
                Event::PeersChanged { present, left } => {
                    self.snapshot.peers.retain(|peer| !left.contains(&peer.id));
                    for peer in present {
                        if let Some(current) = self
                            .snapshot
                            .peers
                            .iter_mut()
                            .find(|item| item.id == peer.id)
                        {
                            *current = peer;
                        } else {
                            self.snapshot.peers.push(peer);
                        }
                    }
                }
                Event::NetworkError { message } => self.message = message,
            }
        }
        if gap && let Connection::Ready(instance) = self.connection {
            self.request = None;
            self.submit = Some(send(instance, &Command::Snapshot)?);
        }
        Ok(())
    }
}

impl PageState {
    fn record_activity(&mut self, message: impl Into<String>) {
        if self.activity.len() == 8 {
            self.activity.pop_front();
        }
        self.activity.push_back(message.into());
    }

    fn save_preferences(&mut self) -> PluginResult<()> {
        let raw = self.port_input.as_deref().unwrap_or_default().trim();
        let port = if raw.is_empty() || raw == "0" {
            0
        } else {
            raw.parse::<u16>()
                .ok()
                .filter(|port| *port >= 1025)
                .ok_or_else(|| plugin_error("game-port", "端口应为 1025–65535，留空时自动探测。"))?
        };
        let mut document = read_config()?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| plugin_error("config", error.to_string()))?;
        document["player_name"] = toml_edit::value(self.config.player_name.trim());
        document["game_port"] = toml_edit::value(i64::from(port));
        document["bootstrap_peers"] = toml_edit::value(self.config.bootstrap_peers.trim());
        document["disable_p2p"] = toml_edit::value(self.config.disable_p2p);
        write_config(document.to_string())?;
        self.config.game_port = port;
        self.record_activity("联机选项已保存");
        self.advanced = false;
        Ok(())
    }

    fn finish_command(&mut self, request: u64, message: String) {
        let id = RequestId(request);
        if request == 0 || self.request == Some(id) {
            self.request = None;
            if !message.is_empty() {
                self.record_activity(message.clone());
                self.message = message;
            }
        } else if self.submit.is_some() {
            self.early_result = Some((id, message));
        }
    }
}

fn ready<T: serde::de::DeserializeOwned>(job: Job<T>) -> PluginResult<T> {
    job.try_take()?
        .ok_or_else(|| plugin_error("native-job", "原生任务尚未完成。"))
}

fn send(instance: InstanceHandle, command: &Command) -> PluginResult<Job<RequestId>> {
    let bytes =
        serde_json::to_vec(command).map_err(|error| plugin_error("command", error.to_string()))?;
    native::submit_command(instance, &bytes)
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Config {
    native_version: Option<String>,
    native_sha256: Option<String>,
    player_name: String,
    bootstrap_peers: String,
    game_port: u16,
    disable_p2p: bool,
}

fn config() -> Config {
    read_config()
        .ok()
        .and_then(|value| toml::from_str(&value).ok())
        .unwrap_or_default()
}

impl Config {
    fn module_digest(&self) -> Option<&str> {
        self.native_sha256
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(option_env!("PAPERCONNECT_NATIVE_SHA256"))
    }

    fn preferences(&self) -> Preferences {
        Preferences {
            player: self.player_name.clone(),
            peers: self
                .bootstrap_peers
                .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
                .filter(|peer| !peer.is_empty())
                .map(str::to_owned)
                .collect(),
            game_port: (self.game_port >= 1025).then_some(self.game_port),
            disable_p2p: self.disable_p2p,
        }
    }
}

#[bmcbl_plugin]
impl Plugin for PaperConnect {
    fn init(_context: PluginContext) -> PluginResult<Vec<Registration>> {
        PAGE.with_borrow_mut(|state| state.config = config());
        Ok(registrations! {
            page "main", title = "创建或加入好友房间", tool = Tool::new("联机大厅").icon("users").order(100);
            subscribe "config-changed";
        })
    }

    fn handle_event(event: PluginEvent) -> PluginResult<()> {
        let changed = PAGE.with_borrow_mut(|state| {
            let result = if event
                .global_event()
                .is_some_and(|(name, _)| name == "config-changed")
            {
                state.config = config();
                state.port_input = None;
                Ok(true)
            } else if let PluginEventKind::Action(action) = &event.kind {
                match action.action_id.as_str() {
                    "detect" => state.start(Command::DetectPorts),
                    "check-nat" => state.start(Command::CheckNat),
                    "host" => match room::generate_room() {
                        Ok(code) => state.start(Command::Host {
                            code,
                            preferences: state.config.preferences(),
                        }),
                        Err(error) => Err(plugin_error("room-code", error.to_string())),
                    },
                    "room-code" => {
                        state.room_code = action.value.clone().unwrap_or_default();
                        return false;
                    }
                    "paste" => {
                        bmcbl_plugin_api::read_clipboard_text().and_then(|input| {
                            match input.filter(|text| !text.trim().is_empty()) {
                                Some(input) => {
                                    state.room_code = input.trim().replace(['\n', '\r'], " ");
                                    Ok(())
                                }
                                None => Err(plugin_error("room-code", "剪贴板没有联机码。")),
                            }
                        })
                    }
                    "join" | "room-code-submit" => {
                        if let Some(value) = &action.value {
                            state.room_code = value.clone();
                        }
                        match room::room_from_code(&state.room_code) {
                            Ok(code) => state.start(Command::Join {
                                code,
                                preferences: state.config.preferences(),
                            }),
                            Err(error) => Err(plugin_error("room-code", error)),
                        }
                    }
                    "copy" => match &state.snapshot.room {
                        Some(room) => bmcbl_plugin_api::write_clipboard_text(&room.code),
                        None => Ok(()),
                    },
                    "stop" => state.start(Command::Stop),
                    "retry" => state.start(Command::RetryTransport),
                    "refresh" => state.start(Command::Snapshot),
                    "advanced" => {
                        state.advanced = !state.advanced;
                        if state.advanced
                            && state.port_input.is_none()
                            && state.config.game_port != 0
                        {
                            state.port_input = Some(state.config.game_port.to_string());
                        }
                        Ok(())
                    }
                    "player-name" => {
                        state.config.player_name = action.value.clone().unwrap_or_default();
                        return false;
                    }
                    "bootstrap-peers" => {
                        state.config.bootstrap_peers = action.value.clone().unwrap_or_default();
                        return false;
                    }
                    "game-port" => {
                        state.port_input = action.value.clone();
                        return false;
                    }
                    "disable-p2p" => {
                        state.config.disable_p2p = action.value.as_deref() == Some("true");
                        Ok(())
                    }
                    "save" => state.save_preferences(),
                    "cancel" => match (&state.connection, state.request) {
                        (Connection::Ready(instance), Some(request))
                            if state.cancel_job.is_none() =>
                        {
                            native::cancel_command(*instance, request)
                                .map(|job| state.cancel_job = Some(job))
                        }
                        _ => Ok(()),
                    },
                    "install" if !state.command_pending() && state.snapshot.room.is_none() => {
                        state.install_native()
                    }
                    "check-update" if !state.command_pending() && state.snapshot.room.is_none() => {
                        state.check_native_update(false)
                    }
                    _ => return false,
                }
                .map(|()| true)
            } else {
                state.complete(&event)
            };
            match result {
                Ok(changed) => changed,
                Err(error) => {
                    state.record_activity(error.message.clone());
                    state.message = error.message;
                    if !matches!(state.connection, Connection::Ready(..)) {
                        state.pending = None;
                    }
                    true
                }
            }
        });
        if changed {
            invalidate_page("main")?;
        }
        Ok(())
    }

    fn render_page(_request: PageRenderRequest) -> PluginResult<ViewTree> {
        Ok(PAGE.with_borrow(page::render))
    }
}
