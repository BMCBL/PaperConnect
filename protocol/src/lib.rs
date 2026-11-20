//! Private control protocol shared by the PaperConnect Wasm and native builds.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Preferences {
    pub player: String,
    pub peers: Vec<String>,
    pub game_port: Option<u16>,
    pub disable_p2p: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Command {
    Snapshot,
    DetectPorts,
    RetryTransport,
    Host {
        code: String,
        preferences: Preferences,
    },
    Join {
        code: String,
        preferences: Preferences,
    },
    Stop,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Room {
    pub code: String,
    pub is_host: bool,
    pub game_port: Option<u16>,
    pub discovery_port_occupied: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    pub room: Option<Room>,
    pub players: Vec<String>,
    pub peers: Vec<Peer>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Peer {
    pub id: String,
    pub hostname: String,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Batch {
    pub gap: bool,
    pub events: Vec<SequencedEvent>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SequencedEvent {
    pub sequence: u64,
    pub event: Event,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    Completed {
        request: u64,
    },
    Failed {
        request: u64,
        message: String,
    },
    Cancelled {
        request: u64,
    },
    Snapshot {
        snapshot: Snapshot,
    },
    Ports {
        ports: Vec<u16>,
    },
    RoomChanged {
        room: Option<Room>,
    },
    PlayersChanged {
        joined: Vec<String>,
        left: Vec<String>,
    },
    PeersChanged {
        present: Vec<Peer>,
        left: Vec<String>,
    },
    NetworkError {
        message: String,
    },
}
