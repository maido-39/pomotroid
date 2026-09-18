/// Global keyboard shortcuts. Desktop only; the mobile stub keeps the same
/// signatures so callers need no cfg gating.
#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
pub use desktop::*;

#[cfg(mobile)]
mod stub;
#[cfg(mobile)]
pub use stub::*;
