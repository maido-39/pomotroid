/// Audio playback.
///
/// `common` holds everything platform-independent (cue definitions, custom
/// file bookkeeping, bundled presets, and sound resolution). The
/// `AudioManager` implementation is chosen per platform:
///   - desktop: rodio playback on a dedicated thread (`desktop.rs`)
///   - mobile:  bridged to the Kotlin `TimerServicePlugin` (`mobile.rs`)
pub mod common;
pub use common::*;

#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
pub use desktop::*;

#[cfg(mobile)]
mod mobile;
#[cfg(mobile)]
pub use mobile::*;
