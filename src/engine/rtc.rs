//! WebRTC packet loss probe (SPEC §5.7).
//!
//! 1. Create an `RTCPeerConnection` without STUN/TURN servers (LAN host candidates only)
//!    and an unreliable, unordered data channel (`ordered: false, maxRetransmits: 0`).
//! 2. Create the offer, wait briefly for ICE gathering, and send `RtcOffer{sdp}` on the
//!    run's control socket; apply the server's `RtcAnswer`. The server is ICE-lite and
//!    learns our address from our connectivity checks, so no trickle ICE is needed.
//! 3. Wait for the channel to open (`connection_timeout_ms`); failing that, the probe
//!    errors and packet loss is reported as unavailable.
//! 4. Send `num_packets` 4-byte little-endian sequence numbers in batches of `batch_size`,
//!    `batch_wait_ms` apart. The server echoes each message back.
//! 5. Stop `responses_wait_ms` after the last echo (or after the last send if nothing
//!    came back), or as soon as everything is back, and count the unique echoed seqs.
//!
//! Dropping the probe future (abort) closes the peer connection.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{Poll, Waker};

use js_sys::{Array, ArrayBuffer, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Event, MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelState,
    RtcDataChannelType, RtcIceGatheringState, RtcPeerConnection, RtcPeerConnectionState,
    RtcSdpType, RtcSessionDescriptionInit,
};

use super::js::{EngineError, js_err, now, sleep, timeout};
use super::{LocalBoxFuture, PacketLossContext, PacketLossParams, PacketLossProbe};
use crate::model::protocol::{ClientMsg, ServerMsg};
use crate::model::result::PacketLossCounts;

/// Longest wait for ICE gathering before sending the offer anyway. The server doesn't
/// need our candidates (it is ICE-lite and learns our address from our checks), so this
/// is only a courtesy: Chromium's host-only gathering can take a second or more to report
/// "complete" on hosts with several interfaces.
const GATHER_TIMEOUT_MS: u32 = 300;
/// Granularity of the final wait for late echoes.
const POLL_MS: u32 = 20;
/// Label of the data channel.
const LABEL: &str = "speedtest-loss";

/// The browser's WebRTC packet loss probe.
pub fn packet_loss_probe() -> Rc<dyn PacketLossProbe> {
    Rc::new(WebRtcProbe)
}

struct WebRtcProbe;

impl PacketLossProbe for WebRtcProbe {
    fn run(
        &self,
        ctx: PacketLossContext,
        params: PacketLossParams,
    ) -> LocalBoxFuture<Result<PacketLossCounts, String>> {
        Box::pin(async move {
            run(ctx, params)
                .await
                .map_err(|e| format!("packet loss unavailable: {e}"))
        })
    }
}

#[derive(Default)]
struct Shared {
    open: Cell<bool>,
    failed: RefCell<Option<String>>,
    gathered: Cell<bool>,
    /// `received[seq]` once `seq` has been echoed.
    received: RefCell<Vec<bool>>,
    received_count: Cell<u32>,
    /// `performance.now()` of the last echo; NaN until one arrives.
    last_rx: Cell<f64>,
    waker: RefCell<Option<Waker>>,
}

impl Shared {
    fn wake(&self) {
        if let Some(w) = self.waker.borrow_mut().take() {
            w.wake();
        }
    }

    fn fail(&self, reason: &str) {
        let mut failed = self.failed.borrow_mut();
        if failed.is_none() {
            *failed = Some(reason.to_owned());
        }
        drop(failed);
        self.wake();
    }

