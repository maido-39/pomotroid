/// WebSocket server for external integrations and paired phones.
///
/// - `protocol`: wire types shared with the remote client (all platforms).
/// - desktop: the axum server (`desktop.rs`).
/// - mobile: no-op stubs with the same signatures so the timer loop and
///   settings commands compile unchanged (`stub.rs`).
pub mod protocol;
pub use protocol::*;

#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
pub use desktop::*;

#[cfg(mobile)]
mod stub;
#[cfg(mobile)]
pub use stub::*;
