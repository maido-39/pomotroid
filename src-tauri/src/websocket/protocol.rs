/// Wire format shared by the desktop WebSocket server and the phone-side
/// remote client (and the browser build).
///
/// Every message is a JSON object with a `type` field.
///
/// Client → Server requests carry an optional `id` that the reply echoes so
/// the client can correlate responses:
///
/// | request                          | auth | reply                                   |
/// |----------------------------------|------|-----------------------------------------|
/// | `getState`                       | no   | `state { payload: TimerSnapshot }`      |
/// | `ping`                           | no   | `pong`                                  |
/// | `auth { token }`                 | no   | `authOk { payload: AppInfo }` / `error` |
/// | `toggle` `reset` `skip` `restartRound` | yes | `ok`                              |
/// | `getSettings`                    | yes  | `settings { payload: Settings }`        |
/// | `setSetting { key, value }`      | yes  | `settings { payload: Settings }`        |
/// | `resetSettings`                  | yes  | `settings { payload: Settings }`        |
/// | `getThemes`                      | yes  | `themes { payload: Theme[] }`           |
/// | `getStats { kind }`              | yes  | `stats { kind, payload }`               |
/// | `clearSessions`                  | yes  | `ok`                                    |
/// | `getBackground`                  | yes  | `background { payload: Blob \| null }`  |
/// | `setBackground { name, data_b64 }` | yes | `ok`                                   |
/// | `clearBackground`                | yes  | `ok`                                    |
/// | `getAudio { cue }`               | yes  | `audio { cue, payload: Blob }`          |
/// | `setAudio { cue, name, data_b64 }` | yes | `ok`                                   |
/// | `clearAudio { cue }`             | yes  | `ok`                                    |
/// | `getAudioInfo`                   | yes  | `audioInfo { payload: CustomAudioInfo }`|
///
/// Server → Client broadcasts are `WsEvent` values. Timer events are public;
/// everything else is delivered only to authenticated connections (the
/// settings payload contains the pairing token).
use serde::{Deserialize, Serialize};

use crate::audio::CustomAudioInfo;
use crate::settings::Settings;
use crate::themes::Theme;
use crate::timer::TimerSnapshot;

/// Shared payload for events that carry elapsed time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ElapsedPayload {
    pub elapsed_secs: u32,
}

/// Payload for the `started` event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StartedPayload {
    pub total_secs: u32,
}

/// Payload for the `tick` event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TickPayload {
    pub elapsed_secs: u32,
    pub total_secs: u32,
}

/// Binary payload (image or audio) transported as base64.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobPayload {
    pub name: String,
    pub mime: String,
    pub data_b64: String,
    /// `"custom"`, a preset id, or `"default"` (audio only).
    #[serde(default)]
    pub origin: String,
}

/// Reply to a successful `auth`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub hostname: String,
}

/// Events broadcast to connected WebSocket clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WsEvent {
    Started { payload: StartedPayload },
    RoundChange { payload: TimerSnapshot },
    Paused { payload: ElapsedPayload },
    Resumed { payload: ElapsedPayload },
    Reset { payload: TimerSnapshot },
    Tick { payload: TickPayload },
    SettingsChanged { payload: Settings },
    ThemesChanged { payload: Vec<Theme> },
    BackgroundChanged,
    AudioChanged { payload: String },
    AudioInfoChanged { payload: CustomAudioInfo },
    SessionsChanged,
}

impl WsEvent {
    /// Timer events are safe for unauthenticated clients (stream overlays).
    pub fn is_public(&self) -> bool {
        matches!(
            self,
            WsEvent::Started { .. }
                | WsEvent::RoundChange { .. }
                | WsEvent::Paused { .. }
                | WsEvent::Resumed { .. }
                | WsEvent::Reset { .. }
                | WsEvent::Tick { .. }
        )
    }
}

/// Audio cue identifiers as used on the wire (same as the IPC keys).
pub const CUE_KEYS: &[&str] = &["work_alert", "short_break_alert", "long_break_alert", "tick"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_flag() {
        assert!(WsEvent::Tick { payload: TickPayload { elapsed_secs: 1, total_secs: 2 } }.is_public());
        assert!(!WsEvent::BackgroundChanged.is_public());
        assert!(!WsEvent::SettingsChanged { payload: Settings::default() }.is_public());
    }

    #[test]
    fn event_round_trip() {
        let ev = WsEvent::AudioChanged { payload: "tick".into() };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"type":"audioChanged","payload":"tick"}"#);
        let back: WsEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, WsEvent::AudioChanged { payload } if payload == "tick"));
    }
}
