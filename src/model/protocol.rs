//! WebSocket protocol messages (SPEC §6.1, §5.11).
//!
//! Every JSON frame on `/ws/control`, `/ws/data` and `/ws/probe` is one of the
//! enums below, tagged by `"t"` in `snake_case`, e.g. `{"t":"ping","seq":1}`.
//! Binary frames (payload) are not modelled here.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::profile::Profile;
use super::result::GroupResult;

/// Version of this wire protocol, advertised in [`ServerMsg::Welcome`].
/// Bump on any incompatible change.
pub const PROTOCOL_VERSION: u32 = 1;

/// Which of a session's sockets a connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// `/ws/control`: presence, session control, groups, WebRTC signalling.
    Control,
    /// `/ws/data`: download and upload transfers.
    Data,
    /// `/ws/probe`: ping/pong for idle and loaded latency.
    Probe,
}

/// Broad device type, used for icons and default names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Phone,
    Tablet,
    Laptop,
    Desktop,
    Tv,
    #[default]
    Other,
}

/// Identity a browser announces on its control socket (SPEC §7.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Random per-browser ID kept in `localStorage`.
    pub device_id: String,
    /// Friendly name, e.g. "iPhone" or a user-chosen one.
    pub device_name: String,
    pub device_kind: DeviceKind,
}

/// A measurement phase, as shown in presence and recorded in results.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Not measuring.
    #[default]
    Idle,
    Latency,
    Download,
    Upload,
    PacketLoss,
    /// A deliberate gap between phases (group `concurrent` profile).
    Pause,
}

/// What a peer is doing right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerState {
    #[default]
    Idle,
    /// Live rates are computed by the server from its byte counters (§5.11).
    Testing {
        phase: Phase,
        #[serde(default)]
        down_bps: f64,
        #[serde(default)]
        up_bps: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latency_ms: Option<f64>,
    },
}

/// One connected device in the Devices panel. Hidden devices are not listed
/// (see [`ServerMsg::Peers::hidden_active`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    /// The peer's session ID.
    pub id: Uuid,
    pub device_name: String,
    pub device_kind: DeviceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// True on the copy sent to the peer itself.
    #[serde(default)]
    pub is_you: bool,
    #[serde(default)]
    pub state: PeerState,
}

/// Lifecycle of a group test.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupState {
    /// Created; members can join. The host can start.
    #[default]
    Forming,
    /// `GroupStart` sent; counting down.
    Starting,
    /// The common measurement window is running.
    Running,
    /// Results computed and sent.
    Finished,
    /// Cancelled (host left or aborted).
    Cancelled,
}

/// Server limits, advertised in [`ServerMsg::Welcome`] and enforced (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub max_bytes_per_request: u64,
    pub max_streams: u8,
    pub max_concurrent_sessions: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes_per_request: 2_000_000_000,
            max_streams: 8,
            max_concurrent_sessions: 16,
        }
    }
}

/// Machine-readable error kinds carried by [`ServerMsg::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// `max_concurrent_sessions` reached.
    Busy,
    /// Malformed or unexpected message.
    BadRequest,
    /// A request exceeded a [`Limits`] value.
    LimitExceeded,
    /// Missing or wrong auth token.
    Unauthorized,
    /// Unknown session, group or run.
    NotFound,
    /// Operation not allowed in the current state (e.g. non-host `GroupStart`).
    Conflict,
    /// Something timed out (idle session, ICE).
    Timeout,
    /// Server-side failure.
    Internal,
}

