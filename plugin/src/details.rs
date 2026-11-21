use bmcbl_plugin_api::prelude::*;
use paperconnect_protocol::Nat;

use super::PageState;
use super::page::{detail, flat_action, label, page_body, row, section, session_field};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Diagnostics,
    Nodes,
}

pub(super) fn diagnostics(state: &PageState) -> View {
    let ports = match state.ports.as_deref() {
        None => "尚未检测".to_owned(),
        Some([]) => "未发现游戏端口".to_owned(),
        Some(ports) => ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join("、"),
    };
    let mut body = page_body()
        .gap(4)
        .child(session_field("本机游戏 UDP 端口", &ports));
    if !state.command_pending() {
        body = body.child(
            row()
                .child(flat_action("检测 UDP 端口", "detect"))
                .child(flat_action("检测 NAT", "check-nat")),
        );
    }
    body.finish_view()
}

pub(super) fn nodes(state: &PageState) -> View {
    let mut peers = page_body().gap(4);
    if state.snapshot.peers.is_empty() {
        peers = peers.child(detail("当前没有网络节点。"));
    }
    for peer in state.snapshot.peers.iter().take(32) {
        let name = if peer.hostname.is_empty() {
            &peer.id
        } else {
            &peer.hostname
        };
        let latency = peer
            .latency_ms
            .map_or_else(|| "延迟未知".into(), |ms| format!("{ms} ms"));
        let transport = peer
            .relay
            .as_deref()
            .map(|relay| format!("经由 {relay} 中转"))
            .or_else(|| peer.protocol.clone());
        let status =
            transport.map_or_else(|| latency.clone(), |route| format!("{latency} · {route}"));
        peers = peers.child(session_field(name, &status));
    }
    let mut body = page_body().gap(6).child(peers);
    if state.snapshot.room.is_some() {
        body = body
            .child(View::node(ViewNode::Separator))
            .child(players(state));
        if !state.command_pending() {
            body = body.child(flat_action("刷新", "refresh"));
        }
    }
    body.finish_view()
}

fn players(state: &PageState) -> View {
    let mut players = section("房间成员");
    if state.snapshot.players.is_empty() {
        players = players.child(detail("等待玩家进入游戏。"));
    }
    for player in state.snapshot.players.iter().take(32) {
        players = players.child(label(player));
    }
    players.finish_view()
}

pub(super) fn format_nat(nat: Nat) -> String {
    format!("UDP {} · TCP {}", nat_type(nat.udp), nat_type(nat.tcp))
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
