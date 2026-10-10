//! Pluggable WebRTC signalling for the packet-loss test (SPEC §5.7).
//!
//! The control socket forwards `rtc_offer` and `ice` messages to an [`RtcSignaling`]
//! implementation (the str0m echo endpoint, added in a later wave). When none is installed
//! in [`crate::server::ws::MeasureState::rtc`], the control socket answers
//! `Error{code: NotFound, message: "packet loss test unavailable"}`.

use async_trait::async_trait;
use uuid::Uuid;

/// Server side of the WebRTC handshake for one session.
///
/// Implementations must be cheap to call concurrently: each call comes from the task of
/// the session's control socket.
#[async_trait]
pub trait RtcSignaling: Send + Sync {
    /// Answers the browser's SDP `offer_sdp` for `session` and returns the answer SDP. The
    /// answer should already carry the server's host candidate(s), so no server-to-client
    /// trickle ICE is needed on a LAN. An `Err` is sent to the client as
    /// `Error{code: Internal, message}`.
    async fn answer(&self, session: Uuid, offer_sdp: String) -> Result<String, String>;

    /// A trickled remote ICE candidate (`candidate:` line) for `session`. The default
    /// implementation ignores it: the server only needs the browser's host candidates,
    /// which are usually in the offer already.
    async fn remote_candidate(
        &self,
        session: Uuid,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_m_line_index: Option<u16>,
    ) -> Result<(), String> {
        let _ = (session, candidate, sdp_mid, sdp_m_line_index);
        Ok(())
    }
}
