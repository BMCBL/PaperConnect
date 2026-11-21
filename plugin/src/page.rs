use bmcbl_plugin_api::prelude::*;
use bmcbl_plugin_api::{ButtonNode, Container, InputNode, TextNode, TextSizeToken};
use paperconnect_protocol::Room;

use super::details::{self, Section};
use super::{PageState, Update};

pub(super) const NATIVE_SETUP: &str = "安装原生组件后即可创建或加入房间。";

pub(super) fn render(state: &PageState) -> ViewTree {
    if !state.component_available() {
        return component_setup(state);
    }

    let busy = state.command_pending();
    page_body()
        .gap(8)
        .child(room_controls(state, busy))
        .child(View::node(ViewNode::Separator))
        .child(session(state, busy))
        .child(View::node(ViewNode::Separator))
        .child(network_nodes(state))
        .child(View::node(ViewNode::Separator))
        .child(activity(state))
        .finish()
}

fn room_controls(state: &PageState, busy: bool) -> View {
    let mut body = page_body().gap(8);
    if let Some(room) = state.snapshot.room.as_ref() {
        body = body
            .child(View::title("房间已连接"))
            .child(connected(room, busy));
    } else if !busy {
        body = body
            .child(View::title("多人联机"))
            .child(create_room())
            .child(join_form(state));
    }
    if busy {
        body = body.child(pending_action(state));
    } else if state.snapshot.room.is_none() {
        body = body.child(lobby_actions(state));
    }
    body.finish_view()
}

fn pending_action(state: &PageState) -> View {
    let operation = state.activity.back().map_or("正在处理", String::as_str);
    let mut progress = row().full_width().child(label(operation));
    if state.request.is_some() && state.cancel_job.is_none() {
        progress = progress.child(flat_action("取消操作", "cancel"));
    }
    progress.finish_view()
}

fn component_setup(state: &PageState) -> ViewTree {
    let mut setup = page_body()
        .child(heading("plug", "下载原生组件"))
        .child(detail(NATIVE_SETUP));
    if !state.message.is_empty() {
        setup = setup.child(detail(&state.message));
    }
    match &state.update {
        Update::Idle => {
            let label = if state.component_load_failed {
                "重新下载"
            } else {
                "下载并安装"
            };
            setup = setup.child(View::button(label, "install"));
        }
        Update::Available(update) => {
            setup = setup.child(View::button(
                format!("安装 v{}", update.version()),
                "install",
            ));
        }
        _ => {}
    }
    setup.finish()
}

fn create_room() -> View {
    row()
        .full_width()
        .child(
            section("创建我的房间")
                .gap(4)
                .child(detail("联机码会自动复制到剪贴板。")),
        )
        .child(compact_action("创建房间", "host", ThemeToken::Accent))
        .finish_view()
}

fn join_form(state: &PageState) -> View {
    section("加入好友房间")
        .gap(4)
        .child(
            row()
                .full_width()
                .child(View::node(ViewNode::Input(InputNode {
                    value: state.room_code.clone(),
                    placeholder: "P/NNNN-NNNN-SSSS-SSSS".into(),
                    action_id: "room-code".into(),
                    action_value: None,
                    style: ViewStyle {
                        text_size: TextSizeToken::Small,
                        full_width: true,
                        ..default_style()
                    },
                })))
                .child(flat_action("粘贴", "paste"))
                .child(compact_action("加入", "join", ThemeToken::Accent)),
        )
        .finish_view()
}

fn connected(room: &Room, busy: bool) -> View {
    let label = if room.is_host {
        "分享联机码"
    } else {
        "房间码"
    };
    let mut room_view = section(label).child(View::text(room.code.clone())).child(
        if room.discovery_port_occupied {
            text(
                "本机发现端口被占用，释放端口后请重试。",
                ThemeToken::Danger,
                false,
                true,
            )
        } else {
            detail("已连接，可进入 Minecraft。")
        },
    );
    let mut actions = row().child(compact_action("复制联机码", "copy", ThemeToken::Accent));
    if !busy {
        if room.discovery_port_occupied {
            actions = actions.child(flat_action("重试发现", "retry"));
        }
        actions = actions.child(flat_action("断开", "stop"));
    }
    room_view = room_view.child(actions);
    room_view.finish_view()
}

fn lobby_actions(state: &PageState) -> View {
    let mut actions = row().child(flat_action("联机设置", "settings"));
    match &state.update {
        Update::Idle => {
            actions = actions.child(flat_action("检查组件更新", "check-update"));
        }
        Update::Available(update) => {
            actions = actions.child(View::button(
                format!("安装 v{}", update.version()),
                "install",
            ));
        }
        _ => {}
    }
    actions.finish_view()
}