/// Messages sent by the browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// First message on every socket. `device` is sent on the control socket;
    /// `auth` carries the optional `SPEEDTEST_AUTH_TOKEN` (§11).
    Hello {
        session: Uuid,
        role: Role,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<DeviceInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<String>,
    },
    /// Presence update: current phase (`None` = idle) and label.
    Status {
        #[serde(default)]
        phase: Option<Phase>,
        #[serde(default)]
        label: Option<String>,
    },
    /// "Hide me" (§5.11).
    SetHidden {
        hidden: bool,
    },
    /// Creates a group with the sender as host. `direction_ms`: the download (and
    /// upload) time the members will run, in ms; the server clamps it to 2–300 s and
    /// uses `--group-test-seconds` when absent.
    GroupCreate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction_ms: Option<u32>,
    },
    GroupJoin {
        group: Uuid,
    },
    GroupLeave,
    /// Host only. `direction_ms` (optional) replaces the length chosen at
    /// `GroupCreate` (same clamp).
    GroupStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction_ms: Option<u32>,
    },
    /// Latency probe (probe socket).
    Ping {
        seq: u32,
    },
    /// Ask for `bytes` of payload (data socket).
    Download {
        id: u32,
        bytes: u64,
    },
    /// Announce `bytes` of payload that follow as binary frames (data socket).
    Upload {
        id: u32,
        bytes: u64,
    },
    /// WebRTC offer for the packet-loss data channel.
    RtcOffer {
        sdp: String,
    },
    /// Trickled ICE candidate (`candidate:` line).
    Ice {
        candidate: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sdp_mid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sdp_m_line_index: Option<u16>,
    },
    /// Stop whatever this session is doing.
    Abort,
}

