//! Waking the UI thread from another thread.

/// Implemented by the window layer (posts one message to the UI thread's
/// hidden window; repeated wakes before it is handled collapse into one).
/// Must be cheap and must never block.
pub trait Wake: Send + Sync {
    fn wake(&self);
}

/// For tests and headless runs.
pub struct NoWake;

impl Wake for NoWake {
    fn wake(&self) {}
}