fn activity(state: &PageState) -> View {
    let mut body = page_body()
        .gap(4)
        .child(section_title("activity", "活动与提示"));
    if !state.message.is_empty() {
        body = body.child(detail(&state.message));
    }
    if state.activity.is_empty() && state.message.is_empty() {
        body = body.child(detail(
            "创建或加入房间后，在 Minecraft 好友列表中进入局域网游戏。",
        ));
    }
    for line in state
        .activity
        .iter()
        .rev()
        .filter(|line| *line != &state.message)
        .take(3)
    {
        body = body.child(detail(line));
    }
    body.finish_view()
}

fn session(state: &PageState, busy: bool) -> View {
    let room = state.snapshot.room.as_ref();
    let ip = room.and_then(|room| room.virtual_ip.as_deref());
    let hostname = room
        .and_then(|room| room.hostname.as_deref())
        .filter(|name| !name.is_empty());
    let address = room.and_then(|room| {
        let port = room.game_port?;
        let address = match room.game_host.as_deref() {
            Some(host) if host.contains(':') => format!("[{host}]:{port}"),
            Some(host) => format!("{host}:{port}"),
            None => port.to_string(),
        };
        Some(address)
    });
    let nat = state.snapshot.nat.map(super::details::format_nat);
    let status = if room.is_some() {
        "已连接"
    } else if busy {
        "处理中"
    } else {
        "等待连接"
    };
    let expanded = state.expanded_details == Some(Section::Diagnostics);
    let caption = if expanded { "收起诊断" } else { "诊断" };
    let mut body = page_body()
        .gap(4)
        .child(section_header(
            "users",
            "当前会话",
            status,
            flat_action(caption, "diagnostics"),
        ))
        .child(session_field("虚拟 IP", ip.unwrap_or("连接后显示")))
        .child(session_field("节点名称", hostname.unwrap_or("尚未连接")))
        .child(session_field(
            "Minecraft 地址",
            address.as_deref().unwrap_or("连接后显示"),
        ))
        .child(session_field("NAT", nat.as_deref().unwrap_or("尚未检测")));
    if expanded {
        body = body.child(details::diagnostics(state));
    }
    body.finish_view()
}

pub(super) fn session_field(caption: &str, value: &str) -> View {
    row()
        .full_width()
        .child(detail(caption))
        .child(View::node(ViewNode::Text(TextNode {
            text: value.into(),
            style: ViewStyle {
                align: Align::End,
                full_width: true,
                ..default_style()
            },
        })))
        .finish_view()
}

fn network_nodes(state: &PageState) -> View {
    let expanded = state.expanded_details == Some(Section::Nodes);
    let caption = if expanded { "收起" } else { "查看" };
    let count = format!("{} 个节点", state.snapshot.peers.len());
    let mut body = page_body().gap(4).child(section_header(
        "network",
        "玩家与节点",
        &count,
        flat_action(caption, "nodes"),
    ));
    if expanded {
        body = body.child(details::nodes(state));
    }
    body.finish_view()
}

pub(super) fn page_body() -> Container {
    View::column().full_width().gap(12).padding(0)
}

fn section_header(icon: &str, title: &str, meta: &str, action: View) -> View {
    row()
        .full_width()
        .child(
            row()
                .full_width()
                .child(View::node(bmcbl_plugin_api::icon(icon)))
                .child(text(title, ThemeToken::PrimaryText, true, false))
                .child(text(meta, ThemeToken::SecondaryText, false, false)),
        )
        .child(action)
        .finish_view()
}

pub(super) fn flat_action(label: &str, action: &str) -> View {
    compact_action(label, action, ThemeToken::SecondaryText)
}

fn compact_action(label: &str, action: &str, color: ThemeToken) -> View {
    View::node(ViewNode::Button(ButtonNode {
        label: label.into(),
        action_id: action.into(),
        action_value: None,
        style: ViewStyle {
            color: Some(color),
            text_size: TextSizeToken::Small,
            ..default_style()
        },
    }))
}

fn heading(icon: &str, title: &str) -> View {
    row()
        .child(View::node(bmcbl_plugin_api::icon(icon)))
        .child(View::title(title))
        .finish_view()
}

fn section_title(icon: &str, title: &str) -> View {
    row()
        .full_width()
        .child(View::node(bmcbl_plugin_api::icon(icon)))
        .child(text(title, ThemeToken::PrimaryText, true, false))
        .finish_view()
}

