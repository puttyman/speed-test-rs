//! Small browser helpers: clock, timers, abort flag, errors.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Poll, Waker};

use futures::future::{Either, select};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Performance, Window};

use crate::model::protocol::ErrorCode;

/// Engine error.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// The socket or request could not be opened.
    Connect(String),
    /// The server answered with `Error{code,message}`.
    Server { code: ErrorCode, message: String },
    /// The socket closed during an operation.
    Closed(String),
    /// An operation timed out.
    Timeout(&'static str),
    /// Unexpected data from the server.
    Protocol(String),
    /// A browser API failed.
    Js(String),
    /// The run was aborted.
    Aborted,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Connect(m) => write!(f, "Could not connect to the server: {m}"),
            EngineError::Server {
                code: ErrorCode::Busy,
                message,
            } => write!(f, "Server busy: {message}"),
            EngineError::Server { message, .. } => write!(f, "Server error: {message}"),
            EngineError::Closed(m) => write!(f, "Connection closed: {m}"),
            EngineError::Timeout(what) => write!(f, "Timed out waiting for {what}"),
            EngineError::Protocol(m) => write!(f, "Protocol error: {m}"),
            EngineError::Js(m) => write!(f, "Browser error: {m}"),
            EngineError::Aborted => f.write_str("Aborted"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Converts a JS exception into an [`EngineError::Js`].
pub fn js_err(v: JsValue) -> EngineError {
    let msg = v
        .dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| v.as_string())
        .unwrap_or_else(|| format!("{v:?}"));
    EngineError::Js(msg)
}

pub type Result<T> = std::result::Result<T, EngineError>;

pub fn window() -> Window {
    web_sys::window().expect("the engine runs in a browser window")
}

pub fn performance() -> Performance {
    window().performance().expect("performance is available")
}

/// `performance.now()` in milliseconds.
pub fn now() -> f64 {
    performance().now()
}

/// Resolves after `ms` milliseconds (`setTimeout`).
pub async fn sleep(ms: u32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(
            &resolve,
            i32::try_from(ms).unwrap_or(i32::MAX),
        );
    });
    let _ = JsFuture::from(promise).await;
}

/// Runs `fut`, failing with [`EngineError::Timeout`] after `ms`.
pub async fn timeout<T>(
    ms: u32,
    what: &'static str,
    fut: impl Future<Output = Result<T>>,
) -> Result<T> {
    match select(pin!(fut), pin!(sleep(ms))).await {
        Either::Left((r, _)) => r,
        Either::Right(_) => Err(EngineError::Timeout(what)),
    }
}

#[derive(Default)]
struct AbortInner {
    aborted: Cell<bool>,
    wakers: RefCell<Vec<Waker>>,
}

/// A cloneable abort flag whose [`wait`](Self::wait) future resolves on abort.
#[derive(Clone, Default)]
pub struct AbortFlag(Rc<AbortInner>);

impl AbortFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        if !self.0.aborted.replace(true) {
            for w in self.0.wakers.borrow_mut().drain(..) {
                w.wake();
            }
        }
    }

    pub fn is_aborted(&self) -> bool {
        self.0.aborted.get()
    }

    /// Resolves once [`abort`](Self::abort) has been called.
    pub fn wait(&self) -> impl Future<Output = ()> + '_ {
        std::future::poll_fn(move |cx| {
            if self.0.aborted.get() {
                Poll::Ready(())
            } else {
                let mut wakers = self.0.wakers.borrow_mut();
                if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
                    wakers.push(cx.waker().clone());
                }
                Poll::Pending
            }
        })
    }
}

/// Fills a new `Uint8Array` of `len` bytes with `crypto.getRandomValues`
/// (at most 64 KiB per call, as the API requires).
pub fn random_bytes(len: usize) -> Result<js_sys::Uint8Array> {
    let crypto = window().crypto().map_err(js_err)?;
    let len32 = u32::try_from(len).map_err(|_| EngineError::Js("buffer too large".into()))?;
    let buf = js_sys::Uint8Array::new_with_length(len32);
    let mut off = 0u32;
    while off < len32 {
        let end = (off + 65_536).min(len32);
        crypto
            .get_random_values_with_array_buffer_view(&buf.subarray(off, end))
            .map_err(js_err)?;
        off = end;
    }
    Ok(buf)
}

/// The page origin (`location.origin`).
pub fn location_origin() -> Option<String> {
    window().location().origin().ok()
}
