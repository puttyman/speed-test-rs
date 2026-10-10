//! The page-wide presence client (SPEC §5.11, §7.11).
//!
//! One long-lived `/ws/control` socket per page, opened on page load and kept
//! open (with reconnects) until [`PresenceHandle::close`]:
//!
//! ```ignore
//! let presence = engine::presence::connect_presence(
//!     PresenceOptions::new(engine::detect_device()),
//!     move |ev| match ev {
//!         PresenceEvent::Peers(peers, hidden_active) => { /* Devices panel */ }
//!         PresenceEvent::GroupInvite { group, host } => { /* "host wants to test together" */ }
//!         PresenceEvent::GroupStart { starts_in_ms, plan, .. } => {
//!             // engine::start_at(presence.engine_options(Transport::Websocket, plan), starts_in_ms, ...)
//!         }
//!         _ => {}
//!     },
//! );
//! presence.set_status(Some(Phase::Download), Some("Desk".into()));
//! ```
//!
//! - The session ID is generated once per [`PresenceOptions`] (one per page
//!   load) and kept across reconnects, so the server sees one device. Runs
//!   should reuse it ([`PresenceHandle::engine_options`]) so their bytes are
//!   attributed to this device.
//! - Reconnects use exponential backoff (0.5 s doubling to 10 s, reset after a
//!   successful connection). After a reconnect the last status and the hidden
//!   flag are sent again. Group membership does not survive a disconnect (the
//!   server removes a member whose last control socket closes).
//! - Commands ([`create_group`](PresenceHandle::create_group) etc.) are sent
//!   immediately when connected and return `false` (doing nothing) otherwise.
//!   Server refusals arrive as [`PresenceEvent::Error`].

use std::cell::{Cell, RefCell};
use std::pin::pin;
use std::rc::Rc;

use futures::future::{Either, select};
use uuid::Uuid;

use super::EngineOptions;
use super::js::{AbortFlag, EngineError, location_origin, sleep};
use super::socket::Socket;
use super::ws_transport::open;
use crate::model::profile::Profile;
use crate::model::protocol::{
    ClientMsg, DeviceInfo, ErrorCode, GroupState, Peer, Phase, Role, ServerMsg,
};
use crate::model::result::{GroupResult, Transport};

/// First reconnect delay.
const BACKOFF_MIN_MS: u32 = 500;
/// Longest reconnect delay.
const BACKOFF_MAX_MS: u32 = 10_000;

/// What the presence client needs.
#[derive(Debug, Clone)]
pub struct PresenceOptions {
    /// Server origin, e.g. `http://192.168.1.10:8080`. Empty = `location.origin`.
    pub origin: String,
    /// Shared secret sent in `Hello`.
    pub auth: Option<String>,
    pub device: DeviceInfo,
    /// This page's session ID (stable across reconnects).
    pub session: Uuid,
    /// Initial label shown to others.
    pub label: Option<String>,
    /// Initial "Hide me" setting.
    pub hidden: bool,
}

impl PresenceOptions {
    /// Options for the page origin with a fresh session ID.
    pub fn new(device: DeviceInfo) -> Self {
        Self {
            origin: String::new(),
            auth: None,
            device,
            session: Uuid::new_v4(),
            label: None,
            hidden: false,
        }
    }
}

/// Presence and group events, delivered through the [`connect_presence`] callback.
#[derive(Debug, Clone, PartialEq)]
pub enum PresenceEvent {
    /// The control socket is open (also after every reconnect).
    Connected {
        server_name: String,
        server_version: String,
    },
    /// The socket closed or could not be opened; the next attempt is in
    /// `retry_in_ms`. Not emitted after [`PresenceHandle::close`].
    Disconnected { reason: String, retry_in_ms: u32 },
    /// Full snapshot of the other devices (and this one, `is_you`), and the
    /// number of hidden devices currently testing. At most 4 per second.
    Peers(Vec<Peer>, u32),
    /// Another device wants to test together.
    GroupInvite { group: Uuid, host: String },
    /// Membership or state of the group this device is in.
    GroupUpdate {
        group: Uuid,
        host: Uuid,
        members: Vec<Peer>,
        state: GroupState,
    },
    /// Start `plan` in `starts_in_ms` (use [`super::start_at`]).
    GroupStart {
        group: Uuid,
        starts_in_ms: u32,
        plan: Profile,
    },
    /// The server-computed group result.
    GroupResult {
        group: Uuid,
        result: Box<GroupResult>,
    },
    /// The server refused a command (e.g. a non-host `GroupStart`).
    Error { code: ErrorCode, message: String },
}

struct Shared {
    opts: PresenceOptions,
    status: RefCell<(Option<Phase>, Option<String>)>,
    hidden: Cell<bool>,
    socket: RefCell<Option<Rc<Socket>>>,
    closed: AbortFlag,
}

impl Shared {
    fn send(&self, msg: &ClientMsg) -> bool {
        let socket = self.socket.borrow().clone();
        socket.is_some_and(|s| s.send(msg).is_ok())
    }

    /// Re-announces the status and the hidden flag (after a reconnect).
    fn send_state(&self, socket: &Socket) {
        let (phase, label) = self.status.borrow().clone();
        if phase.is_some() || label.is_some() {
            let _ = socket.send(&ClientMsg::Status { phase, label });
        }
        if self.hidden.get() {
            let _ = socket.send(&ClientMsg::SetHidden { hidden: true });
        }
    }
}

/// Controls the presence connection. Cheap to clone. Dropping it does **not**
/// close the connection; call [`close`](Self::close).
#[derive(Clone)]
pub struct PresenceHandle(Rc<Shared>);

impl PresenceHandle {
    /// This page's session ID.
    pub fn session(&self) -> Uuid {
        self.0.opts.session
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.0.opts.device
    }

