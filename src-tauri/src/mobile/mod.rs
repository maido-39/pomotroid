/// Mobile-only native integrations.
///
/// `timer_service` bridges the Rust timer (local engine or remote follow
/// mode) to the Kotlin `TimerServicePlugin`, which runs a foreground service
/// with a live countdown notification, plays sounds with a native player and
/// posts round-change notifications — all of which keep working while the
/// WebView is paused in the background. On desktop every function is a no-op.
pub mod timer_service;