/// Messages sent by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Reply to `Hello`.
    Welcome {
        server_version: String,
        server_name: String,
        #[serde(default = "default_protocol_version")]
        protocol_version: u32,
        limits: Limits,
    },
    Pong {
        seq: u32,
    },
    /// All bytes of download `id` sent; `server_ms` is the server's send time.
    DownloadDone {
        id: u32,
        server_ms: f64,
    },
    /// Upload `id` fully received; `server_ms` is first-to-last-byte on the server.
    UploadDone {
        id: u32,
        bytes: u64,
        server_ms: f64,
    },
    RtcAnswer {
        sdp: String,
    },
    Ice {
        candidate: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sdp_mid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sdp_m_line_index: Option<u16>,
    },
    /// Full presence snapshot, at most 4 Hz.
    Peers {
        peers: Vec<Peer>,
        /// Hidden devices currently testing.
        #[serde(default)]
        hidden_active: u32,
    },
    /// `host` is the inviting device's name.
    GroupInvite {
        group: Uuid,
        host: String,
        /// Download (and upload) time of the group's plan, in ms.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction_ms: Option<u32>,
    },
    GroupUpdate {
        group: Uuid,
        /// Session ID of the host.
        host: Uuid,
        members: Vec<Peer>,
        state: GroupState,
        /// Download (and upload) time of the group's plan, in ms.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction_ms: Option<u32>,
    },
    /// Start `plan` in `starts_in_ms`.
    GroupStart {
        group: Uuid,
        starts_in_ms: u32,
        plan: Profile,
    },
    GroupResult {
        group: Uuid,
        result: GroupResult,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

fn default_protocol_version() -> u32 {
    PROTOCOL_VERSION
}

impl ClientMsg {
    /// Serializes to a JSON text frame.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ClientMsg is always serializable")
    }

    /// Parses a JSON text frame.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

impl ServerMsg {
    /// Serializes to a JSON text frame.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ServerMsg is always serializable")
    }

    /// Parses a JSON text frame.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Convenience constructor for [`ServerMsg::Error`].
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        ServerMsg::Error {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::profile::Profile;
    use crate::model::result::{GroupMemberInput, GroupResult};

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn peer() -> Peer {
        Peer {
            id: uuid(7),
            device_name: "iPhone".into(),
            device_kind: DeviceKind::Phone,
            label: Some("Bedroom".into()),
            is_you: false,
            state: PeerState::Testing {
                phase: Phase::Download,
                down_bps: 5e8,
                up_bps: 0.0,
                latency_ms: Some(3.5),
            },
        }
    }

    fn client_msgs() -> Vec<ClientMsg> {
        vec![
            ClientMsg::Hello {
                session: uuid(1),
                role: Role::Control,
                device: Some(DeviceInfo {
                    device_id: "dev-1".into(),
                    device_name: "Office Mac".into(),
                    device_kind: DeviceKind::Laptop,
                }),
                auth: Some("secret".into()),
            },
            ClientMsg::Hello {
                session: uuid(1),
                role: Role::Probe,
                device: None,
                auth: None,
            },
            ClientMsg::Status {
                phase: Some(Phase::Upload),
                label: Some("Desk".into()),
            },
            ClientMsg::Status {
                phase: None,
                label: None,
            },
            ClientMsg::SetHidden { hidden: true },
            ClientMsg::GroupCreate { direction_ms: None },
            ClientMsg::GroupCreate {
                direction_ms: Some(60_000),
            },
            ClientMsg::GroupJoin { group: uuid(9) },
            ClientMsg::GroupLeave,
            ClientMsg::GroupStart { direction_ms: None },
            ClientMsg::GroupStart {
                direction_ms: Some(3000),
            },
            ClientMsg::Ping { seq: 1 },
            ClientMsg::Download {
                id: 3,
                bytes: 1_000_000,
            },
            ClientMsg::Upload {
                id: 4,
                bytes: 250_000_000,
            },
            ClientMsg::RtcOffer { sdp: "v=0".into() },
            ClientMsg::Ice {
                candidate: "candidate:1 1 udp 1 192.168.1.2 3478 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_m_line_index: Some(0),
            },
            ClientMsg::Abort,
        ]
    }

    fn server_msgs() -> Vec<ServerMsg> {
        let result = GroupResult::compute(
            uuid(9),
            "2026-10-10T00:00:00Z".into(),
            10_000,
            vec![GroupMemberInput {
                session: uuid(7),
                device_name: "iPhone".into(),
                device_kind: DeviceKind::Phone,
                label: None,
                download_bps: Some(3e8),
                upload_bps: Some(2e8),
                down_loaded_latency_ms: Some(12.0),
                up_loaded_latency_ms: None,
                incomplete: false,
                series: vec![],
                download_peak_bps: Some(3.2e8),
                upload_peak_bps: None,
            }],
            None,
        );
        vec![
            ServerMsg::Welcome {
                server_version: "0.1.0".into(),
                server_name: "nas".into(),
                protocol_version: PROTOCOL_VERSION,
                limits: Limits::default(),
            },
            ServerMsg::Pong { seq: 1 },
            ServerMsg::DownloadDone {
                id: 3,
                server_ms: 12.5,
            },
            ServerMsg::UploadDone {
                id: 4,
                bytes: 1000,
                server_ms: 1.25,
            },
            ServerMsg::RtcAnswer { sdp: "v=0".into() },
            ServerMsg::Ice {
                candidate: "candidate:x".into(),
                sdp_mid: None,
                sdp_m_line_index: None,
            },
            ServerMsg::Peers {
                peers: vec![
                    peer(),
                    Peer {
                        state: PeerState::Idle,
                        is_you: true,
                        label: None,
                        ..peer()
                    },
                ],
                hidden_active: 1,
            },
            ServerMsg::GroupInvite {
                group: uuid(9),
                host: "Office Mac".into(),
                direction_ms: Some(60_000),
            },
            ServerMsg::GroupUpdate {
                group: uuid(9),
                host: uuid(1),
                members: vec![peer()],
                state: GroupState::Forming,
                direction_ms: Some(60_000),
            },
            ServerMsg::GroupStart {
                group: uuid(9),
                starts_in_ms: 3000,
                plan: Profile::concurrent(),
            },
            ServerMsg::GroupResult {
                group: uuid(9),
                result,
            },
            ServerMsg::error(ErrorCode::Busy, "Server busy"),
        ]
    }

    #[test]
    fn client_round_trip_every_variant() {
        for m in client_msgs() {
            let json = m.to_json();
            assert_eq!(ClientMsg::from_json(&json).unwrap(), m, "{json}");
        }
    }

    #[test]
    fn server_round_trip_every_variant() {
        for m in server_msgs() {
            let json = m.to_json();
            assert_eq!(ServerMsg::from_json(&json).unwrap(), m, "{json}");
        }
    }

    #[test]
    fn golden_client_json() {
        assert_eq!(
            ClientMsg::Ping { seq: 1 }.to_json(),
            r#"{"t":"ping","seq":1}"#
        );
        assert_eq!(
            ClientMsg::Download {
                id: 2,
                bytes: 100000
            }
            .to_json(),
            r#"{"t":"download","id":2,"bytes":100000}"#
        );
        assert_eq!(
            ClientMsg::GroupCreate { direction_ms: None }.to_json(),
            r#"{"t":"group_create"}"#
        );
        assert_eq!(
            ClientMsg::GroupStart {
                direction_ms: Some(30_000)
            }
            .to_json(),
            r#"{"t":"group_start","direction_ms":30000}"#
        );
        // Messages of older clients and servers, without the length.
        let old_create: ClientMsg = serde_json::from_str(r#"{"t":"group_create"}"#).unwrap();
        assert_eq!(old_create, ClientMsg::GroupCreate { direction_ms: None });
        let old_start: ClientMsg = serde_json::from_str(r#"{"t":"group_start"}"#).unwrap();
        assert_eq!(old_start, ClientMsg::GroupStart { direction_ms: None });
        let invite: ServerMsg = serde_json::from_str(
            r#"{"t":"group_invite","group":"00000000-0000-0000-0000-000000000009","host":"Mac"}"#,
        )
        .unwrap();
        assert_eq!(
            invite,
            ServerMsg::GroupInvite {
                group: uuid(9),
                host: "Mac".into(),
                direction_ms: None
            }
        );
        assert_eq!(ClientMsg::Abort.to_json(), r#"{"t":"abort"}"#);
        assert_eq!(
            ClientMsg::Hello {
                session: uuid(1),
                role: Role::Data,
                device: None,
                auth: None
            }
            .to_json(),
            r#"{"t":"hello","session":"00000000-0000-0000-0000-000000000001","role":"data"}"#
        );
        assert_eq!(
            ClientMsg::SetHidden { hidden: false }.to_json(),
            r#"{"t":"set_hidden","hidden":false}"#
        );
    }

    #[test]
    fn golden_server_json() {
        assert_eq!(
            ServerMsg::Pong { seq: 1 }.to_json(),
            r#"{"t":"pong","seq":1}"#
        );
        assert_eq!(
            ServerMsg::UploadDone {
                id: 4,
                bytes: 1000,
                server_ms: 2.5
            }
            .to_json(),
            r#"{"t":"upload_done","id":4,"bytes":1000,"server_ms":2.5}"#
        );
        assert_eq!(
            ServerMsg::error(ErrorCode::LimitExceeded, "too big").to_json(),
            r#"{"t":"error","code":"limit_exceeded","message":"too big"}"#
        );
        assert_eq!(
            ServerMsg::Welcome {
                server_version: "0.1.0".into(),
                server_name: "nas".into(),
                protocol_version: 1,
                limits: Limits::default(),
            }
            .to_json(),
            r#"{"t":"welcome","server_version":"0.1.0","server_name":"nas","protocol_version":1,"limits":{"max_bytes_per_request":2000000000,"max_streams":8,"max_concurrent_sessions":16}}"#
        );
    }

    #[test]
    fn golden_peer_state_json() {
        assert_eq!(
            serde_json::to_string(&PeerState::Idle).unwrap(),
            r#"{"kind":"idle"}"#
        );
        assert_eq!(
            serde_json::to_string(&PeerState::Testing {
                phase: Phase::PacketLoss,
                down_bps: 1.0,
                up_bps: 2.0,
                latency_ms: None
            })
            .unwrap(),
            r#"{"kind":"testing","phase":"packet_loss","down_bps":1.0,"up_bps":2.0}"#
        );
    }

    #[test]
    fn parses_spec_examples_and_lenient_fields() {
        // From SPEC §6.1: hello without device/auth.
        let m = ClientMsg::from_json(
            r#"{"t":"hello","session":"00000000-0000-0000-0000-000000000002","role":"probe"}"#,
        )
        .unwrap();
        assert!(matches!(
            m,
            ClientMsg::Hello {
                role: Role::Probe,
                ..
            }
        ));
        // Status with fields omitted.
        assert_eq!(
            ClientMsg::from_json(r#"{"t":"status"}"#).unwrap(),
            ClientMsg::Status {
                phase: None,
                label: None
            }
        );
        // Welcome from an older server without protocol_version / limits fields.
        let w = ServerMsg::from_json(
            r#"{"t":"welcome","server_version":"0","server_name":"x","limits":{}}"#,
        )
        .unwrap();
        assert_eq!(
            w,
            ServerMsg::Welcome {
                server_version: "0".into(),
                server_name: "x".into(),
                protocol_version: PROTOCOL_VERSION,
                limits: Limits::default()
            }
        );
    }

    #[test]
    fn rejects_unknown_tags() {
        assert!(ClientMsg::from_json(r#"{"t":"nope"}"#).is_err());
        assert!(ServerMsg::from_json(r#"{"seq":1}"#).is_err());
    }
}
