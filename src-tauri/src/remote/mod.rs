/// Remote follow mode: this instance (normally the Android app) connects to a
/// desktop Pomotroid over its LAN WebSocket server and mirrors it.
///
/// While connected:
///   - every timer / settings / theme / background / stats command issued by
///     the UI is forwarded to the desktop (`forward`), so the desktop is the
///     single source of truth;
///   - events received from the desktop are re-emitted as the *same* Tauri
///     events the local engine would emit (`timer:tick`, `timer:round-change`,
///     `settings:changed`, …) so the Svelte UI needs no special casing;
///   - alert and tick sounds are fetched from the desktop once (base64 over
///     the socket), cached, and played locally through the platform audio
///     manager; round changes also drive the native foreground service /
///     notification on Android via `crate::mobile::timer_service`.
///
/// Mode switching is manual (Settings → Connect / Disconnect). A dropped
/// connection keeps remote mode and reconnects with exponential backoff.
use std::collections::HashMap;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::audio::{AudioCue, AudioManager};
use crate::background::BackgroundInfo;
use crate::settings::{self, Settings};
use crate::timer::{TimerController, TimerSnapshot};

/// How long a forwarded request may wait for the desktop's reply.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How often an idle session pings the desktop.
const PING_EVERY: Duration = Duration::from_secs(20);
/// A session that has heard nothing for this long is treated as gone. The
/// desktop answers every ping, so a live link is never silent for more than
/// `PING_EVERY`; a phone that changed networks, though, can keep a half-open
/// TCP connection for many minutes, showing "connected" while nothing arrives.
const SILENCE_LIMIT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

/// Snapshot returned by `remote_status` and carried by the `remote:status` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteStatus {
    pub mode: Mode,
    pub state: ConnState,
    pub host: String,
    pub port: u16,
    pub token: String,
    pub desktop_name: String,
    pub error: Option<String>,
}

/// Parsed pairing target (from a QR code or pasted URL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingTarget {
    pub host: String,
    pub port: u16,
    pub token: String,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Inner {
    mode: Mode,
    state: ConnState,
    host: String,
    port: u16,
    token: String,
    desktop_name: String,
    error: Option<String>,
    /// Incremented on every connect/disconnect so stale tasks can bail out.
    generation: u64,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    outbound: Option<mpsc::UnboundedSender<String>>,
    pending: HashMap<u64, SyncSender<Value>>,
    next_id: u64,
    /// Desktop settings as last received (tick/volume decisions for sounds).
    settings: Option<Settings>,
    /// cue key → (bytes, mime) fetched from the desktop.
    audio_cache: HashMap<String, (Vec<u8>, String)>,
    last_snapshot: Option<TimerSnapshot>,
}

pub struct RemoteState {
    inner: Mutex<Inner>,
}

impl RemoteState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                mode: Mode::Local,
                state: ConnState::Disconnected,
                host: String::new(),
                port: 1314,
                token: String::new(),
                desktop_name: String::new(),
                error: None,
                generation: 0,
                task: None,
                outbound: None,
                pending: HashMap::new(),
                next_id: 1,
                settings: None,
                audio_cache: HashMap::new(),
                last_snapshot: None,
            }),
        })
    }

    fn status(&self) -> RemoteStatus {
        let i = self.inner.lock().unwrap();
        RemoteStatus {
            mode: i.mode,
            state: i.state.clone(),
            host: i.host.clone(),
            port: i.port,
            token: i.token.clone(),
            desktop_name: i.desktop_name.clone(),
            error: i.error.clone(),
        }
    }
}

fn state(app: &AppHandle) -> Option<Arc<RemoteState>> {
    app.try_state::<Arc<RemoteState>>().map(|s| Arc::clone(&*s))
}

fn emit_status(app: &AppHandle, st: &Arc<RemoteState>) {
    let _ = app.emit("remote:status", st.status());
}