    /// Resolves once `ready()` is true, or fails once the connection has failed.
    async fn until(&self, ready: impl Fn(&Shared) -> bool) -> Result<(), EngineError> {
        std::future::poll_fn(|cx| {
            if ready(self) {
                return Poll::Ready(Ok(()));
            }
            if let Some(reason) = self.failed.borrow().clone() {
                return Poll::Ready(Err(EngineError::Connect(reason)));
            }
            *self.waker.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
}

/// The peer connection, its data channel and their event handlers. Dropping it closes
/// both and detaches the handlers.
struct Peer {
    pc: RtcPeerConnection,
    dc: RtcDataChannel,
    shared: Rc<Shared>,
    _on_open: Closure<dyn FnMut(Event)>,
    _on_close: Closure<dyn FnMut(Event)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_gathering: Closure<dyn FnMut(Event)>,
    _on_state: Closure<dyn FnMut(Event)>,
}

impl Peer {
    fn new(num_packets: u32) -> Result<Peer, EngineError> {
        let config = RtcConfiguration::new();
        config.set_ice_servers(&Array::new());
        let pc = RtcPeerConnection::new_with_configuration(&config).map_err(js_err)?;

        let init = RtcDataChannelInit::new();
        init.set_ordered(false);
        init.set_max_retransmits(0);
        let dc = pc.create_data_channel_with_data_channel_dict(LABEL, &init);
        dc.set_binary_type(RtcDataChannelType::Arraybuffer);

        let shared = Rc::new(Shared {
            received: RefCell::new(vec![false; num_packets as usize]),
            last_rx: Cell::new(f64::NAN),
            ..Default::default()
        });

        let on_open = {
            let shared = shared.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                shared.open.set(true);
                shared.wake();
            })
        };
        let on_close = {
            let shared = shared.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| shared.fail("data channel closed"))
        };
        let on_message = {
            let shared = shared.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
                let t = now();
                let Some(buf) = ev.data().dyn_ref::<ArrayBuffer>().cloned() else {
                    return;
                };
                if buf.byte_length() < 4 {
                    return;
                }
                let mut bytes = [0u8; 4];
                Uint8Array::new_with_byte_offset_and_length(&buf, 0, 4).copy_to(&mut bytes);
                let seq = u32::from_le_bytes(bytes) as usize;
                let mut received = shared.received.borrow_mut();
                if let Some(slot) = received.get_mut(seq)
                    && !*slot
                {
                    *slot = true;
                    shared.received_count.set(shared.received_count.get() + 1);
                    shared.last_rx.set(t);
                }
            })
        };
        let on_gathering = {
            let shared = shared.clone();
            let pc = pc.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                if pc.ice_gathering_state() == RtcIceGatheringState::Complete {
                    shared.gathered.set(true);
                    shared.wake();
                }
            })
        };
        let on_state = {
            let shared = shared.clone();
            let pc = pc.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| match pc.connection_state() {
                RtcPeerConnectionState::Failed => shared.fail("WebRTC connection failed"),
                RtcPeerConnectionState::Closed => shared.fail("WebRTC connection closed"),
                _ => {}
            })
        };
        dc.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        dc.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        dc.set_onerror(Some(on_close.as_ref().unchecked_ref()));
        dc.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        pc.set_onicegatheringstatechange(Some(on_gathering.as_ref().unchecked_ref()));
        pc.set_onconnectionstatechange(Some(on_state.as_ref().unchecked_ref()));

        Ok(Peer {
            pc,
            dc,
            shared,
            _on_open: on_open,
            _on_close: on_close,
            _on_message: on_message,
            _on_gathering: on_gathering,
            _on_state: on_state,
        })
    }

    /// Creates the offer and returns its SDP once ICE gathering is complete (or after
    /// [`GATHER_TIMEOUT_MS`]).
    async fn offer(&self) -> Result<String, EngineError> {
        let offer = JsFuture::from(self.pc.create_offer())
            .await
            .map_err(js_err)?;
        let sdp = Reflect::get(&offer, &JsValue::from_str("sdp"))
            .ok()
            .and_then(|v| v.as_string())
            .ok_or_else(|| EngineError::Js("the offer has no SDP".into()))?;
        let init = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        init.set_sdp(&sdp);
        JsFuture::from(self.pc.set_local_description(&init))
            .await
            .map_err(js_err)?;
        if self.pc.ice_gathering_state() == RtcIceGatheringState::Complete {
            self.shared.gathered.set(true);
        }
        let gathered = timeout(GATHER_TIMEOUT_MS, "ICE gathering", async {
            self.shared.until(|s| s.gathered.get()).await
        })
        .await;
        if let Err(e) = gathered {
            log(&format!(
                "packet loss: sending the offer without ICE gathering ({e})"
            ));
        }
        Ok(self.pc.local_description().map(|d| d.sdp()).unwrap_or(sdp))
    }

    async fn accept(&self, answer: &str) -> Result<(), EngineError> {
        let init = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        init.set_sdp(answer);
        JsFuture::from(self.pc.set_remote_description(&init))
            .await
            .map_err(js_err)?;
        Ok(())
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.dc.set_onopen(None);
        self.dc.set_onclose(None);
        self.dc.set_onerror(None);
        self.dc.set_onmessage(None);
        self.pc.set_onicegatheringstatechange(None);
        self.pc.set_onconnectionstatechange(None);
        self.dc.close();
        self.pc.close();
    }
}

