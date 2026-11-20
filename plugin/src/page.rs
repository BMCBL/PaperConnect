use bmcbl_plugin_api::TextNode;
use bmcbl_plugin_api::prelude::*;

use super::{Connection, PageState, Update};

pub(super) const NATIVE_SETUP: &str = "请先安装原生组件，再创建或加入房间。";

pub(super) fn render(state: &PageState) -> ViewTree {
    let native_available =
        state.config.module_digest().is_some() || matches!(state.connection, Connection::Ready(_));
    let busy = state.command_pending();
    let mut page = View::column()
        .padding(6)
        .gap(14)
        .full_width()
        .child(View::title("PaperConnect"));
    if !state.message.is_empty() {
        page = page.child(detail(&state.message));
    }
    page = page.child(room_card(state, native_available, busy));
    if native_available && (!busy || state.ports.is_some()) {
        page = page.child(diagnostics_card(state, busy));
    }
    if state.snapshot.room.is_none() && !busy {
        let mut items = vec![View::title("原生组件")];
        if let Some(version) = &state.config.native_version {
            items.push(detail(format!("已安装 {version}")));
        } else if !native_available {
            items.push(detail(NATIVE_SETUP));
        }
        match &state.update {
            Update::Idle if native_available => {
                items.push(View::button("检查更新", "check-update"));
            }
            Update::Idle => items.push(View::button("安装原生组件", "install")),
            Update::Available(component) => items.push(View::button(
                format!("安装 {}", component.version()),
                "install",
            )),
            _ => {}
        }
        page = page.child(card(items));
    }
    page.finish()
}

fn room_card(state: &PageState, native_available: bool, busy: bool) -> View {
    let room = state.snapshot.room.as_ref();
    let status = room.map_or("尚未加入房间", |room| {
        if room.is_host { "房主" } else { "来宾" }
    });
    let heading = View::row()
        .padding(0)
        .gap(10)
        .align(Align::Center)
        .child(View::title("房间"))
        .child(View::badge(status));
    let mut items = vec![heading.into()];
    let mut actions = View::row().padding(0).gap(8).align(Align::Center);
    if let Some(room) = room {
        items.push(View::text(format!("房间码  {}", room.code)));
        items.push(detail(format!(
            "游戏端口 {}  ·  {} 个节点",
            room.game_port
                .map_or_else(|| "等待发现".into(), |port| port.to_string()),
            state.snapshot.peers.len(),
        )));
        if !state.snapshot.players.is_empty() {
            items.push(detail(format!(
                "玩家  {}",
                state.snapshot.players.join("、")
            )));
        }
        if room.discovery_port_occupied {
            items.push(detail("本机发现端口被占用，释放端口后可重试。"));
        }
        actions = actions.child(View::button("复制房间码", "copy"));
        if !busy {
            if room.discovery_port_occupied {
                actions = actions.child(View::button("重试发现", "retry"));
            }
            actions = actions.child(View::button("退出房间", "stop"));
        }
    } else if native_available && !busy {
        actions = actions
            .child(View::button("创建房间", "host"))
            .child(View::button("从剪贴板加入", "join"));
    }
    let can_cancel = state.request.is_some() && state.cancel_job.is_none();
    if can_cancel {
        actions = actions.child(View::button("取消命令", "cancel"));
    }
    if room.is_some() || (native_available && !busy) || can_cancel {
        items.push(actions.into());
    }
    card(items)
}

fn diagnostics_card(state: &PageState, busy: bool) -> View {
    let mut items = vec![View::title("本机诊断")];
    if let Some(ports) = &state.ports {
        let result = if ports.is_empty() {
            "未发现正在监听的游戏 UDP 端口。".to_owned()
        } else {
            let ports = ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join("、");
            format!("游戏 UDP 端口  {ports}")
        };
        items.push(detail(result));
    }
    if !busy {
        items.push(View::button("检测游戏 UDP 端口", "detect"));
    }
    card(items)
}

fn detail(text: impl Into<String>) -> View {
    View::node(ViewNode::Text(TextNode {
        text: text.into(),
        style: ViewStyle {
            color: Some(ThemeToken::SecondaryText),
            full_width: true,
            ..default_style()
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use bmcbl_plugin_api::native::RequestId;
    use paperconnect_protocol::Room;

    fn actions(page: &ViewTree) -> Vec<&str> {
        page.nodes
            .iter()
            .filter_map(|node| match node {
                ViewNode::Button(button) => Some(button.action_id.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn room_actions_follow_session_and_command_state() {
        if option_env!("PAPERCONNECT_NATIVE_SHA256").is_none() {
            assert_eq!(actions(&render(&PageState::default())), ["install"]);
        }
        let mut state = PageState {
            config: Config {
                native_sha256: Some("configured".into()),
                ..Config::default()
            },
            ..PageState::default()
        };
        assert_eq!(
            actions(&render(&state)),
            ["host", "join", "detect", "check-update"]
        );
        state.request = Some(RequestId(1));
        assert_eq!(actions(&render(&state)), ["cancel"]);
        state.request = None;
        state.snapshot.room = Some(Room {
            code: "ROOM".into(),
            is_host: true,
            game_port: None,
            discovery_port_occupied: false,
        });
        assert_eq!(actions(&render(&state)), ["copy", "stop", "detect"]);
        state
            .snapshot
            .room
            .as_mut()
            .unwrap()
            .discovery_port_occupied = true;
        assert_eq!(
            actions(&render(&state)),
            ["copy", "retry", "stop", "detect"]
        );
    }
}
