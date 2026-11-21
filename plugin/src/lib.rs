use std::cell::RefCell;
use std::collections::VecDeque;

use bmcbl_plugin_api::r#async::Job;
use bmcbl_plugin_api::native::{self, InstanceHandle, ModuleHandle, RequestId};
use bmcbl_plugin_api::prelude::*;
use paperconnect_protocol::{Batch, Command, Event, Preferences, Snapshot};

mod details;
mod page;
mod room;
mod settings;
mod updates;

use updates::Update;

const PLUGIN: PluginMetadata = plugin_metadata!();

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
    early_result: Option<(RequestId, CommandOutcome)>,
    operation: Option<&'static str>,
    update: Update,
    snapshot: Snapshot,
    ports: Option<Vec<u16>>,
    message: String,
    problem: Option<String>,
    activity: VecDeque<String>,
    room_code: String,
    settings: settings::Settings,
    expanded_details: Option<details::Section>,
    component_load_failed: bool,
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

enum CommandOutcome {
    Completed,
    Failed(String),
    Cancelled,
}

thread_local! {
    static PAGE: RefCell<PageState> = RefCell::default();
}

impl PageState {
    fn component_available(&self) -> bool {
        matches!(self.connection, Connection::Ready(_))
            || (!self.component_load_failed && self.config.module_digest().is_some())
    }

    fn command_pending(&self) -> bool {
        self.pending.is_some() || self.submit.is_some() || self.request.is_some()
    }

    fn start(&mut self, command: Command) -> PluginResult<()> {
        if self.command_pending() {
            return Err(plugin_error("busy", "正在执行上一条联机命令。"));
        }
        let operation = match &command {
            Command::Host { .. } => "创建房间",
            Command::Join { .. } => "加入房间",
            Command::Stop => "断开联机",
            Command::RetryTransport => "重试本机游戏发现",
            Command::DetectPorts => "检测游戏 UDP 端口",
            Command::CheckNat => "检测 NAT 类型",
            Command::Snapshot => "刷新会话",
        };
        log_info!("PaperConnect {operation}开始");
        match self.connection {
            Connection::Ready(instance) => {
                self.submit = Some(send(instance, &command)?);
            }
            Connection::Idle => {
                let digest = self
                    .config
                    .module_digest()
                    .ok_or_else(|| plugin_error("native-component", page::NATIVE_SETUP))?;
                log_info("PaperConnect 正在加载原生组件");
                self.connection = Connection::Loading(
                    native::load(digest).map_err(|error| stage_error("加载原生组件", error))?,
                );
                self.pending = Some(command);
            }
            Connection::Loading(_) | Connection::Creating(_, _) => {
                return Err(plugin_error("busy", "联机组件正在启动。"));
            }
        }
        self.operation = Some(operation);
        self.message.clear();
        self.problem = None;
        Ok(())
    }