pub(super) fn row() -> Container {
    View::row().padding(0).gap(8).align(Align::Center)
}

pub(super) fn section(title: &str) -> Container {
    View::column()
        .padding(0)
        .gap(8)
        .full_width()
        .child(label(title))
}

pub(super) fn label(value: &str) -> View {
    text(value, ThemeToken::PrimaryText, true, true)
}

pub(super) fn detail(value: impl Into<String>) -> View {
    text(value, ThemeToken::SecondaryText, false, true)
}

pub(super) fn text(
    value: impl Into<String>,
    color: ThemeToken,
    emphasis: bool,
    full_width: bool,
) -> View {
    View::node(ViewNode::Text(TextNode {
        text: value.into(),
        style: ViewStyle {
            color: Some(color),
            emphasis,
            full_width,
            ..default_style()
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use bmcbl_plugin_api::LayoutDirection;
    use bmcbl_plugin_api::native::RequestId;

    fn actions(page: &ViewTree) -> Vec<&str> {
        page.nodes
            .iter()
            .filter_map(|node| match node {
                ViewNode::Button(button) => Some(button.action_id.as_str()),
                _ => None,
            })
            .collect()
    }

    fn assert_plain_layout(page: &ViewTree) {
        for node in &page.nodes {
            if let ViewNode::Container(container) = node {
                assert_eq!(container.style.background, None);
                assert_eq!(container.style.corner_radius, None);
                assert_eq!(container.style.padding, 0);
            }
        }
    }

    fn assert_lobby_sections(page: &ViewTree) {
        assert_plain_layout(page);
        let ViewNode::Container(root) = &page.nodes[page.root as usize] else {
            panic!("missing lobby layout");
        };
        assert_eq!(root.style.direction, LayoutDirection::Column);
        assert_eq!(root.children.len(), 7);
        assert!(
            root.children
                .iter()
                .skip(1)
                .step_by(2)
                .all(|&index| { matches!(page.nodes[index as usize], ViewNode::Separator) })
        );
        assert!(
            !page
                .nodes
                .iter()
                .any(|node| matches!(node, ViewNode::Badge(_)))
        );
    }

    fn configured() -> PageState {
        PageState {
            config: Config {
                native_sha256: Some("configured".into()),
                ..Config::default()
            },
            ..PageState::default()
        }
    }

    #[test]
    fn unavailable_component_exposes_install_as_the_only_action() {
        if option_env!("PAPERCONNECT_NATIVE_SHA256").is_none() {
            let setup = render(&PageState::default());
            assert_plain_layout(&setup);
            assert_eq!(actions(&setup), ["install"]);
            assert!(
                !setup
                    .nodes
                    .iter()
                    .any(|node| matches!(node, ViewNode::Input(_)))
            );
        }
        let state = PageState {
            component_load_failed: true,
            ..configured()
        };
        let repair = render(&state);
        assert_plain_layout(&repair);
        assert_eq!(actions(&repair), ["install"]);
    }

    #[test]
    fn lobby_renders_actions_and_details_in_the_host_panel() {
        let mut state = configured();
        let lobby = render(&state);
        assert_lobby_sections(&lobby);
        assert_eq!(
            actions(&lobby),
            [
                "host",
                "paste",
                "join",
                "settings",
                "check-update",
                "diagnostics",
                "nodes",
            ]
        );
        assert!(
            lobby.nodes.iter().any(
                |node| matches!(node, ViewNode::Input(input) if input.action_id == "room-code" && input.style.text_size == TextSizeToken::Small)
            )
        );

        let settings = crate::settings::Settings::from(&state.config).render(&state.config);
        assert_plain_layout(&settings);
        assert_eq!(actions(&settings), ["save"]);
        assert!(!actions(&lobby).contains(&"save"));

        state.expanded_details = Some(Section::Diagnostics);
        let diagnostics = render(&state);
        assert_lobby_sections(&diagnostics);
        assert!(actions(&diagnostics).ends_with(&["diagnostics", "detect", "check-nat", "nodes"]));

        state.request = Some(RequestId(1));
        assert_eq!(actions(&render(&state)), ["cancel", "diagnostics", "nodes"]);
        state.request = None;

        state.snapshot.room = Some(Room {
            code: "ROOM".into(),
            is_host: true,
            discovery_port_occupied: true,
            ..Room::default()
        });
        state.expanded_details = Some(Section::Nodes);
        let connected = render(&state);
        assert_lobby_sections(&connected);
        assert_eq!(
            actions(&connected),
            ["copy", "retry", "stop", "diagnostics", "nodes", "refresh"]
        );
    }
}
