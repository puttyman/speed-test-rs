//! Async wrapper around `web_sys::WebSocket`.
//!
//! - `binaryType = "arraybuffer"`.
//! - **Zero-copy**: a binary frame is only counted (`ArrayBuffer.byteLength`)
//!   and timestamped with `performance.now()` inside `onmessage`; its bytes
//!   never enter WASM memory (SPEC §5.2). Binary frames don't wake the reader.
//! - Text frames are parsed into [`ServerMsg`] and queued with their arrival
//!   time, so RTTs aren't inflated by task scheduling.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::task::{Poll, Waker};

use js_sys::ArrayBuffer;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use super::js::{EngineError, Result, js_err, performance, timeout};
use crate::model::protocol::{ClientMsg, Limits, ServerMsg};

/// Queued text messages beyond this are dropped (oldest first), e.g. presence
/// snapshots on a control socket nobody is reading.
const MAX_QUEUE: usize = 1024;

/// A received text message and its `performance.now()` arrival time.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub at: f64,
    pub msg: ServerMsg,
}

#[derive(Default)]
struct Shared {
    open: Cell<bool>,
    closed: Cell<bool>,
    close_reason: RefCell<String>,
    queue: RefCell<VecDeque<Incoming>>,
    waker: RefCell<Option<Waker>>,
    /// Cumulative binary bytes received.
    rx_bytes: Cell<u64>,
    /// Arrival of the first/last binary frame since the last [`Socket::reset_rx_timing`].
    first_rx: Cell<f64>,
    last_rx: Cell<f64>,
    /// Cumulative binary bytes handed to `send()`.
    tx_bytes: Cell<u64>,
}

impl Shared {
    fn wake(&self) {
        if let Some(w) = self.waker.borrow_mut().take() {
            w.wake();
        }
    }
}

/// Server identity from the `Welcome` message.
#[derive(Debug, Clone, PartialEq)]
pub struct Welcome {
    pub server_name: String,
    pub server_version: String,
    pub limits: Limits,
}

/// An open, greeted WebSocket.
pub struct Socket {
    ws: WebSocket,
    shared: Rc<Shared>,
    pub welcome: Welcome,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_open: Closure<dyn FnMut(Event)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
    _on_error: Closure<dyn FnMut(Event)>,
}