fn set_state(app: &AppHandle, st: &Arc<RemoteState>, s: ConnState, error: Option<String>) {
    {
        let mut i = st.inner.lock().unwrap();
        i.state = s;
        i.error = error;
    }
    emit_status(app, st);
}

// ---------------------------------------------------------------------------
// Public API used by commands
// ---------------------------------------------------------------------------

/// True when this instance is in remote mode (whether or not currently connected).
pub fn is_active(app: &AppHandle) -> bool {
    state(app)
        .map(|s| s.inner.lock().unwrap().mode == Mode::Remote)
        .unwrap_or(false)
}

pub fn status(app: &AppHandle) -> RemoteStatus {
    match state(app) {
        Some(s) => s.status(),
        None => RemoteStatus {
            mode: Mode::Local,
            state: ConnState::Disconnected,
            host: String::new(),
            port: 1314,
            token: String::new(),
            desktop_name: String::new(),
            error: None,
        },
    }
}

/// If remote mode is active, execute `msg` on the desktop and return the
/// reply's `payload` deserialised as `T`. Returns `None` in local mode so
/// the caller falls through to its local implementation.
pub fn forward<T: DeserializeOwned>(app: &AppHandle, msg: Value) -> Option<Result<T, String>> {
    let st = state(app)?;
    if st.inner.lock().unwrap().mode != Mode::Remote {
        return None;
    }
    Some(request(&st, msg).and_then(|reply| {
        let payload = reply.get("payload").cloned().unwrap_or(Value::Null);
        serde_json::from_value::<T>(payload).map_err(|e| format!("bad reply from desktop: {e}"))
    }))
}

/// Blocking request/response over the socket (call from Tauri's command
/// thread pool or any non-async thread — never from inside the tokio runtime).
fn request(st: &Arc<RemoteState>, mut msg: Value) -> Result<Value, String> {
    let (tx, rx) = sync_channel::<Value>(1);
    let outbound = {
        let mut i = st.inner.lock().unwrap();
        if i.state != ConnState::Connected {
            return Err(match &i.state {
                ConnState::Error => i.error.clone().unwrap_or_else(|| "not connected".into()),
                _ => "not connected to desktop".into(),
            });
        }
        let id = i.next_id;
        i.next_id += 1;
        msg["id"] = json!(id);
        i.pending.insert(id, tx);
        i.outbound.clone().ok_or_else(|| "not connected".to_string())?
    };
    outbound
        .send(msg.to_string())
        .map_err(|_| "connection closed".to_string())?;
    match rx.recv_timeout(REQUEST_TIMEOUT) {
        Ok(reply) => {
            if reply.get("type").and_then(|t| t.as_str()) == Some("error") {
                Err(reply
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("desktop rejected the request")
                    .to_string())
            } else {
                Ok(reply)
            }
        }
        Err(_) => {
            // Drop the waiter so a late reply is ignored.
            if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                st.inner.lock().unwrap().pending.remove(&id);
            }
            Err("desktop did not answer in time".into())
        }
    }
}

/// Fetch the desktop's background image (data URL form for the UI).
pub fn fetch_background(app: &AppHandle) -> Result<Option<BackgroundInfo>, String> {
    let st = state(app).ok_or("remote state missing")?;
    let reply = request(&st, json!({ "type": "getBackground" }))?;
    let payload = reply.get("payload").cloned().unwrap_or(Value::Null);
    if payload.is_null() {
        return Ok(None);
    }
    let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mime = payload.get("mime").and_then(|v| v.as_str()).unwrap_or("image/png");
    let data = payload.get("data_b64").and_then(|v| v.as_str()).unwrap_or("");
    // The picture is the desktop's; how it sits is this device's business. A
    // phone held upright crops nothing like a desktop window, so the framing
    // is looked up here, under a key that cannot collide with a local image.
    let key = format!("remote:{name}");
    let f = crate::commands::framing_for(app, &key)?;
    Ok(Some(BackgroundInfo {
        name,
        data_url: format!("data:{mime};base64,{data}"),
        key,
        pos_x: f.pos_x,
        pos_y: f.pos_y,
        zoom: f.zoom,
    }))
}

