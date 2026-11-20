use bmcbl_plugin_api::TextNode;
use bmcbl_plugin_api::prelude::*;

use super::{Connection, PageState, Update};

pub(super) const NATIVE_SETUP: &str = "请先安装原生组件，再创建或加入房间。";

pub(super) fn render(state: &PageState) -> ViewTree {
    let component =
        state.config.module_digest().is_some() || matches!(state.connection, Connection::Ready(_));
    let busy = state.command_pending();
    let room = state.snapshot.room.as_ref();
    let mut details = View::column().gap(14).full_width();
    if room.is_some() {
        details = details.child(members(state));
    }
    details = details
        .child(activity(state))
        .child(session(state))
        .child(peers(state));
    if room.is_none() && !busy {
        details = details.child(native_component(state, component));
    }

    View::columns()
        .gap(14)
        .child(room_card(state, component, busy))
        .child(details.finish_view())
        .finish()
}

fn room_card(state: &PageState, component: bool, busy: bool) -> View {
    let mut body = vec![
        heading("users", "开始联机"),
        detail("创建新房间，或使用好友分享的联机码直接加入。"),
    ];

    if let Some(room) = &state.snapshot.room {
        if room.discovery_port_occupied {
            body.push(notice("本机发现端口被占用，关闭占用程序后请重试。"));
        } else {
            body.push(View::badge("已连接，可以进入 Minecraft"));
        }
        body.push(inset([
            detail(if room.is_host {
                "分享此联机码"
            } else {
                "当前房间"
            }),
            View::title(room.code.clone()),
        ]));
        let mut actions = vec![View::secondary_button("复制联机码", "copy")];
        if !busy {
            if room.discovery_port_occupied {
                actions.push(View::button("重试发现", "retry"));
            }
            actions.push(View::secondary_button("断开连接", "stop"));
        }
        body.push(row(actions));
    } else if busy {
        body.push(inset([
            View::title(
                state
                    .activity
                    .back()
                    .map(String::as_str)
                    .unwrap_or("请稍候"),
            ),
            detail("正在建立网络，完成后即可进入 Minecraft。"),
        ]));
    } else if component {
        body.push(inset([
            View::title("创建我的房间"),
            detail("自动生成联机码并复制到剪贴板"),
            row([View::button("立即创建", "host")]),
        ]));
        body.push(inset([
            View::title("加入好友房间"),
            View::input(&state.room_code, "P/NNNN-NNNN-SSSS-SSSS", "room-code"),
            row([
                View::secondary_button("粘贴", "paste"),
                View::button("加入", "join"),
            ]),
        ]));
        body.push(advanced(state));
    } else {
        body.push(detail(NATIVE_SETUP));
    }
    if state.request.is_some() && state.cancel_job.is_none() {
        body.push(row([View::secondary_button("取消", "cancel")]));
    }
    panel(body)
}

fn advanced(state: &PageState) -> View {
    let mut fields = vec![row([View::secondary_button(
        if state.advanced {
            "收起高级选项"
        } else {
            "高级选项"
        },
        "advanced",
    )])];
    if state.advanced {
        fields.extend([
            detail("玩家名"),
            View::input(&state.config.player_name, "留空时自动生成", "player-name"),
            detail("本机游戏端口"),
            View::input(
                state.port_input.as_deref().unwrap_or_default(),
                "自动探测",
                "game-port",
            ),
            detail("引导节点"),
            View::input(
                &state.config.bootstrap_peers,
                "留空以使用公共节点",
                "bootstrap-peers",
            ),
            View::toggle("禁用 P2P", state.config.disable_p2p, "disable-p2p"),
            row([View::button("保存选项", "save")]),
        ]);
    }
    inset(fields)
}

fn members(state: &PageState) -> View {
    let mut content = vec![heading("users", "房间成员")];
    if state.snapshot.players.is_empty() {
        content.push(detail("等待玩家进入游戏。"));
    }
    content.extend(
        state
            .snapshot
            .players
            .iter()
            .take(32)
            .cloned()
            .map(View::text),
    );
    if !state.command_pending() {
        content.push(row([View::secondary_button("刷新成员和节点", "refresh")]));
    }
    panel(content)
}

fn activity(state: &PageState) -> View {
    let mut content = vec![heading("activity", "活动与提示")];
    if !state.message.is_empty() {
        content.push(detail(&state.message));
    }
    if state.activity.is_empty() {
        content.push(detail(
            "创建或加入房间后等待数秒，再进入 Minecraft。大多数游戏使用默认高级选项即可。",
        ));
    } else {
        content.extend(state.activity.iter().rev().map(detail));
    }
    panel(content)
}