    pub fn is_connected(&self) -> bool {
        self.0
            .socket
            .borrow()
            .as_ref()
            .is_some_and(|s| !s.is_closed())
    }

    /// Announces the current phase (`None` = idle) and label. Remembered and
    /// re-sent after a reconnect. Returns whether it was sent now.
    pub fn set_status(&self, phase: Option<Phase>, label: Option<String>) -> bool {
        *self.0.status.borrow_mut() = (phase, label.clone());
        self.0.send(&ClientMsg::Status { phase, label })
    }

    /// "Hide me": others don't see this device by name. Remembered.
    pub fn set_hidden(&self, hidden: bool) -> bool {
        self.0.hidden.set(hidden);
        self.0.send(&ClientMsg::SetHidden { hidden })
    }

    /// "Test together": creates a group with this device as host.
    pub fn create_group(&self) -> bool {
        self.0.send(&ClientMsg::GroupCreate)
    }

    pub fn join_group(&self, group: Uuid) -> bool {
        self.0.send(&ClientMsg::GroupJoin { group })
    }

    pub fn leave_group(&self) -> bool {
        self.0.send(&ClientMsg::GroupLeave)
    }

    /// Host only: starts the group test (the server answers with `GroupStart`).
    pub fn start_group(&self) -> bool {
        self.0.send(&ClientMsg::GroupStart)
    }

    /// Engine options for a run of this device: same origin, auth, device and
    /// session ID (so the server attributes the run's traffic to this device),
    /// and the current label.
    pub fn engine_options(&self, transport: Transport, profile: Profile) -> EngineOptions {
        let mut opts = EngineOptions::new(transport, profile, self.0.opts.device.clone());
        opts.origin = self.0.opts.origin.clone();
        opts.auth = self.0.opts.auth.clone();
        opts.session = self.0.opts.session;
        opts.label = self.0.status.borrow().1.clone();
        opts
    }

    /// Closes the connection for good (no reconnect, no further events).
    pub fn close(&self) {
        self.0.closed.abort();
        if let Some(s) = self.0.socket.borrow_mut().take() {
            drop(s);
        }
    }
}

/// Opens the presence connection in the background (`spawn_local`).
pub fn connect_presence(
    opts: PresenceOptions,
    on_event: impl Fn(PresenceEvent) + 'static,
) -> PresenceHandle {
    let shared = Rc::new(Shared {
        status: RefCell::new((None, opts.label.clone())),
        hidden: Cell::new(opts.hidden),
        opts,
        socket: RefCell::new(None),
        closed: AbortFlag::new(),
    });
    wasm_bindgen_futures::spawn_local(run(shared.clone(), Rc::new(on_event)));
    PresenceHandle(shared)
}

fn event_of(msg: ServerMsg) -> Option<PresenceEvent> {
    Some(match msg {
        ServerMsg::Peers {
            peers,
            hidden_active,
        } => PresenceEvent::Peers(peers, hidden_active),
        ServerMsg::GroupInvite { group, host } => PresenceEvent::GroupInvite { group, host },
        ServerMsg::GroupUpdate {
            group,
            host,
            members,
            state,
        } => PresenceEvent::GroupUpdate {
            group,
            host,
            members,
            state,
        },
        ServerMsg::GroupStart {
            group,
            starts_in_ms,
            plan,
        } => PresenceEvent::GroupStart {
            group,
            starts_in_ms,
            plan,
        },
        ServerMsg::GroupResult { group, result } => PresenceEvent::GroupResult {
            group,
            result: Box::new(result),
        },
        ServerMsg::Error { code, message } => PresenceEvent::Error { code, message },
        _ => return None,
    })
}

async fn run(shared: Rc<Shared>, emit: Rc<dyn Fn(PresenceEvent)>) {
    let opts = &shared.opts;
    let mut backoff = BACKOFF_MIN_MS;
    loop {
        if shared.closed.is_aborted() {
            return;
        }
        let origin = if opts.origin.is_empty() {
            location_origin().unwrap_or_default()
        } else {
            opts.origin.trim_end_matches('/').to_string()
        };
        let connect = open(
            &origin,
            "/ws/control",
            opts.session,
            Role::Control,
            opts.auth.clone(),
            Some(opts.device.clone()),
        );
        let reason = match select(pin!(connect), pin!(shared.closed.wait())).await {
            Either::Right(_) => return,
            Either::Left((Err(e), _)) => e.to_string(),
            Either::Left((Ok(socket), _)) => {
                backoff = BACKOFF_MIN_MS;
                let socket = Rc::new(socket);
                *shared.socket.borrow_mut() = Some(socket.clone());
                emit(PresenceEvent::Connected {
                    server_name: socket.welcome.server_name.clone(),
                    server_version: socket.welcome.server_version.clone(),
                });
                shared.send_state(&socket);
                let reason = loop {
                    match select(pin!(socket.recv()), pin!(shared.closed.wait())).await {
                        Either::Right(_) => break None,
                        Either::Left((Err(e), _)) => break Some(e),
                        Either::Left((Ok(m), _)) => {
                            if let Some(ev) = event_of(m.msg) {
                                emit(ev);
                            }
                        }
                    }
                };
                shared.socket.borrow_mut().take();
                drop(socket);
                match reason {
                    None => return,
                    Some(EngineError::Closed(r)) => r,
                    Some(e) => e.to_string(),
                }
            }
        };
        if shared.closed.is_aborted() {
            return;
        }
        emit(PresenceEvent::Disconnected {
            reason,
            retry_in_ms: backoff,
        });
        if let Either::Right(_) = select(pin!(sleep(backoff)), pin!(shared.closed.wait())).await {
            return;
        }
        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
    }
}