/// Play the desktop's current sound for `cue` on this device (Settings preview).
pub fn preview_remote_sound(app: &AppHandle, cue: AudioCue) -> Result<(), String> {
    let st = state(app).ok_or("remote state missing")?;
    let (bytes, _mime) = cached_or_fetch_audio(&st, cue)?;
    let volume = st
        .inner
        .lock()
        .unwrap()
        .settings
        .as_ref()
        .map(|s| s.volume)
        .unwrap_or(1.0)
        .max(0.3);
    play_bytes(app, bytes, volume);
    Ok(())
}

fn cached_or_fetch_audio(st: &Arc<RemoteState>, cue: AudioCue) -> Result<(Vec<u8>, String), String> {
    if let Some(hit) = st.inner.lock().unwrap().audio_cache.get(cue.key()).cloned() {
        return Ok(hit);
    }
    let reply = request(st, json!({ "type": "getAudio", "cue": cue.key() }))?;
    let entry = decode_audio_reply(&reply).ok_or("bad audio reply")?;
    st.inner
        .lock()
        .unwrap()
        .audio_cache
        .insert(cue.key().to_string(), entry.clone());
    Ok(entry)
}

fn decode_audio_reply(reply: &Value) -> Option<(Vec<u8>, String)> {
    use base64::Engine;
    let payload = reply.get("payload")?;
    let mime = payload.get("mime")?.as_str()?.to_string();
    let data = payload.get("data_b64")?.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    Some((bytes, mime))
}

fn play_bytes(app: &AppHandle, bytes: Vec<u8>, volume: f32) {
    if let Some(audio) = app.try_state::<Arc<AudioManager>>() {
        audio.play_bytes(bytes, volume);
    }
}

/// Persisted keys for the last pairing target (phone-local settings table).
const KEY_HOST: &str = "remote_host";
const KEY_PORT: &str = "remote_port";
const KEY_TOKEN: &str = "remote_token";

/// Enter remote mode and start the connection task.
pub fn connect(app: &AppHandle, host: String, port: u16, token: String) -> Result<(), String> {
    let host = host.trim().to_string();
    if host.is_empty() {
        return Err("host is required".into());
    }
    let st = state(app).ok_or("remote state missing")?;

    // Pause the local timer so it does not keep running unseen.
    if let Some(timer) = app.try_state::<TimerController>() {
        timer.pause();
    }
    crate::commands::refresh_ambient(app);

    // Remember the target for next time.
    if let Some(db) = app.try_state::<crate::db::DbState>() {
        if let Ok(conn) = db.lock() {
            let _ = settings::save_setting(&conn, KEY_HOST, &host);
            let _ = settings::save_setting(&conn, KEY_PORT, &port.to_string());
            let _ = settings::save_setting(&conn, KEY_TOKEN, &token);
        }
    }

    let generation = {
        let mut i = st.inner.lock().unwrap();
        if let Some(task) = i.task.take() {
            task.abort();
        }
        i.generation += 1;
        i.mode = Mode::Remote;
        i.state = ConnState::Connecting;
        i.host = host.clone();
        i.port = port;
        i.token = token.clone();
        i.error = None;
        i.desktop_name.clear();
        i.pending.clear();
        i.outbound = None;
        i.audio_cache.clear();
        i.settings = None;
        i.generation
    };
    emit_status(app, &st);

    let app_c = app.clone();
    let st_c = Arc::clone(&st);
    let handle = tauri::async_runtime::spawn(async move {
        connection_loop(app_c, st_c, generation, host, port, token).await;
    });
    st.inner.lock().unwrap().task = Some(handle);
    log::info!("[remote] connecting to {}:{}", status(app).host, port);
    Ok(())
}

