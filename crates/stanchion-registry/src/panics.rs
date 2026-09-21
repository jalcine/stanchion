//! Turning a Rust panic raised under a plugin call into an ordinary error.
//!
//! A panic in a host callback never unwinds through the interpreter — mlua wraps every
//! callback in `catch_unwind` and carries the payload out as a Lua error, resuming the
//! panic once control is back in Rust. That resumption is the problem this module
//! addresses: without it the panic lands in whatever called `dispatch`, so one bad
//! plugin takes the caller's stack with it, which is the opposite of the per-plugin
//! failure isolation the registry promises everywhere else.
//!
//! Two things this does not do. It cannot help with a process that never unwinds —
//! `os.exit`, an abort, or a segfault in a C rock are signals and exits, not panics;
//! that is what out-of-process hosting is for. And it relies on `panic = "unwind"`: a
//! profile built with `panic = "abort"` ends the process before anything here runs.

use std::any::Any;
use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A plugin call panicked rather than returning an error.
///
/// Reported through [`mlua::Error::external`], so a caller that wants to treat a
/// panicking plugin differently from a failing one can recover it by type rather than
/// by matching on a message:
///
/// ```ignore
/// if let Some(panicked) = error.downcast_ref::<Panicked>() {
///     tracing::error!("{} panicked: {}", outcome.name, panicked.message());
/// }
/// ```
///
/// Use `mlua::Error::downcast_ref`, not a `source()` walk: mlua's `source` skips the
/// external error it holds, while `downcast_ref` descends through the `CallbackError`
/// and `WithContext` layers it adds on the way out.
///
/// Recovering from one is a judgement call. The unwind was caught, but mlua makes no
/// promise about a Lua state's invariants afterwards, so the safe reading is that this
/// plugin is suspect — a good reason to unload it with [`crate::Registry::remove`]
/// rather than to dispatch to it again.
#[derive(Debug, Clone)]
pub struct Panicked {
    message: String,
}

impl Panicked {
    /// The panic's own message, as far as it could be recovered.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Panicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the plugin call panicked: {}", self.message)
    }
}

impl Error for Panicked {}

/// Runs `call`, converting a panic into [`Panicked`].
///
/// The unwind-safety assertion is deliberate: the values a plugin call borrows are the
/// registry's own, and a caught panic leaves them observable. mlua makes no promise
/// about a Lua state's invariants after one, so the honest reading of an `Err` here is
/// that this plugin is suspect, not that nothing happened.
pub(crate) fn guard<R>(call: impl FnOnce() -> R) -> Result<R, Panicked> {
    catch_unwind(AssertUnwindSafe(call)).map_err(|payload| Panicked {
        message: describe(&*payload),
    })
}

/// Awaits `future`, converting a panic during any poll into [`Panicked`].
///
/// The future is boxed so it can be pinned without unsafe pin projection; a panicking
/// poll ends the wait, so the future is never polled again after unwinding.
#[cfg(feature = "async")]
pub(crate) async fn guard_future<F>(future: F) -> Result<F::Output, Panicked>
where
    F: std::future::Future,
{
    use std::task::Poll;

    let mut future = Box::pin(future);
    std::future::poll_fn(move |cx| {
        match guard(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Err(panicked) => Poll::Ready(Err(panicked)),
        }
    })
    .await
}

/// Recovers whatever text a panic payload carries.
///
/// `panic!` with a literal yields `&str` and with arguments yields `String`; anything
/// else came from `panic_any` and has no text to show.
fn describe(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        return (*text).to_string();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "no message".to_string()
}

impl From<Panicked> for mlua::Error {
    fn from(panicked: Panicked) -> Self {
        mlua::Error::external(panicked)
    }
}