impl Socket {
    /// Opens `url`, sends `hello` and waits for `Welcome` (or the server's
    /// `Error`), all within `timeout_ms`.
    pub async fn connect(url: &str, hello: &ClientMsg, timeout_ms: u32) -> Result<Socket> {
        let ws = WebSocket::new(url).map_err(|e| EngineError::Connect(js_err(e).to_string()))?;
        ws.set_binary_type(BinaryType::Arraybuffer);
        let shared = Rc::new(Shared {
            first_rx: Cell::new(f64::NAN),
            last_rx: Cell::new(f64::NAN),
            ..Default::default()
        });

        let on_message = {
            let shared = shared.clone();
            let perf = performance();
            Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
                let t = perf.now();
                let data = ev.data();
                if let Some(buf) = data.dyn_ref::<ArrayBuffer>() {
                    // Hot path: count and timestamp only.
                    let n = u64::from(buf.byte_length());
                    shared.rx_bytes.set(shared.rx_bytes.get() + n);
                    if shared.first_rx.get().is_nan() {
                        shared.first_rx.set(t);
                    }
                    shared.last_rx.set(t);
                } else if let Some(msg) = data
                    .as_string()
                    .and_then(|text| ServerMsg::from_json(&text).ok())
                {
                    let mut q = shared.queue.borrow_mut();
                    if q.len() >= MAX_QUEUE {
                        q.pop_front();
                    }
                    q.push_back(Incoming { at: t, msg });
                    drop(q);
                    shared.wake();
                }
            })
        };
        let on_open = {
            let shared = shared.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                shared.open.set(true);
                shared.wake();
            })
        };
        let on_close = {
            let shared = shared.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |ev: CloseEvent| {
                if !shared.closed.get() {
                    let reason = ev.reason();
                    *shared.close_reason.borrow_mut() = if reason.is_empty() {
                        format!("code {}", ev.code())
                    } else {
                        format!("{reason} (code {})", ev.code())
                    };
                }
                shared.closed.set(true);
                shared.wake();
            })
        };
        let on_error = {
            let shared = shared.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                if !shared.closed.get() {
                    *shared.close_reason.borrow_mut() = "WebSocket error".into();
                }
                shared.closed.set(true);
                shared.wake();
            })
        };
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        let mut socket = Socket {
            ws,
            shared,
            welcome: Welcome {
                server_name: String::new(),
                server_version: String::new(),
                limits: Limits::default(),
            },
            _on_message: on_message,
            _on_open: on_open,
            _on_close: on_close,
            _on_error: on_error,
        };
        let welcome = timeout(timeout_ms, "the server greeting", async {
            socket.wait_open().await?;
            socket.send(hello)?;
            loop {
                match socket.recv().await?.msg {
                    ServerMsg::Welcome {
                        server_version,
                        server_name,
                        limits,
                        ..
                    } => {
                        break Ok(Welcome {
                            server_name,
                            server_version,
                            limits,
                        });
                    }
                    ServerMsg::Error { code, message } => {
                        break Err(EngineError::Server { code, message });
                    }
                    _ => {}
                }
            }
        })
        .await?;
        socket.welcome = welcome;
        Ok(socket)
    }

    async fn wait_open(&self) -> Result<()> {
        std::future::poll_fn(|cx| {
            if self.shared.closed.get() {
                Poll::Ready(Err(EngineError::Connect(
                    self.shared.close_reason.borrow().clone(),
                )))
            } else if self.shared.open.get() {
                Poll::Ready(Ok(()))
            } else {
                *self.shared.waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }

    /// Sends a JSON text frame.
    pub fn send(&self, msg: &ClientMsg) -> Result<()> {
        self.check_open()?;
        self.ws.send_with_str(&msg.to_json()).map_err(js_err)
    }

    /// Sends a binary frame from a JS typed-array view (no copy through WASM).
    pub fn send_view(&self, view: &js_sys::Uint8Array) -> Result<()> {
        self.check_open()?;
        self.ws.send_with_array_buffer_view(view).map_err(js_err)?;
        self.shared
            .tx_bytes
            .set(self.shared.tx_bytes.get() + u64::from(view.byte_length()));
        Ok(())
    }

    fn check_open(&self) -> Result<()> {
        if self.shared.closed.get() {
            Err(EngineError::Closed(
                self.shared.close_reason.borrow().clone(),
            ))
        } else {
            Ok(())
        }
    }

    /// The next text message. Fails once the socket is closed and the queue
    /// is drained. Only one task should wait on a socket at a time.
    pub async fn recv(&self) -> Result<Incoming> {
        std::future::poll_fn(|cx| {
            if let Some(m) = self.shared.queue.borrow_mut().pop_front() {
                return Poll::Ready(Ok(m));
            }
            if self.shared.closed.get() {
                return Poll::Ready(Err(EngineError::Closed(
                    self.shared.close_reason.borrow().clone(),
                )));
            }
            *self.shared.waker.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Bytes queued by `send()` but not yet handed to the network.
    pub fn buffered_amount(&self) -> u32 {
        self.ws.buffered_amount()
    }

    /// Cumulative binary bytes received.
    pub fn rx_bytes(&self) -> u64 {
        self.shared.rx_bytes.get()
    }

    /// Cumulative binary bytes passed to `send()`.
    pub fn tx_bytes(&self) -> u64 {
        self.shared.tx_bytes.get()
    }

    /// Forgets the first/last binary frame times (call before a download).
    pub fn reset_rx_timing(&self) {
        self.shared.first_rx.set(f64::NAN);
        self.shared.last_rx.set(f64::NAN);
    }

    /// Arrival times of the first and last binary frame since the last reset.
    pub fn rx_timing(&self) -> Option<(f64, f64)> {
        let (a, b) = (self.shared.first_rx.get(), self.shared.last_rx.get());
        (!a.is_nan() && !b.is_nan()).then_some((a, b))
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.get()
    }

    /// Discards queued text messages.
    pub fn clear_queue(&self) {
        self.shared.queue.borrow_mut().clear();
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        // Detach the handlers first: the closures are freed with `self`.
        self.ws.set_onmessage(None);
        self.ws.set_onopen(None);
        self.ws.set_onclose(None);
        self.ws.set_onerror(None);
        if !self.shared.closed.get() {
            let _ = self.ws.close_with_code(1000);
        }
    }
}
