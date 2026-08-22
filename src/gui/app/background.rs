// ============================================================================
// Module:       gui::app::background
// Description:  A one-shot background read, non-blocking to poll.
//
// Dependencies: crossbeam-channel; std::thread
// ============================================================================

//! Reading something off the UI thread, once, on demand.
//!
//! [`crate::engine::sampler`] is a persistent thread sampling on an
//! interval; this is its lighter sibling for state that is read on
//! demand instead — the Services and Startup views' own lists. The rule
//! is the same one stated there: a call that can block
//! (`EnumServicesStatusExW`, a registry walk) must not run on the paint
//! thread, or the window stops redrawing for exactly as long as the call
//! takes.
//!
//! A [`BackgroundRead`] spawns a thread that runs its work once and sends
//! the result back over a channel of depth one; [`BackgroundRead::poll`]
//! drains it without blocking. There is no persistent thread to manage
//! and nothing to join on drop — the spawned thread exits the moment it
//! sends, whether or not anyone is still listening.

use crossbeam_channel::{bounded, Receiver};

/// One in-flight background read of a `T`.
pub struct BackgroundRead<T> {
    /// Where the result arrives. Depth one: there is only ever one
    /// answer to wait for.
    receiver: Receiver<T>,
}

impl<T: Send + 'static> BackgroundRead<T> {
    /// Spawns `work` on its own thread and returns a handle to its
    /// eventual result.
    ///
    /// `name` is the thread's name, which shows up in this app's own
    /// process list — a thing people will do, on this app most of all.
    ///
    /// A failure to spawn is swallowed rather than surfaced: the
    /// receiver then simply never yields a value, which [`Self::poll`]
    /// reports the same way it reports "still running". A machine that
    /// cannot spawn a thread has larger problems than a stale service
    /// list, and [`crate::engine::Engine::start`] makes the same choice
    /// for the sampler thread.
    #[must_use]
    pub fn spawn(name: &str, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (sender, receiver) = bounded(1);
        let _ = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _ = sender.send(work());
            });
        Self { receiver }
    }

    /// The result, if the read has finished since the last poll.
    #[must_use]
    pub fn poll(&self) -> Option<T> {
        self.receiver.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_background_read_eventually_yields_its_result() {
        let read = BackgroundRead::spawn("test-read", || 42);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = None;
        while Instant::now() < deadline {
            if let Some(value) = read.poll() {
                seen = Some(value);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(seen, Some(42));
    }

    #[test]
    fn polling_before_the_work_finishes_yields_nothing() {
        let (sender, receiver) = bounded::<u32>(1);
        let read = BackgroundRead { receiver };
        assert_eq!(
            read.poll(),
            None,
            "nothing has been sent yet, so this must not block or fabricate a value"
        );
        drop(sender);
    }
}