    fn complete(&mut self, event: &PluginEvent) -> PluginResult<bool> {
        let mut changed = self.advance_update(event)?;
        let connection = std::mem::take(&mut self.connection);
        self.connection = match connection {
            Connection::Loading(job) if job.is_ready(event) => {
                changed = true;
                let module = match ready(job) {
                    Ok(module) => module,
                    Err(error) => {
                        self.component_load_failed = matches!(
                            error.code.as_str(),
                            "app" | "denied" | "invalid-input" | "not-found"
                        );
                        return Err(stage_error("加载原生组件", error));
                    }
                };
                log_info("PaperConnect 原生组件已加载，正在创建会话");
                Connection::Creating(module, start_instance(module)?)
            }
            Connection::Creating(module, job) if job.is_ready(event) => {
                changed = true;
                let instance = match ready(job) {
                    Ok(instance) => instance,
                    Err(error) => {
                        let _ = native::unload(module);
                        return Err(stage_error("创建原生会话", error));
                    }
                };
                log_info("PaperConnect 原生会话已就绪");
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
            let id = ready(self.submit.take().expect("matched submit job"))
                .map_err(|error| stage_error("提交原生命令", error))?;
            self.request = Some(id);
            log_info!(
                "PaperConnect {}已提交，请求 #{}",
                self.operation.unwrap_or("同步会话"),
                id.0
            );
            if let Some((id, outcome)) = self.early_result.take() {
                self.finish_command(id.0, outcome);
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
            let bytes = ready(self.read.take().expect("matched read job"))
                .map_err(|error| stage_error("读取原生事件", error))?;
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
                Event::Completed { request } => {
                    self.finish_command(request, CommandOutcome::Completed);
                }
                Event::Failed { request, message } => {
                    self.finish_command(request, CommandOutcome::Failed(message));
                }
                Event::Cancelled { request } => {
                    self.finish_command(request, CommandOutcome::Cancelled);
                }
                Event::Ports { ports } => {
                    log_info!("PaperConnect 本机游戏 UDP 端口：{ports:?}");
                    self.ports = Some(ports);
                }
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
                                self.report_problem(stage_error("复制联机码", error).message);
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
                Event::NetworkError { message } => self.report_problem(message),
            }
        }
        if gap && let Connection::Ready(instance) = self.connection {
            log_warn("PaperConnect 原生事件存在缺口，正在同步会话");
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

    fn report_problem(&mut self, message: String) {
        log_warn!("PaperConnect {message}");
        self.record_activity(message.clone());
        self.problem = Some(message);
    }

    fn finish_command(&mut self, request: u64, outcome: CommandOutcome) {
        let id = RequestId(request);
        if request == 0 || self.request == Some(id) {
            self.request = None;
            let operation = self.operation.take().unwrap_or("同步会话");
            match outcome {
                CommandOutcome::Completed => {
                    log_info!("PaperConnect {operation}已完成，请求 #{request}");
                }
                CommandOutcome::Failed(message) => {
                    self.report_problem(format!("{operation}失败：{message}"));
                }
                CommandOutcome::Cancelled => {
                    log_info!("PaperConnect {operation}已取消，请求 #{request}");
                    self.message = format!("{operation}已取消。");
                }
            }
        } else if self.submit.is_some() {
            self.early_result = Some((id, outcome));
        }
    }
}

fn stage_error(stage: &str, mut error: PluginError) -> PluginError {
    error.message = format!("{stage}失败（{}）：{}", error.code, error.message);
    error
}

fn start_instance(module: ModuleHandle) -> PluginResult<Job<InstanceHandle>> {
    native::create_instance(module, &[]).map_err(|error| {
        let _ = native::unload(module);
        stage_error("创建原生会话", error)
    })
}

fn ready<T: serde::de::DeserializeOwned>(job: Job<T>) -> PluginResult<T> {
    job.try_take()?
        .ok_or_else(|| plugin_error("native-job", "原生任务尚未完成。"))
}

fn send(instance: InstanceHandle, command: &Command) -> PluginResult<Job<RequestId>> {
    let bytes =
        serde_json::to_vec(command).map_err(|error| plugin_error("command", error.to_string()))?;
    native::submit_command(instance, &bytes).map_err(|error| stage_error("提交原生命令", error))
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
        log_info!("PaperConnect v{} 联机大厅已注册", env!("CARGO_PKG_VERSION"));
        Ok(registrations! {
            page "main", title = "创建或加入好友房间", tool = Tool::new("联机大厅").icon("users").order(100);
            page settings::PAGE_ID, title = "联机设置";
            subscribe "config-changed";
        })
    }

    fn handle_event(event: PluginEvent) -> PluginResult<()> {
        if event.page_id.as_deref() == Some(settings::PAGE_ID)
            && let PluginEventKind::Action(action) = &event.kind
        {
            PAGE.with_borrow_mut(|state| state.settings.handle_action(action, &mut state.config));
            return Ok(());
        }
        let changed = PAGE.with_borrow_mut(|state| {
            let result = if event
                .global_event()
                .is_some_and(|(name, _)| name == "config-changed")
            {
                let config = config();
                if state.config.module_digest() != config.module_digest() {
                    state.component_load_failed = false;
                }
                state.config = config;
                Ok(true)
            } else if let PluginEventKind::Action(action) = &event.kind {
                if matches!(
                    action.action_id.as_str(),
                    "host"
                        | "join"
                        | "room-code-submit"
                        | "paste"
                        | "copy"
                        | "install"
                        | "check-update"
                ) && !state.command_pending()
                {
                    state.problem = None;
                    state.message.clear();
                }
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
                        Some(room) => bmcbl_plugin_api::write_clipboard_text(&room.code)
                            .map(|()| state.message = "联机码已复制。".into()),
                        None => Ok(()),
                    },
                    "stop" => state.start(Command::Stop),
                    "retry" => state.start(Command::RetryTransport),
                    "refresh" => state.start(Command::Snapshot),
                    "settings" => {
                        state.settings = settings::Settings::from(&state.config);
                        open_dialog(settings::PAGE_ID, "联机设置", 500, 490)
                    }
                    "diagnostics" | "nodes" => {
                        let section = if action.action_id == "diagnostics" {
                            details::Section::Diagnostics
                        } else {
                            details::Section::Nodes
                        };
                        state.expanded_details =
                            (state.expanded_details != Some(section)).then_some(section);
                        Ok(())
                    }
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
                    if !matches!(state.connection, Connection::Ready(..)) {
                        state.pending = None;
                    }
                    if !state.command_pending() {
                        state.operation = None;
                    }
                    state.report_problem(error.message);
                    true
                }
            }
        });
        if changed {
            invalidate_page("main")?;
        }
        Ok(())
    }

    fn render_page(request: PageRenderRequest) -> PluginResult<ViewTree> {
        Ok(PAGE.with_borrow(|state| match request.page_id.as_str() {
            settings::PAGE_ID => state.settings.render(&state.config),
            _ => page::render(state),
        }))
    }
}

fn open_dialog(page: &str, title: &str, width: u32, height: u32) -> PluginResult<()> {
    invalidate_page(page)?;
    PLUGIN
        .modal(page)
        .title(title)
        .size(width, height)
        .open(PLUGIN)
}
