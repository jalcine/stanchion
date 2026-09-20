//! Turning a deadlock into an error message.
//!
//! A capability provider runs while the registry that invoked it is locked, on the
//! thread that took the lock. If that provider calls back into the same registry, the
//! lock is not reentrant: the process stops dead, with no error, no stack and nothing
//! in a log. It is the worst failure this crate could hand a binding author, and it is
//! an easy mistake to make — `provider.invoke` looks like an ordinary callback.
//!
//! So each [`Stanchion`](crate::Stanchion) carries an id, and the thread records which
//! one it is currently inside. Re-entering *that* instance is [`Error::Reentrant`].
//! Entering a different one is fine and stays fine: it is a different lock, so it
//! cannot deadlock against the first, and the previous id is restored on the way out.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Error, Result};

/// Hands out the per-instance ids the guard compares. Zero means "none".
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An id no instance has, meaning this thread is not inside a call.
const NONE: u64 = 0;

thread_local! {
    /// The instance this thread is currently inside a call on.
    static ACTIVE: Cell<u64> = const { Cell::new(NONE) };
}

/// Allocates an id for a new instance.
pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Marks this thread as being inside a call, and unmarks it on the way out.
pub(crate) struct CallGuard {
    previous: u64,
}

impl CallGuard {
    /// Claims the thread for `id`, or reports the re-entry that would have hung.
    pub(crate) fn enter(id: u64) -> Result<CallGuard> {
        ACTIVE.with(|active| {
            let previous = active.get();
            if previous == id {
                return Err(Error::Reentrant);
            }
            active.set(id);
            Ok(CallGuard { previous })
        })
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.set(self.previous));
    }
}

/// Holds the guard across an `async` call, one poll at a time.
///
/// A held [`CallGuard`] would be wrong here. The thread that sets the flag is not
/// necessarily the thread that clears it: a multi-threaded runtime is free to move a
/// future between polls, which would leave the flag set on one thread forever and
/// clear a flag the guard never set on another. Both threads would then be wrong
/// about whether they are inside a call.
///
/// Setting the flag for the duration of each *poll* is what actually matches the
/// hazard. A provider only ever runs synchronously inside a poll, on the thread doing
/// the polling, so that is exactly the window where re-entry has to be caught — and
/// between polls no thread is left marked.
/// The inner future is boxed rather than projected through `unsafe`. Nothing else in
/// this project reaches for `unsafe`, and one allocation per plugin call — next to
/// evaluating Lua — is not worth being the exception.
#[cfg(feature = "async")]
pub(crate) struct Guarded<F> {
    id: u64,
    inner: std::pin::Pin<Box<F>>,
}

#[cfg(feature = "async")]
impl<F> Guarded<F> {
    pub(crate) fn new(id: u64, inner: F) -> Self {
        Guarded {
            id,
            inner: Box::pin(inner),
        }
    }
}

#[cfg(feature = "async")]
impl<T, F> std::future::Future for Guarded<F>
where
    F: std::future::Future<Output = Result<T>>,
{
    type Output = Result<T>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // `Pin<Box<_>>` and `u64` are both `Unpin`, so this needs no projection.
        let guarded = self.get_mut();
        let id = guarded.id;

        let previous = ACTIVE.with(|active| active.get());
        if previous == id {
            return std::task::Poll::Ready(Err(Error::Reentrant));
        }
        ACTIVE.with(|active| active.set(id));
        let polled = guarded.inner.as_mut().poll(cx);
        ACTIVE.with(|active| active.set(previous));
        polled
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn re_entering_the_same_instance_is_refused() {
        let id = next_id();
        let outer = CallGuard::enter(id).expect("the first entry");
        assert_eq!(CallGuard::enter(id).err(), Some(Error::Reentrant));
        drop(outer);
        assert!(CallGuard::enter(id).is_ok(), "the thread is usable again");
    }

    #[test]
    fn entering_a_different_instance_is_allowed_and_restores_the_first() {
        let (first, second) = (next_id(), next_id());
        let outer = CallGuard::enter(first).expect("the first entry");
        {
            let _inner = CallGuard::enter(second).expect("a different instance");
            assert_eq!(CallGuard::enter(second).err(), Some(Error::Reentrant));
        }
        // Leaving the inner call must not have cleared the outer one.
        assert_eq!(CallGuard::enter(first).err(), Some(Error::Reentrant));
        drop(outer);
    }

    #[test]
    fn each_thread_tracks_its_own_calls() {
        let id = next_id();
        let _outer = CallGuard::enter(id).expect("the first entry");
        std::thread::spawn(move || {
            CallGuard::enter(id).expect("another thread is not inside this call");
        })
        .join()
        .expect("the thread");
    }
}