fn log(msg: &str) {
    web_sys::console::debug_1(&JsValue::from_str(msg));
}

async fn run(ctx: PacketLossContext, p: PacketLossParams) -> Result<PacketLossCounts, EngineError> {
    let connect_ms = if p.connection_timeout_ms == 0 {
        5000
    } else {
        p.connection_timeout_ms
    };
    let peer = Peer::new(p.num_packets)?;
    let t0 = now();

    // Signalling and connection, within the connection timeout.
    timeout(connect_ms, "the WebRTC connection", async {
        let sdp = peer.offer().await?;
        ctx.control.clear_queue();
        ctx.control.send(&ClientMsg::RtcOffer { sdp })?;
        let answer = loop {
            match ctx.control.recv().await?.msg {
                ServerMsg::RtcAnswer { sdp } => break sdp,
                ServerMsg::Error { code, message } => {
                    return Err(EngineError::Server { code, message });
                }
                _ => {}
            }
        };
        peer.accept(&answer).await?;
        peer.shared.until(|s| s.open.get()).await
    })
    .await?;
    let connected_ms = now() - t0;

    // Send in batches.
    let batch = p.batch_size.max(1);
    let mut sent = 0u32;
    let mut last_send = now();
    while sent < p.num_packets {
        if peer.dc.ready_state() != RtcDataChannelState::Open {
            break;
        }
        let end = (sent + batch).min(p.num_packets);
        while sent < end && peer.dc.send_with_u8_array(&sent.to_le_bytes()).is_ok() {
            sent += 1;
        }
        if sent < end {
            // The channel refused a message (closing, or its buffer is full).
            break;
        }
        last_send = now();
        if sent < p.num_packets && p.batch_wait_ms > 0 {
            sleep(p.batch_wait_ms).await;
        }
    }

    // Wait for stragglers: `responses_wait_ms` after the last echo (or the last send).
    let wait = f64::from(p.responses_wait_ms);
    loop {
        let received = peer.shared.received_count.get();
        if received >= sent {
            break;
        }
        let last_rx = peer.shared.last_rx.get();
        let anchor = if last_rx.is_nan() {
            last_send
        } else {
            last_rx.max(last_send)
        };
        let left = anchor + wait - now();
        if left <= 0.0 {
            break;
        }
        sleep((left.ceil() as u32).clamp(1, POLL_MS)).await;
    }

    let received = peer.shared.received_count.get().min(sent);
    log(&format!(
        "packet loss: connected in {connected_ms:.0} ms, {received}/{sent} echoed, {:.0} ms total",
        now() - t0
    ));
    drop(peer);
    Ok(PacketLossCounts::new(sent, received))
}