fn session(state: &PageState) -> View {
    let room = state.snapshot.room.as_ref();
    let status = if state.command_pending() {
        "正在处理"
    } else if room.is_some() {
        "已连接"
    } else {
        "等待连接"
    };
    let game_port = room
        .and_then(|room| room.game_port)
        .map_or_else(|| "连接后显示".to_string(), |port| port.to_string());
    let mut content = vec![
        heading("settings", "当前会话"),
        View::badge(status),
        detail(format!(
            "虚拟 IP  {}",
            room.and_then(|room| room.virtual_ip.as_deref())
                .unwrap_or("连接后显示")
        )),
        detail(format!(
            "节点名称  {}",
            room.and_then(|room| room.hostname.as_deref())
                .unwrap_or("连接后显示")
        )),
        detail(match room.and_then(|room| room.game_host.as_deref()) {
            Some(host) => format!("Minecraft 地址  {host}:{game_port}"),
            None => format!("Minecraft 端口  {game_port}"),
        }),
    ];
    if let Some(ports) = &state.ports {
        let text = if ports.is_empty() {
            "未发现正在监听的游戏 UDP 端口。".to_string()
        } else {
            format!(
                "本机游戏端口  {}",
                ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join("、")
            )
        };
        content.push(detail(text));
    }
    let nat = state.snapshot.nat.map_or_else(
        || "尚未检测 NAT".into(),
        |nat| format!("NAT  UDP {} · TCP {}", nat_type(nat.udp), nat_type(nat.tcp)),
    );
    content.push(detail(nat));
    if state.config.module_digest().is_some() && !state.command_pending() {
        content.push(row([
            View::secondary_button("检测游戏 UDP 端口", "detect"),
            View::secondary_button("检测 NAT", "check-nat"),
        ]));
    }
    panel(content)
}

fn nat_type(value: i32) -> &'static str {
    match value {
        1 => "开放网络",
        2 => "完全锥形",
        3 => "受限锥形",
        4 => "端口受限",
        5 => "对称型",
        _ => "未知",
    }
}

fn peers(state: &PageState) -> View {
    let mut content = vec![heading("network", "网络节点")];
    if state.snapshot.peers.is_empty() {
        content.push(detail("连接房间后可查看局域网节点与中转信息。"));
    }
    for peer in state.snapshot.peers.iter().take(32) {
        let latency = peer
            .latency_ms
            .map_or_else(|| "延迟未知".to_owned(), |ms| format!("{ms} ms"));
        let name = if peer.hostname.is_empty() {
            &peer.id
        } else {
            &peer.hostname
        };
        content.push(detail(format!("{name}  ·  {latency}")));
        if let Some(relay) = &peer.relay {
            content.push(detail(format!("经由 {relay} 中转")));
        } else if let Some(protocol) = &peer.protocol {
            content.push(detail(protocol));
        }
    }
    panel(content)
}

fn native_component(state: &PageState, installed: bool) -> View {
    let mut content = vec![heading("plug", "原生组件")];
    if let Some(version) = &state.config.native_version {
        content.push(detail(format!("已安装 v{version}")));
    }
    match &state.update {
        Update::Idle if installed => {
            content.push(row([View::secondary_button("检查更新", "check-update")]))
        }
        Update::Idle => content.push(row([View::button("安装原生组件", "install")])),
        Update::Available(component) => content.push(row([View::button(
            format!("安装 {}", component.version()),
            "install",
        )])),
        _ => {}
    }
    panel(content)
}

fn heading(icon: &str, title: &str) -> View {
    View::row()
        .align(Align::Center)
        .gap(8)
        .child(View::node(bmcbl_plugin_api::icon(icon)))
        .child(View::title(title))
        .finish_view()
}

fn row(children: impl IntoIterator<Item = View>) -> View {
    let mut row = View::row().gap(8).align(Align::Center);
    for child in children {
        row = row.child(child);
    }
    row.finish_view()
}

fn panel(children: impl IntoIterator<Item = View>) -> View {
    let mut panel = View::column()
        .full_width()
        .gap(14)
        .padding(18)
        .corner_radius(16)
        .background(ThemeToken::Surface);
    for child in children {
        panel = panel.child(child);
    }
    panel.finish_view()
}

fn inset(children: impl IntoIterator<Item = View>) -> View {
    let mut inner = View::column().full_width().gap(9).padding(8);
    for child in children {
        inner = inner.child(child);
    }
    inner.finish_view()
}

fn detail(text: impl Into<String>) -> View {
    text_with_color(text, ThemeToken::SecondaryText)
}

fn notice(text: impl Into<String>) -> View {
    text_with_color(text, ThemeToken::Danger)
}

fn text_with_color(text: impl Into<String>, color: ThemeToken) -> View {
    View::node(ViewNode::Text(TextNode {
        text: text.into(),
        style: ViewStyle {
            color: Some(color),
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
        let page = render(&state);
        assert_eq!(
            actions(&page),
            [
                "host",
                "paste",
                "join",
                "advanced",
                "detect",
                "check-nat",
                "check-update"
            ]
        );
        assert!(
            page.nodes.iter().any(
                |node| matches!(node, ViewNode::Input(input) if input.action_id == "room-code")
            )
        );
        state.request = Some(RequestId(1));
        assert_eq!(actions(&render(&state)), ["cancel"]);
        state.request = None;
        state.snapshot.room = Some(Room {
            code: "ROOM".into(),
            is_host: true,
            game_port: None,
            discovery_port_occupied: true,
            ..Room::default()
        });
        assert_eq!(
            actions(&render(&state)),
            ["copy", "retry", "stop", "refresh", "detect", "check-nat"]
        );
    }
}