/// Leave remote mode; the local timer becomes the source of truth again.
pub fn disconnect(app: &AppHandle) {
    let Some(st) = state(app) else { return };
    {
        let mut i = st.inner.lock().unwrap();
        i.generation += 1;
        if let Some(task) = i.task.take() {
            task.abort();
        }
        i.mode = Mode::Local;
        i.state = ConnState::Disconnected;
        i.outbound = None;
        i.pending.clear();
        i.error = None;
        i.desktop_name.clear();
        i.audio_cache.clear();
        i.settings = None;
        i.last_snapshot = None;
    }
    emit_status(app, &st);
    log::info!("[remote] disconnected — local mode");

    // Re-hydrate the UI from the local engine and settings.
    if let Some(timer) = app.try_state::<TimerController>() {
        let _ = app.emit("timer:reset", timer.get_snapshot());
    }
    if let Ok(s) = crate::commands::settings_get_impl(app) {
        let _ = app.emit("settings:changed", &s);
    }
    if let Ok(themes) = crate::commands::themes_list_impl(app) {
        let _ = app.emit("themes:changed", &themes);
    }
    let _ = app.emit("background:changed", ());
    crate::commands::refresh_ambient(app);
}

/// Last pairing target saved on this device, if any.
pub fn saved_target(app: &AppHandle) -> Option<PairingTarget> {
    let db = app.try_state::<crate::db::DbState>()?;
    let conn = db.lock().ok()?;
    let host = settings::get_setting(&conn, KEY_HOST)?;
    if host.is_empty() {
        return None;
    }
    let port = settings::get_setting(&conn, KEY_PORT)?.parse().ok()?;
    let token = settings::get_setting(&conn, KEY_TOKEN).unwrap_or_default();
    Some(PairingTarget { host, port, token })
}

/// Accepts `http://host:port/?token=…`, `ws://host:port/ws?token=…`,
/// `host:port?token=…`, `host:port`, or a bare host.
pub fn parse_pairing(text: &str) -> Result<PairingTarget, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty pairing code".into());
    }
    let without_scheme = text
        .strip_prefix("http://")
        .or_else(|| text.strip_prefix("https://"))
        .or_else(|| text.strip_prefix("ws://"))
        .or_else(|| text.strip_prefix("wss://"))
        .or_else(|| text.strip_prefix("pomotroid://"))
        .unwrap_or(text);
    let (authority_and_path, query) = match without_scheme.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (without_scheme, None),
    };
    let authority = authority_and_path.split('/').next().unwrap_or("").trim();
    if authority.is_empty() {
        return Err("no host in pairing code".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') || h.starts_with('[') => {
            let port = p.parse::<u16>().map_err(|_| format!("invalid port '{p}'"))?;
            (h.trim_matches(|c| c == '[' || c == ']').to_string(), port)
        }
        _ => (authority.to_string(), 1314),
    };
    let mut token = String::new();
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "token" {
                    token = v.to_string();
                }
            }
        }
    }
    Ok(PairingTarget { host, port, token })
}

// ---------------------------------------------------------------------------
// Connection task
// ---------------------------------------------------------------------------

fn still_current(st: &Arc<RemoteState>, generation: u64) -> bool {
    st.inner.lock().unwrap().generation == generation
}

