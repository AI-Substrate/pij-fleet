//! A minimal executor, so a test can await a future without a runtime.
//!
//! `pij-core` must stay tokio-free (workshop 001 R2), and that includes its
//! tests: a `tokio` dev-dependency would put a runtime in the crate's dependency
//! graph and make the "no tokio" claim a matter of which table you read. A few
//! lines of `Waker` plumbing buy the claim outright.
//!
//! This is deliberately NOT a runtime: no timers, no IO reactor, no work
//! stealing. It polls one future to completion on the calling thread, which is
//! all a deterministic fake ever needs. A test that needs more than this is
//! testing an adapter, and adapters get the real runtime.

use std::pin::pin;
use std::task::{Context, Poll, Waker};

/// Poll `future` to completion on this thread.
///
/// # Panics
/// If the future returns `Pending` — which, for a fake, means it awaited real
/// IO. That panic is the point: it says "this fake stopped being a fake" rather
/// than hanging the suite until a wall-clock timeout, which is the failure mode
/// that costs an agent twenty minutes to diagnose.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = pin!(future);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!(
            "block_on: the future returned Pending — nothing in this test substrate should await \
             real IO. A fake that yields is no longer deterministic."
        ),
    }
}
