/// System tray. Real implementation on desktop; no-op stubs with the same
/// public surface on mobile so the timer and settings code need no cfg gating.
#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
pub use desktop::*;

#[cfg(mobile)]
mod stub;
#[cfg(mobile)]
pub use stub::*;