async fn connection_loop(
    app: AppHandle,
    st: Arc<RemoteState>,
    generation: u64,
    host: String,
    port: u16,
    token: String,
) {
    let url = format!("ws://{host}:{port}/ws");
    let mut backoff = BACKOFF_MIN;
    let mut first = true;

    loop {
        if !still_current(&st, generation) {
            return;
        }
        set_state(&app, &st, if first { ConnState::Connecting } else { ConnState::Reconnecting }, None);

        match tokio_tungstenite::connect_async(&url).await {
            Ok((socket, _)) => {
                backoff = BACKOFF_MIN;
                first = false;
                let reason = run_session(&app, &st, generation, socket, &token).await;
                if !still_current(&st, generation) {
                    return;
                }
                log::warn!("[remote] session ended: {reason}");
                if reason.starts_with("auth") {
                    // Wrong token: do not hammer the desktop.
                    set_state(&app, &st, ConnState::Error, Some(reason));
                    return;
                }
                set_state(&app, &st, ConnState::Reconnecting, Some(reason));
            }
            Err(e) => {
                log::warn!("[remote] connect to {url} failed: {e}");
                first = false;
                set_state(&app, &st, ConnState::Reconnecting, Some(format!("cannot reach {host}:{port}: {e}")));
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Runs one connected session; returns a human-readable end reason.
async fn run_session(
    app: &AppHandle,
    st: &Arc<RemoteState>,
    generation: u64,
    socket: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    token: &str,
) -> String {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    st.inner.lock().unwrap().outbound = Some(out_tx.clone());

    // Authenticate first.
    let _ = out_tx.send(json!({ "type": "auth", "token": token, "id": 0 }).to_string());

    let mut authed = false;
    let mut ping_timer = tokio::time::interval(PING_EVERY);
    ping_timer.tick().await; // first tick is immediate
    let mut last_heard = std::time::Instant::now();

    let reason = loop {
        tokio::select! {
            Some(text) = out_rx.recv() => {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break "send failed".to_string();
                }
            }
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break "connection closed".to_string() };
                last_heard = std::time::Instant::now();
                let Message::Text(text) = msg else { continue };
                let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
                if !still_current(st, generation) {
                    break "superseded".to_string();
                }
                // Route correlated replies.
                if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                    if id == 0 {
                        // auth reply
                        match value.get("type").and_then(|t| t.as_str()) {
                            Some("authOk") => {
                                authed = true;
                                let name = value.get("payload")
                                    .and_then(|p| p.get("hostname"))
                                    .and_then(|h| h.as_str())
                                    .unwrap_or("desktop")
                                    .to_string();
                                st.inner.lock().unwrap().desktop_name = name;
                                set_state(app, st, ConnState::Connected, None);
                                log::info!("[remote] authenticated");
                                // Initial sync (uncorrelated so the generic handler applies them).
                                for t in ["getState", "getSettings", "getThemes"] {
                                    let _ = out_tx.send(json!({ "type": t }).to_string());
                                }
                                for cue in AudioCue::ALL {
                                    let _ = out_tx.send(json!({ "type": "getAudio", "cue": cue.key() }).to_string());
                                }
                                let _ = app.emit("background:changed", ());
                            }
                            _ => break "auth rejected (check the pairing token)".to_string(),
                        }
                        continue;
                    }
                    let waiter = st.inner.lock().unwrap().pending.remove(&id);
                    if let Some(tx) = waiter {
                        let _ = tx.try_send(value);
                        continue;
                    }
                }
                if !authed { continue; }
                handle_event(app, st, &out_tx, value);
            }
            _ = ping_timer.tick() => {
                if last_heard.elapsed() > SILENCE_LIMIT {
                    break format!("no reply from the desktop for {} s", SILENCE_LIMIT.as_secs());
                }
                let _ = out_tx.send(json!({ "type": "ping" }).to_string());
            }
        }
    };

    {
        let mut i = st.inner.lock().unwrap();
        i.outbound = None;
        i.pending.clear();
    }
    reason
}

fn handle_event(app: &AppHandle, st: &Arc<RemoteState>, out_tx: &mpsc::UnboundedSender<String>, value: Value) {
    let kind = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = value.get("payload").cloned().unwrap_or(Value::Null);
    match kind {
        "pong" => {}
        "state" | "reset" => {
            if let Ok(snap) = serde_json::from_value::<TimerSnapshot>(payload.clone()) {
                st.inner.lock().unwrap().last_snapshot = Some(snap.clone());
                crate::mobile::timer_service::update(app, &snap);
            }
            let _ = app.emit("timer:reset", payload);
        }
        "tick" => {
            let elapsed = payload.get("elapsed_secs").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let total = payload.get("total_secs").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let snap_for_service = {
                let mut i = st.inner.lock().unwrap();
                if let Some(s) = i.last_snapshot.as_mut() {
                    s.elapsed_secs = elapsed;
                    s.total_secs = total;
                    s.is_running = true;
                    s.is_paused = false;
                }
                i.last_snapshot.clone()
            };
            let _ = app.emit("timer:tick", payload);
            if let Some(snap) = snap_for_service {
                crate::mobile::timer_service::update(app, &snap);
                // Tick sound according to the desktop's settings.
                let (tick_on, volume) = {
                    let i = st.inner.lock().unwrap();
                    match &i.settings {
                        Some(s) => (
                            if snap.round_type == "work" { s.tick_sounds_during_work } else { s.tick_sounds_during_break },
                            s.volume,
                        ),
                        None => (false, 1.0),
                    }
                };
                if tick_on && volume > 0.0 {
                    if let Some((bytes, _)) = st.inner.lock().unwrap().audio_cache.get("tick").cloned() {
                        play_bytes(app, bytes, volume);
                    }
                }
            }
        }
        "started" => {
            {
                let mut i = st.inner.lock().unwrap();
                if let Some(s) = i.last_snapshot.as_mut() {
                    s.is_running = true;
                    s.is_paused = false;
                }
            }
            let _ = app.emit("timer:started", payload);
        }
        "roundChange" => {
            if let Ok(snap) = serde_json::from_value::<TimerSnapshot>(payload.clone()) {
                st.inner.lock().unwrap().last_snapshot = Some(snap.clone());
                let cue = match snap.round_type.as_str() {
                    "work" => AudioCue::WorkAlert,
                    "short-break" => AudioCue::ShortBreakAlert,
                    _ => AudioCue::LongBreakAlert,
                };
                let (bytes, volume) = {
                    let i = st.inner.lock().unwrap();
                    (
                        i.audio_cache.get(cue.key()).map(|(b, _)| b.clone()),
                        i.settings.as_ref().map(|s| s.volume).unwrap_or(1.0),
                    )
                };
                if let Some(bytes) = bytes {
                    if volume > 0.0 {
                        play_bytes(app, bytes, volume);
                    }
                } else {
                    let _ = out_tx.send(json!({ "type": "getAudio", "cue": cue.key() }).to_string());
                }
                crate::mobile::timer_service::round_changed(app, &snap);
            }
            let _ = app.emit("timer:round-change", payload);
        }
        "paused" => {
            let elapsed = payload.get("elapsed_secs").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let snap = {
                let mut i = st.inner.lock().unwrap();
                if let Some(s) = i.last_snapshot.as_mut() {
                    s.elapsed_secs = elapsed;
                    s.is_running = false;
                    s.is_paused = elapsed > 0;
                }
                i.last_snapshot.clone()
            };
            if let Some(snap) = snap {
                crate::mobile::timer_service::update(app, &snap);
            }
            let _ = app.emit("timer:paused", payload);
        }
        "resumed" => {
            let elapsed = payload.get("elapsed_secs").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let snap = {
                let mut i = st.inner.lock().unwrap();
                if let Some(s) = i.last_snapshot.as_mut() {
                    s.elapsed_secs = elapsed;
                    s.is_running = true;
                    s.is_paused = false;
                }
                i.last_snapshot.clone()
            };
            if let Some(snap) = snap {
                crate::mobile::timer_service::update(app, &snap);
            }
            let _ = app.emit("timer:resumed", payload);
        }
        "settings" | "settingsChanged" => {
            if let Ok(s) = serde_json::from_value::<Settings>(payload.clone()) {
                let changed_sounds = {
                    let mut i = st.inner.lock().unwrap();
                    let changed = i
                        .settings
                        .as_ref()
                        .map(|old| {
                            old.sound_work_alert != s.sound_work_alert
                                || old.sound_short_break_alert != s.sound_short_break_alert
                                || old.sound_long_break_alert != s.sound_long_break_alert
                                || old.sound_tick != s.sound_tick
                        })
                        .unwrap_or(false);
                    i.settings = Some(s);
                    changed
                };
                if changed_sounds {
                    st.inner.lock().unwrap().audio_cache.clear();
                    for cue in AudioCue::ALL {
                        let _ = out_tx.send(json!({ "type": "getAudio", "cue": cue.key() }).to_string());
                    }
                }
            }
            let _ = app.emit("settings:changed", payload);
        }
        "themes" | "themesChanged" => {
            let _ = app.emit("themes:changed", payload);
        }
        "tasksChanged" => {
            let _ = app.emit("tasks:changed", payload);
        }
        "overtime" => {
            let _ = app.emit("timer:overtime", payload);
        }
        "backgroundChanged" => {
            let _ = app.emit("background:changed", ());
        }
        "audioChanged" => {
            let cue = payload.as_str().unwrap_or("").to_string();
            {
                let mut i = st.inner.lock().unwrap();
                if cue == "all" {
                    i.audio_cache.clear();
                } else {
                    i.audio_cache.remove(&cue);
                }
            }
            let cues: Vec<&str> = if cue == "all" {
                AudioCue::ALL.iter().map(|c| c.key()).collect()
            } else {
                vec![cue.as_str()]
            };
            for c in cues {
                if AudioCue::from_key(c).is_some() {
                    let _ = out_tx.send(json!({ "type": "getAudio", "cue": c }).to_string());
                }
            }
            let _ = app.emit("audio:changed", cue);
        }
        "audio" => {
            // Only the known cues are cached, so the cache stays one entry per
            // cue whatever the other end sends.
            let cue = value.get("cue").and_then(|c| c.as_str()).and_then(AudioCue::from_key);
            if let (Some(cue), Some(entry)) = (cue, decode_audio_reply(&value)) {
                st.inner.lock().unwrap().audio_cache.insert(cue.key().to_string(), entry);
            }
        }
        "sessionsChanged" => {
            let _ = app.emit("sessions:changed", ());
        }
        "error" => {
            log::warn!(
                "[remote] desktop error: {}",
                value.get("message").and_then(|m| m.as_str()).unwrap_or("?")
            );
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pairing_urls() {
        let t = parse_pairing("http://192.168.1.20:1314/?token=abc123").unwrap();
        assert_eq!(t, PairingTarget { host: "192.168.1.20".into(), port: 1314, token: "abc123".into() });

        let t = parse_pairing("ws://10.0.2.2:2000/ws?x=1&token=zz").unwrap();
        assert_eq!(t, PairingTarget { host: "10.0.2.2".into(), port: 2000, token: "zz".into() });

        let t = parse_pairing("  desktop.local:1414  ").unwrap();
        assert_eq!(t, PairingTarget { host: "desktop.local".into(), port: 1414, token: String::new() });

        let t = parse_pairing("192.168.0.5").unwrap();
        assert_eq!(t, PairingTarget { host: "192.168.0.5".into(), port: 1314, token: String::new() });

        assert!(parse_pairing("").is_err());
        assert!(parse_pairing("http://host:notaport/").is_err());
    }

    #[test]
    fn audio_reply_decoding() {
        let reply = json!({ "type": "audio", "cue": "tick", "payload": { "mime": "audio/wav", "data_b64": "AQID" } });
        let (bytes, mime) = decode_audio_reply(&reply).unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);
        assert_eq!(mime, "audio/wav");
        assert!(decode_audio_reply(&json!({ "type": "audio" })).is_none());
    }

    #[test]
    fn status_serialises_lowercase() {
        let s = RemoteStatus {
            mode: Mode::Remote,
            state: ConnState::Reconnecting,
            host: "h".into(),
            port: 1,
            token: "t".into(),
            desktop_name: "d".into(),
            error: None,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["mode"], "remote");
        assert_eq!(v["state"], "reconnecting");
    }
}
