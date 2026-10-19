/// Opt-in WebSocket server via tokio + axum (desktop only).
///
/// Binds to `127.0.0.1:{port}` by default. When "Allow phone connections
/// (LAN)" is enabled it binds to `0.0.0.0:{port}` and a pairing token guards
/// everything except the historical read-only `getState` protocol, so stream
/// overlays keep working unchanged.
///
/// Besides `/ws`, the server serves the compiled frontend (the same Svelte UI
/// the desktop uses) so a phone browser can open `http://<ip>:<port>/?token=…`
/// for testing without installing the Android app.
///
/// See `protocol.rs` for the message catalogue.
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path as AxumPath, State as AxumState,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc, watch},
    task::JoinHandle,
};

use super::protocol::{AppInfo, BlobPayload, ElapsedPayload, StartedPayload, TickPayload, WsEvent};
use crate::audio::{AudioCue, AudioSource, CustomAudioInfo};
use crate::commands;
use crate::settings::Settings;
use crate::themes::Theme;
use crate::timer::{TimerController, TimerSnapshot};

// ---------------------------------------------------------------------------
// Backend abstraction (lets the protocol handler be unit-tested without Tauri)
// ---------------------------------------------------------------------------

/// Everything the protocol handler needs from the application.
pub trait Backend: Send + Sync {
    fn snapshot(&self) -> Option<TimerSnapshot>;
    fn timer_command(&self, cmd: &str) -> Result<(), String>;
    fn get_settings(&self) -> Result<Settings, String>;
    fn set_setting(&self, key: &str, value: &str) -> Result<Settings, String>;
    fn reset_settings(&self) -> Result<Settings, String>;
    fn get_themes(&self) -> Result<Vec<Theme>, String>;
    fn save_theme(
        &self,
        name: &str,
        colors: &std::collections::HashMap<String, String>,
    ) -> Result<Vec<Theme>, String>;
    fn delete_theme(&self, name: &str) -> Result<Vec<Theme>, String>;
    fn get_stats(&self, kind: &str) -> Result<Value, String>;
    fn clear_sessions(&self) -> Result<(), String>;
    fn get_background(&self) -> Result<Option<BlobPayload>, String>;
    fn set_background(&self, name: &str, bytes: &[u8]) -> Result<String, String>;
    fn clear_background(&self) -> Result<(), String>;
    /// Advance the folder rotation by one picture.
    fn next_background(&self) -> Result<(), String>;
    fn get_audio(&self, cue: AudioCue) -> Result<AudioSource, String>;
    fn set_audio(&self, cue: AudioCue, name: &str, bytes: &[u8]) -> Result<String, String>;
    fn clear_audio(&self, cue: AudioCue) -> Result<(), String>;
    /// Remove one sound from a cue's pool.
    fn remove_audio(&self, cue: AudioCue, name: &str) -> Result<(), String>;
    fn audio_info(&self) -> Result<CustomAudioInfo, String>;
    /// Tasks, notes and the daily goal; see `commands::focus_impl`.
    fn focus(&self, op: &str, args: &Value) -> Result<Value, String>;
    fn app_info(&self) -> AppInfo;
}

/// Production backend: dispatches into the same implementation functions the
/// Tauri commands use, so a paired phone and the local UI behave identically.
struct AppBackend {
    app: AppHandle,
}

impl Backend for AppBackend {
    fn snapshot(&self) -> Option<TimerSnapshot> {
        self.app.try_state::<TimerController>().map(|t| t.get_snapshot())
    }

    fn timer_command(&self, cmd: &str) -> Result<(), String> {
        let timer = self
            .app
            .try_state::<TimerController>()
            .ok_or_else(|| "timer not available".to_string())?;
        match cmd {
            "toggle" => timer.toggle(),
            "reset" => timer.reset(),
            "skip" => timer.skip(),
            "restartRound" => timer.restart_round(),
            _ => return Err(format!("unknown timer command: {cmd}")),
        }
        Ok(())
    }

    fn get_settings(&self) -> Result<Settings, String> {
        commands::settings_get_impl(&self.app)
    }

    fn set_setting(&self, key: &str, value: &str) -> Result<Settings, String> {
        commands::settings_set_impl(&self.app, key, value)
    }

    fn reset_settings(&self) -> Result<Settings, String> {
        commands::settings_reset_impl(&self.app)
    }

    fn get_themes(&self) -> Result<Vec<Theme>, String> {
        commands::themes_list_impl(&self.app)
    }

    fn save_theme(
        &self,
        name: &str,
        colors: &std::collections::HashMap<String, String>,
    ) -> Result<Vec<Theme>, String> {
        commands::themes_save_impl(&self.app, name, colors)
    }

    fn delete_theme(&self, name: &str) -> Result<Vec<Theme>, String> {
        commands::themes_delete_impl(&self.app, name)
    }

    fn get_stats(&self, kind: &str) -> Result<Value, String> {
        match kind {
            "detailed" => serde_json::to_value(commands::stats_detailed_impl(&self.app)?)
                .map_err(|e| e.to_string()),
            "heatmap" => serde_json::to_value(commands::stats_heatmap_impl(&self.app)?)
                .map_err(|e| e.to_string()),
            _ => Err(format!("unknown stats kind: {kind}")),
        }
    }

    fn clear_sessions(&self) -> Result<(), String> {
        commands::sessions_clear_impl(&self.app)
    }

    fn get_background(&self) -> Result<Option<BlobPayload>, String> {
        Ok(commands::background_raw_impl(&self.app)?.map(|(name, mime, bytes)| BlobPayload {
            name,
            mime: mime.to_string(),
            data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
            origin: "custom".into(),
        }))
    }

    fn set_background(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        commands::background_set_bytes_impl(&self.app, name, bytes)
    }

    fn clear_background(&self) -> Result<(), String> {
        commands::background_clear_impl(&self.app)
    }

    fn next_background(&self) -> Result<(), String> {
        commands::background_next_impl(&self.app)
    }

    fn get_audio(&self, cue: AudioCue) -> Result<AudioSource, String> {
        commands::audio_resolve_impl(&self.app, cue)
    }

    fn set_audio(&self, cue: AudioCue, name: &str, bytes: &[u8]) -> Result<String, String> {
        commands::audio_add_bytes_impl(&self.app, cue, name, bytes)
    }

    fn clear_audio(&self, cue: AudioCue) -> Result<(), String> {
        commands::audio_clear_impl(&self.app, cue)
    }

    fn remove_audio(&self, cue: AudioCue, name: &str) -> Result<(), String> {
        commands::audio_remove_impl(&self.app, cue, name)
    }

    fn audio_info(&self) -> Result<CustomAudioInfo, String> {
        commands::audio_info_impl(&self.app)
    }

    fn focus(&self, op: &str, args: &Value) -> Result<Value, String> {
        commands::focus_impl(&self.app, op, args)
    }

    fn app_info(&self) -> AppInfo {
        AppInfo {
            name: "Pomotroid".into(),
            version: env!("APP_BUILD_VERSION").to_string(),
            hostname: hostname(),
        }
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "desktop".into())
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ServerState {
    broadcast_tx: broadcast::Sender<WsEvent>,
    backend: Arc<dyn Backend>,
    /// Required pairing token; empty = only loopback peers may authenticate.
    token: String,
    app: AppHandle,
    counters: Arc<Counters>,
    /// Becomes `true` when the server that accepted a connection is stopped.
    shutdown: watch::Receiver<bool>,
}

#[derive(Default)]
struct Counters {
    connected: AtomicUsize,
    authed: AtomicUsize,
}

/// One running server: its accept loop and the signal that ends its
/// connections.
///
/// Aborting the accept loop alone is not enough. Axum runs every upgraded
/// WebSocket in a task of its own, so connections outlived the server that
/// accepted them: a regenerated pairing token or a turned-off LAN switch left
/// already-paired phones attached, and each one kept its broadcast receiver and
/// task alive.
struct Running {
    accept: JoinHandle<()>,
    shutdown: watch::Sender<bool>,
}

/// Upper bound on replies queued for one client. A client that stops reading
/// makes the socket write wait, and the reply loop then waits here instead of
/// buffering megabytes of base64 per request.
const DIRECT_QUEUE: usize = 32;

/// Tauri-managed WebSocket state.
pub struct WsState {
    /// Also serialises restarts: two settings changes in quick succession must
    /// not interleave one's stop with the other's start.
    task: tokio::sync::Mutex<Option<Running>>,
    pub broadcast_tx: broadcast::Sender<WsEvent>,
    counters: Arc<Counters>,
}

impl WsState {
    pub fn new() -> Arc<Self> {
        let (broadcast_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            task: tokio::sync::Mutex::new(None),
            broadcast_tx,
            counters: Arc::new(Counters::default()),
        })
    }

    /// Forward app-wide Tauri events to connected (authenticated) clients.
    /// Call once during setup; the listeners live for the app's lifetime.
    pub fn attach(self: &Arc<Self>, app: &AppHandle) {
        let tx = self.broadcast_tx.clone();
        app.listen("settings:changed", move |event| {
            if let Ok(settings) = serde_json::from_str::<Settings>(event.payload()) {
                let _ = tx.send(WsEvent::SettingsChanged { payload: settings });
            }
        });
        let tx = self.broadcast_tx.clone();
        app.listen("themes:changed", move |event| {
            if let Ok(themes) = serde_json::from_str::<Vec<Theme>>(event.payload()) {
                let _ = tx.send(WsEvent::ThemesChanged { payload: themes });
            }
        });
        let tx = self.broadcast_tx.clone();
        app.listen("background:changed", move |_| {
            let _ = tx.send(WsEvent::BackgroundChanged);
        });
        let tx = self.broadcast_tx.clone();
        app.listen("audio:changed", move |event| {
            let cue = serde_json::from_str::<String>(event.payload()).unwrap_or_default();
            let _ = tx.send(WsEvent::AudioChanged { payload: cue });
        });
        let tx = self.broadcast_tx.clone();
        app.listen("sessions:cleared", move |_| {
            let _ = tx.send(WsEvent::SessionsChanged);
        });
        let tx = self.broadcast_tx.clone();
        app.listen("sessions:changed", move |_| {
            let _ = tx.send(WsEvent::SessionsChanged);
        });
        let tx = self.broadcast_tx.clone();
        app.listen("tasks:changed", move |event| {
            if let Ok(tasks) = serde_json::from_str(event.payload()) {
                let _ = tx.send(WsEvent::TasksChanged { payload: tasks });
            }
        });
        let tx = self.broadcast_tx.clone();
        app.listen("timer:overtime", move |event| {
            if let Ok(payload) = serde_json::from_str(event.payload()) {
                let _ = tx.send(WsEvent::Overtime { payload });
            }
        });
    }

    pub fn broadcast(&self, event: WsEvent) {
        let _ = self.broadcast_tx.send(event);
    }

    pub fn client_count(&self) -> (usize, usize) {
        (
            self.counters.connected.load(Ordering::Relaxed),
            self.counters.authed.load(Ordering::Relaxed),
        )
    }
}

#[derive(Clone, Debug)]
pub struct StartConfig {
    pub port: u16,
    /// Bind to all interfaces instead of loopback.
    pub lan: bool,
    pub token: String,
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Stop the running server, if any, and start one from `config()` if it
/// returns a configuration.
///
/// `config` is evaluated while the restart lock is held, so when several
/// restarts queue up the last one to run reads the newest settings; passing a
/// configuration captured at call time would let an older one win the race.
pub async fn restart<F>(app: &AppHandle, state: &Arc<WsState>, config: F)
where
    F: FnOnce() -> Option<StartConfig>,
{
    let mut slot = state.task.lock().await;
    stop_running(&mut slot).await;
    if let Some(cfg) = config() {
        *slot = start(cfg, app.clone(), state).await;
    }
}

/// End a server and every connection it accepted, and wait until its socket is
/// released so the same port can be bound again straight away.
async fn stop_running(slot: &mut Option<Running>) {
    let Some(running) = slot.take() else { return };
    let _ = running.shutdown.send(true);
    running.accept.abort();
    let _ = running.accept.await;
}

/// Bind and spawn the WebSocket/HTTP server.
///
/// Emits `websocket:error` and returns `None` if the port is already in use.
async fn start(cfg: StartConfig, app: AppHandle, state: &Arc<WsState>) -> Option<Running> {
    let ip: IpAddr = if cfg.lan { [0, 0, 0, 0].into() } else { [127, 0, 0, 1].into() };
    let addr = SocketAddr::new(ip, cfg.port);
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("[ws] failed to bind {addr}: {e}");
            let _ = app.emit(
                "websocket:error",
                json!({ "message": e.to_string(), "port": cfg.port }),
            );
            return None;
        }
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server_state = ServerState {
        broadcast_tx: state.broadcast_tx.clone(),
        backend: Arc::new(AppBackend { app: app.clone() }),
        token: cfg.token.clone(),
        app: app.clone(),
        counters: Arc::clone(&state.counters),
        shutdown: shutdown_rx,
    };

    let router = Router::new()
        .route("/ws", get(ws_handler))
        .route("/", get(index_handler))
        .route("/{*path}", get(static_handler))
        .with_state(server_state);

    let handle = tokio::spawn(async move {
        if let Err(e) =
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await
        {
            log::error!("[ws] server error: {e}");
        }
    });

    log::info!(
        "[ws] listening on ws://{}:{}/ws (lan={}, token={})",
        if cfg.lan { "0.0.0.0" } else { "127.0.0.1" },
        cfg.port,
        cfg.lan,
        if cfg.token.is_empty() { "none" } else { "set" }
    );
    Some(Running { accept: handle, shutdown: shutdown_tx })
}

// ---------------------------------------------------------------------------
// Static web UI (browser test client)
// ---------------------------------------------------------------------------

async fn index_handler(AxumState(state): AxumState<ServerState>) -> Response {
    serve_asset(&state.app, "index.html")
}

async fn static_handler(
    AxumState(state): AxumState<ServerState>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    serve_asset(&state.app, &path)
}

fn serve_asset(app: &AppHandle, path: &str) -> Response {
    let path = path.trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };
    match load_asset(app, candidate).or_else(|| {
        // SPA fallback for client-side routes (/settings, /stats).
        if candidate.contains('.') { None } else { load_asset(app, "index.html") }
    }) {
        Some((bytes, mime)) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Load a frontend asset: from the assets embedded in the binary (production)
/// or from the `build/` directory on disk (development, where Tauri serves
/// the UI from the Vite dev server and embeds nothing).
fn load_asset(app: &AppHandle, path: &str) -> Option<(Vec<u8>, String)> {
    let key = format!("/{path}");
    if let Some(asset) = app.asset_resolver().get(key.clone()) {
        return Some((asset.bytes().to_vec(), asset.mime_type().to_string()));
    }
    if tauri::is_dev() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../build");
        let file = root.join(path);
        // Prevent path traversal outside the build dir.
        if let (Ok(root), Ok(file)) = (root.canonicalize(), file.canonicalize()) {
            if file.starts_with(&root) && file.is_file() {
                if let Ok(bytes) = std::fs::read(&file) {
                    return Some((bytes, mime_for(path).to_string()));
                }
            }
        }
    }
    None
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") | Some("webmanifest") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AxumState(state): AxumState<ServerState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, peer, state))
}

/// Per-connection context handed to the protocol handler.
pub struct ConnCtx {
    pub authed: AtomicBool,
    pub peer_is_loopback: bool,
    pub token: String,
}

async fn handle_socket(socket: WebSocket, peer: SocketAddr, state: ServerState) {
    log::debug!("[ws] client connected from {peer}");
    state.counters.connected.fetch_add(1, Ordering::Relaxed);
    emit_clients(&state);

    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcast_tx.subscribe();
    let (direct_tx, mut direct_rx) = mpsc::channel::<String>(DIRECT_QUEUE);

    let ctx = Arc::new(ConnCtx {
        authed: AtomicBool::new(false),
        peer_is_loopback: peer.ip().is_loopback(),
        token: state.token.clone(),
    });

    // Task: forward broadcast events and direct replies to this client.
    let ctx_send = Arc::clone(&ctx);
    let mut send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = rx.recv() => {
                    let event = match result {
                        Ok(e) => e,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            log::debug!("[ws] client lagged {n} events");
                            continue;
                        }
                        Err(_) => break,
                    };
                    if !event.is_public() && !ctx_send.authed.load(Ordering::Relaxed) { continue }
                    let Ok(json) = serde_json::to_string(&event) else { continue };
                    if sender.send(Message::Text(json.into())).await.is_err() { break }
                }
                msg = direct_rx.recv() => {
                    let Some(json) = msg else { break };
                    if sender.send(Message::Text(json.into())).await.is_err() { break }
                }
            }
        }
    });

    // Main loop: handle incoming messages from this client.
    let backend = Arc::clone(&state.backend);
    let ctx_recv = Arc::clone(&ctx);
    let counters = Arc::clone(&state.counters);
    let app_for_emit = state.app.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    let was_authed = ctx_recv.authed.load(Ordering::Relaxed);
                    // Handlers touch the DB and audio; run them off the async executor.
                    let backend = Arc::clone(&backend);
                    let ctx = Arc::clone(&ctx_recv);
                    let text = text.to_string();
                    let reply = tokio::task::spawn_blocking(move || {
                        handle_client_message(&text, backend.as_ref(), &ctx)
                    })
                    .await
                    .unwrap_or(None);
                    if let Some(json) = reply {
                        if direct_tx.send(json).await.is_err() {
                            break;
                        }
                    }
                    if !was_authed && ctx_recv.authed.load(Ordering::Relaxed) {
                        counters.authed.fetch_add(1, Ordering::Relaxed);
                        let _ = app_for_emit.emit(
                            "websocket:clients",
                            json!({
                                "connected": counters.connected.load(Ordering::Relaxed),
                                "authed": counters.authed.load(Ordering::Relaxed),
                            }),
                        );
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // If either task finishes, abort the other; stopping the server ends both.
    // The counters are only ever decremented here, by the connection itself:
    // resetting them on stop let a late disconnect wrap them past zero.
    let mut shutdown = state.shutdown.clone();
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
        _ = shutdown.wait_for(|stopped| *stopped) => {
            send_task.abort();
            recv_task.abort();
            log::debug!("[ws] closing {peer}: server stopped");
        }
    }

    state.counters.connected.fetch_sub(1, Ordering::Relaxed);
    if ctx.authed.load(Ordering::Relaxed) {
        state.counters.authed.fetch_sub(1, Ordering::Relaxed);
    }
    emit_clients(&state);
    log::debug!("[ws] client disconnected {peer}");
}

fn emit_clients(state: &ServerState) {
    let _ = state.app.emit(
        "websocket:clients",
        json!({
            "connected": state.counters.connected.load(Ordering::Relaxed),
            "authed": state.counters.authed.load(Ordering::Relaxed),
        }),
    );
}

// ---------------------------------------------------------------------------
// Protocol handler (pure: no Tauri types, unit-tested below)
// ---------------------------------------------------------------------------

fn reply(kind: &str, id: Option<&Value>, payload: Option<Value>) -> String {
    let mut obj = json!({ "type": kind });
    if let Some(id) = id {
        obj["id"] = id.clone();
    }
    if let Some(p) = payload {
        obj["payload"] = p;
    }
    obj.to_string()
}

fn reply_error(id: Option<&Value>, message: impl Into<String>) -> String {
    let mut obj = json!({ "type": "error", "message": message.into() });
    if let Some(id) = id {
        obj["id"] = id.clone();
    }
    obj.to_string()
}

fn decode_b64(msg: &Value, field: &str) -> Result<Vec<u8>, String> {
    let s = msg
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing '{field}'"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| format!("invalid base64: {e}"))?;
    if bytes.len() > commands::MAX_UPLOAD_BYTES {
        return Err(format!("payload too large (max {} MB)", commands::MAX_UPLOAD_BYTES / 1024 / 1024));
    }
    Ok(bytes)
}

fn str_field<'a>(msg: &'a Value, field: &str) -> Result<&'a str, String> {
    msg.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing '{field}'"))
}

/// Handle one client message. Returns the JSON reply (if any).
pub fn handle_client_message(text: &str, backend: &dyn Backend, ctx: &ConnCtx) -> Option<String> {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let id = msg.get("id");
    let kind = msg.get("type").and_then(|t| t.as_str())?;

    // --- Unauthenticated ---
    match kind {
        "getState" => {
            return backend.snapshot().map(|snap| reply("state", id, Some(json!(snap))));
        }
        "ping" => return Some(reply("pong", id, None)),
        "auth" => {
            let supplied = msg.get("token").and_then(|t| t.as_str()).unwrap_or("");
            let ok = if ctx.token.is_empty() {
                // No token configured (LAN off): only local clients may control.
                ctx.peer_is_loopback
            } else {
                constant_time_eq(supplied.as_bytes(), ctx.token.as_bytes())
            };
            return Some(if ok {
                ctx.authed.store(true, Ordering::Relaxed);
                reply("authOk", id, Some(json!(backend.app_info())))
            } else {
                reply_error(id, "invalid token")
            });
        }
        _ => {}
    }

    if !ctx.authed.load(Ordering::Relaxed) {
        return Some(reply_error(id, "not authenticated"));
    }

    // --- Authenticated ---
    let result: Result<String, String> = (|| {
        Ok(match kind {
            "toggle" | "reset" | "skip" | "restartRound" => {
                backend.timer_command(kind)?;
                reply("ok", id, None)
            }
            "getSettings" => reply("settings", id, Some(json!(backend.get_settings()?))),
            "setSetting" => {
                let key = str_field(&msg, "key")?;
                let value = str_field(&msg, "value")?;
                reply("settings", id, Some(json!(backend.set_setting(key, value)?)))
            }
            "resetSettings" => reply("settings", id, Some(json!(backend.reset_settings()?))),
            "getThemes" => reply("themes", id, Some(json!(backend.get_themes()?))),
            "saveTheme" => {
                let name = str_field(&msg, "name")?;
                let colors: std::collections::HashMap<String, String> = msg
                    .get("colors")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| format!("invalid colors: {e}"))?
                    .ok_or("missing colors")?;
                reply("themes", id, Some(json!(backend.save_theme(name, &colors)?)))
            }
            "deleteTheme" => {
                let name = str_field(&msg, "name")?;
                reply("themes", id, Some(json!(backend.delete_theme(name)?)))
            }
            "getStats" => {
                let kind_s = msg.get("kind").and_then(|k| k.as_str()).unwrap_or("detailed");
                let mut r: Value = serde_json::from_str(&reply("stats", id, Some(backend.get_stats(kind_s)?)))
                    .unwrap_or_default();
                r["kind"] = json!(kind_s);
                r.to_string()
            }
            "clearSessions" => {
                backend.clear_sessions()?;
                reply("ok", id, None)
            }
            "focus" => {
                let op = str_field(&msg, "op")?;
                reply("ok", id, Some(backend.focus(op, &msg)?))
            }
            "getBackground" => reply("background", id, Some(json!(backend.get_background()?))),
            "setBackground" => {
                let name = str_field(&msg, "name")?;
                let bytes = decode_b64(&msg, "data_b64")?;
                let stored = backend.set_background(name, &bytes)?;
                reply("ok", id, Some(json!(stored)))
            }
            "clearBackground" => {
                backend.clear_background()?;
                reply("ok", id, None)
            }
            "nextBackground" => {
                backend.next_background()?;
                reply("ok", id, None)
            }
            "getAudio" => {
                let cue_key = str_field(&msg, "cue")?;
                let cue = AudioCue::from_key(cue_key).ok_or_else(|| format!("unknown cue '{cue_key}'"))?;
                let src = backend.get_audio(cue)?;
                let payload = BlobPayload {
                    name: cue_key.to_string(),
                    mime: src.mime.to_string(),
                    data_b64: base64::engine::general_purpose::STANDARD.encode(&src.bytes),
                    origin: src.origin,
                };
                let mut r: Value = serde_json::from_str(&reply("audio", id, Some(json!(payload))))
                    .unwrap_or_default();
                r["cue"] = json!(cue_key);
                r.to_string()
            }
            "setAudio" => {
                let cue_key = str_field(&msg, "cue")?;
                let cue = AudioCue::from_key(cue_key).ok_or_else(|| format!("unknown cue '{cue_key}'"))?;
                let name = str_field(&msg, "name")?;
                let bytes = decode_b64(&msg, "data_b64")?;
                let stored = backend.set_audio(cue, name, &bytes)?;
                reply("ok", id, Some(json!(stored)))
            }
            "clearAudio" => {
                let cue_key = str_field(&msg, "cue")?;
                let cue = AudioCue::from_key(cue_key).ok_or_else(|| format!("unknown cue '{cue_key}'"))?;
                backend.clear_audio(cue)?;
                reply("ok", id, None)
            }
            "removeAudio" => {
                let cue_key = str_field(&msg, "cue")?;
                let cue = AudioCue::from_key(cue_key).ok_or_else(|| format!("unknown cue '{cue_key}'"))?;
                backend.remove_audio(cue, str_field(&msg, "name")?)?;
                reply("ok", id, None)
            }
            "getAudioInfo" => reply("audioInfo", id, Some(json!(backend.audio_info()?))),
            other => return Err(format!("unknown message type '{other}'")),
        })
    })();

    Some(match result {
        Ok(json) => json,
        Err(e) => reply_error(id, e),
    })
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------------
// Broadcast helpers used by the timer event listener
// ---------------------------------------------------------------------------

pub fn broadcast_started(state: &Arc<WsState>, total_secs: u32) {
    let _ = state.broadcast_tx.send(WsEvent::Started { payload: StartedPayload { total_secs } });
}

pub fn broadcast_round_change(state: &Arc<WsState>, snapshot: TimerSnapshot) {
    let _ = state.broadcast_tx.send(WsEvent::RoundChange { payload: snapshot });
}

pub fn broadcast_paused(state: &Arc<WsState>, elapsed_secs: u32) {
    let _ = state.broadcast_tx.send(WsEvent::Paused { payload: ElapsedPayload { elapsed_secs } });
}

pub fn broadcast_resumed(state: &Arc<WsState>, elapsed_secs: u32) {
    let _ = state.broadcast_tx.send(WsEvent::Resumed { payload: ElapsedPayload { elapsed_secs } });
}

pub fn broadcast_reset(state: &Arc<WsState>, snapshot: TimerSnapshot) {
    let _ = state.broadcast_tx.send(WsEvent::Reset { payload: snapshot });
}

pub fn broadcast_tick(state: &Arc<WsState>, elapsed_secs: u32, total_secs: u32) {
    let _ = state
        .broadcast_tx
        .send(WsEvent::Tick { payload: TickPayload { elapsed_secs, total_secs } });
}

// ---------------------------------------------------------------------------
// Pairing helpers
// ---------------------------------------------------------------------------

/// Pairing details shown in Settings (URL + QR code content).
#[derive(Clone, Debug, serde::Serialize)]
pub struct PairingInfo {
    /// LAN IPv4 addresses of this machine (best guess first).
    pub hosts: Vec<String>,
    pub port: u16,
    pub token: String,
    /// `http://<host>:<port>/?token=<token>` for the first host.
    pub url: String,
    pub lan_enabled: bool,
    pub clients: usize,
}

pub fn pairing_info(app: &AppHandle) -> Result<PairingInfo, String> {
    let settings = commands::settings_get_impl(app)?;
    let mut hosts = local_ipv4_addresses();
    if hosts.is_empty() {
        hosts.push("127.0.0.1".into());
    }
    if !prefer_host(&mut hosts, &settings.websocket_host) {
        log::warn!(
            "[ws] chosen pairing address {} is not on any adapter right now; using {}",
            settings.websocket_host.trim(),
            hosts[0]
        );
    }
    let clients = app
        .try_state::<Arc<WsState>>()
        .map(|s| s.client_count().0)
        .unwrap_or(0);
    let url = format!(
        "http://{}:{}/?token={}",
        hosts[0], settings.websocket_port, settings.websocket_token
    );
    Ok(PairingInfo {
        hosts,
        port: settings.websocket_port,
        token: settings.websocket_token,
        url,
        lan_enabled: settings.websocket_lan_enabled,
        clients,
    })
}

/// Move the chosen pairing address to the front of `hosts`.
///
/// Returns false only when an address was chosen but is not currently on any
/// adapter — unplugged cable, VPN down — in which case the automatic pick
/// stands. An empty choice means "decide automatically" and succeeds.
///
/// Which address leads matters: it is the one written into the QR code, and
/// on a machine carrying VM, VPN and Docker adapters the automatic probe
/// regularly leads with one the phone cannot reach.
fn prefer_host(hosts: &mut [String], chosen: &str) -> bool {
    let chosen = chosen.trim();
    if chosen.is_empty() {
        return true;
    }
    match hosts.iter().position(|h| h == chosen) {
        Some(pos) => {
            hosts.swap(0, pos);
            true
        }
        None => false,
    }
}

/// One usable IPv4 address per network adapter, newest-looking first.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetworkInterface {
    /// Adapter name as the OS reports it ("Wi-Fi", "eth0", ...).
    pub name: String,
    pub address: String,
}

/// Every adapter carrying a usable IPv4 address.
///
/// The automatic probe below can only report *an* address; it cannot say
/// which adapter it belongs to, which is what makes choosing one possible.
pub fn network_interfaces() -> Vec<NetworkInterface> {
    let mut out: Vec<NetworkInterface> = Vec::new();
    let Ok(addrs) = if_addrs::get_if_addrs() else {
        return out;
    };
    for iface in addrs {
        if iface.is_loopback() {
            continue;
        }
        let std::net::IpAddr::V4(v4) = iface.ip() else {
            continue;
        };
        if v4.is_unspecified() || v4.is_link_local() {
            continue;
        }
        let address = v4.to_string();
        if out.iter().any(|i| i.address == address) {
            continue;
        }
        out.push(NetworkInterface { name: iface.name.clone(), address });
    }
    out
}

/// Best-effort discovery of this machine's LAN IPv4 address using the
/// "UDP connect" trick: no packet is sent, the OS just picks the route.
pub fn local_ipv4_addresses() -> Vec<String> {
    let mut out = Vec::new();
    for target in ["192.168.0.1:80", "10.0.0.1:80", "8.8.8.8:80"] {
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(target).is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    let ip = addr.ip();
                    if let IpAddr::V4(v4) = ip {
                        if !v4.is_loopback() && !v4.is_unspecified() {
                            let s = v4.to_string();
                            if !out.contains(&s) {
                                out.push(s);
                            }
                        }
                    }
                }
            }
        }
    }
    // The probe only reports addresses that carry a route, so add the rest of
    // the adapters behind them: one of those may be the chosen one.
    for iface in network_interfaces() {
        if !out.contains(&iface.address) {
            out.push(iface.address);
        }
    }
    out
}

#[cfg(test)]
mod interface_tests {
    use super::*;

    #[test]
    fn interfaces_are_routable_and_named() {
        // Runs against the real machine: the list must only offer addresses a
        // phone could actually be given, so the noise that makes the automatic
        // pick unreliable — loopback, and the 169.254 addresses that VPN,
        // Bluetooth and disconnected adapters carry — is filtered out, and
        // each entry can be told apart by its adapter name.
        for iface in network_interfaces() {
            assert!(!iface.name.is_empty(), "adapter has no name: {iface:?}");
            let ip: std::net::Ipv4Addr = iface
                .address
                .parse()
                .unwrap_or_else(|_| panic!("not an IPv4 address: {iface:?}"));
            assert!(!ip.is_loopback(), "loopback offered for pairing: {iface:?}");
            assert!(!ip.is_link_local(), "link-local offered for pairing: {iface:?}");
            assert!(!ip.is_unspecified(), "unspecified address offered: {iface:?}");
        }
    }

    #[test]
    fn chosen_address_leads_the_list() {
        let mut hosts = vec!["10.8.0.9".to_string(), "192.168.0.100".to_string()];
        assert!(prefer_host(&mut hosts, "192.168.0.100"));
        assert_eq!(hosts[0], "192.168.0.100", "the QR code follows hosts[0]");

        // Empty means automatic: the probe's order is left alone.
        let mut hosts = vec!["10.8.0.9".to_string(), "192.168.0.100".to_string()];
        assert!(prefer_host(&mut hosts, "   "));
        assert_eq!(hosts[0], "10.8.0.9");

        // A chosen adapter that is gone falls back rather than breaking pairing.
        let mut hosts = vec!["10.8.0.9".to_string()];
        assert!(!prefer_host(&mut hosts, "192.168.0.100"));
        assert_eq!(hosts[0], "10.8.0.9");
    }

    #[test]
    fn every_interface_address_is_a_pairing_candidate() {
        // `pairing_info` looks the chosen address up in `local_ipv4_addresses`,
        // so an adapter that can be picked in settings has to appear there or
        // the choice would silently fall back to the automatic one.
        let candidates = local_ipv4_addresses();
        for iface in network_interfaces() {
            assert!(
                candidates.contains(&iface.address),
                "{} is offered but not a pairing candidate",
                iface.address
            );
        }
    }
}

/// 32-hex-character pairing token from the process-seeded hasher (no extra crate).
pub fn generate_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(32);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for i in 0..2u128 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos ^ (i << 64));
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn make_snapshot() -> TimerSnapshot {
        TimerSnapshot {
            round_type: "work".into(),
            previous_round_type: "short-break".into(),
            elapsed_secs: 60,
            total_secs: 1500,
            is_running: true,
            is_paused: false,
            work_round_number: 1,
            work_rounds_total: 4,
            session_work_count: 1,
        }
    }

    /// Records calls; returns canned data.
    #[derive(Default)]
    struct MockBackend {
        calls: Mutex<Vec<String>>,
        settings: Mutex<Settings>,
        no_timer: bool,
    }

    impl Backend for MockBackend {
        fn snapshot(&self) -> Option<TimerSnapshot> {
            if self.no_timer { None } else { Some(make_snapshot()) }
        }
        fn timer_command(&self, cmd: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("timer:{cmd}"));
            Ok(())
        }
        fn get_settings(&self) -> Result<Settings, String> {
            Ok(self.settings.lock().unwrap().clone())
        }
        fn set_setting(&self, key: &str, value: &str) -> Result<Settings, String> {
            self.calls.lock().unwrap().push(format!("set:{key}={value}"));
            let mut s = self.settings.lock().unwrap();
            if key == "sound_tick" {
                s.sound_tick = value.to_string();
            }
            Ok(s.clone())
        }
        fn reset_settings(&self) -> Result<Settings, String> {
            *self.settings.lock().unwrap() = Settings::default();
            Ok(Settings::default())
        }
        fn get_themes(&self) -> Result<Vec<Theme>, String> {
            Ok(vec![Theme { name: "Nord".into(), colors: Default::default(), is_custom: false }])
        }
        fn save_theme(
            &self,
            name: &str,
            colors: &std::collections::HashMap<String, String>,
        ) -> Result<Vec<Theme>, String> {
            self.calls.lock().unwrap().push(format!("theme:save:{name}:{}", colors.len()));
            Ok(Vec::new())
        }
        fn delete_theme(&self, name: &str) -> Result<Vec<Theme>, String> {
            self.calls.lock().unwrap().push(format!("theme:delete:{name}"));
            Ok(Vec::new())
        }
        fn get_stats(&self, kind: &str) -> Result<Value, String> {
            Ok(json!({ "kind": kind, "rounds": 3 }))
        }
        fn clear_sessions(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("clearSessions".into());
            Ok(())
        }
        fn get_background(&self) -> Result<Option<BlobPayload>, String> {
            Ok(None)
        }
        fn set_background(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
            self.calls.lock().unwrap().push(format!("bg:{name}:{}", bytes.len()));
            Ok(name.to_string())
        }
        fn next_background(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("bg:next".to_string());
            Ok(())
        }
        fn clear_background(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("bg:clear".into());
            Ok(())
        }
        fn get_audio(&self, cue: AudioCue) -> Result<AudioSource, String> {
            Ok(AudioSource { bytes: vec![1, 2, 3], mime: "audio/wav", origin: format!("test-{}", cue.key()) })
        }
        fn set_audio(&self, cue: AudioCue, name: &str, bytes: &[u8]) -> Result<String, String> {
            self.calls.lock().unwrap().push(format!("audio:{}:{name}:{}", cue.key(), bytes.len()));
            Ok(name.to_string())
        }
        fn clear_audio(&self, cue: AudioCue) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("audio:clear:{}", cue.key()));
            Ok(())
        }
        fn remove_audio(&self, cue: AudioCue, name: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("audio:remove:{}:{name}", cue.key()));
            Ok(())
        }
        fn audio_info(&self) -> Result<CustomAudioInfo, String> {
            Ok(CustomAudioInfo::default())
        }
        fn focus(&self, op: &str, args: &Value) -> Result<Value, String> {
            self.calls.lock().unwrap().push(format!("focus:{op}"));
            match op {
                "tasks" => Ok(json!([])),
                "setNote" => Ok(json!(!args.get("note").and_then(|n| n.as_str()).unwrap_or("").is_empty())),
                _ => Err(format!("unknown focus operation '{op}'")),
            }
        }
        fn app_info(&self) -> AppInfo {
            AppInfo { name: "Pomotroid".into(), version: "test".into(), hostname: "unit".into() }
        }
    }

    fn ctx(token: &str, loopback: bool) -> ConnCtx {
        ConnCtx { authed: AtomicBool::new(false), peer_is_loopback: loopback, token: token.into() }
    }

    fn parse(reply: Option<String>) -> Value {
        serde_json::from_str(&reply.expect("expected a reply")).unwrap()
    }

    // -- unauthenticated behaviour (backwards compatible) --

    #[test]
    fn getstate_works_without_auth() {
        let b = MockBackend::default();
        let c = ctx("secret", false);
        let v = parse(handle_client_message(r#"{"type":"getState"}"#, &b, &c));
        assert_eq!(v["type"], "state");
        assert_eq!(v["payload"]["elapsed_secs"], 60);
        assert_eq!(v["payload"]["round_type"], "work");
        assert!(v.get("id").is_none());
    }

    #[test]
    fn getstate_no_timer_sends_nothing() {
        let b = MockBackend { no_timer: true, ..Default::default() };
        assert!(handle_client_message(r#"{"type":"getState"}"#, &b, &ctx("", true)).is_none());
    }

    #[test]
    fn malformed_json_is_ignored() {
        let b = MockBackend::default();
        assert!(handle_client_message("not json {{{", &b, &ctx("", true)).is_none());
    }

    #[test]
    fn control_requires_auth() {
        let b = MockBackend::default();
        let c = ctx("secret", false);
        let v = parse(handle_client_message(r#"{"type":"toggle","id":7}"#, &b, &c));
        assert_eq!(v["type"], "error");
        assert_eq!(v["id"], 7);
        assert!(b.calls.lock().unwrap().is_empty());
    }

    // -- auth --

    #[test]
    fn auth_with_correct_token() {
        let b = MockBackend::default();
        let c = ctx("secret", false);
        let v = parse(handle_client_message(r#"{"type":"auth","token":"secret","id":"a1"}"#, &b, &c));
        assert_eq!(v["type"], "authOk");
        assert_eq!(v["id"], "a1");
        assert_eq!(v["payload"]["hostname"], "unit");
        assert!(c.authed.load(Ordering::Relaxed));
    }

    #[test]
    fn auth_with_wrong_token_is_rejected() {
        let b = MockBackend::default();
        let c = ctx("secret", true);
        let v = parse(handle_client_message(r#"{"type":"auth","token":"nope"}"#, &b, &c));
        assert_eq!(v["type"], "error");
        assert!(!c.authed.load(Ordering::Relaxed));
    }

    #[test]
    fn empty_token_allows_only_loopback() {
        let b = MockBackend::default();
        let local = ctx("", true);
        let v = parse(handle_client_message(r#"{"type":"auth","token":""}"#, &b, &local));
        assert_eq!(v["type"], "authOk");

        let remote = ctx("", false);
        let v = parse(handle_client_message(r#"{"type":"auth","token":""}"#, &b, &remote));
        assert_eq!(v["type"], "error");
    }

    // -- authenticated commands --

    fn authed(b: &MockBackend) -> ConnCtx {
        let c = ctx("secret", false);
        handle_client_message(r#"{"type":"auth","token":"secret"}"#, b, &c);
        c
    }

    #[test]
    fn timer_commands_dispatch() {
        let b = MockBackend::default();
        let c = authed(&b);
        for cmd in ["toggle", "reset", "skip", "restartRound"] {
            let v = parse(handle_client_message(&format!(r#"{{"type":"{cmd}","id":1}}"#), &b, &c));
            assert_eq!(v["type"], "ok", "{cmd}");
            assert_eq!(v["id"], 1);
        }
        assert_eq!(
            *b.calls.lock().unwrap(),
            vec!["timer:toggle", "timer:reset", "timer:skip", "timer:restartRound"]
        );
    }

    #[test]
    fn settings_round_trip() {
        let b = MockBackend::default();
        let c = authed(&b);
        let v = parse(handle_client_message(r#"{"type":"getSettings","id":2}"#, &b, &c));
        assert_eq!(v["type"], "settings");
        assert_eq!(v["payload"]["sound_tick"], "default");

        let v = parse(handle_client_message(
            r#"{"type":"setSetting","key":"sound_tick","value":"bell","id":3}"#, &b, &c,
        ));
        assert_eq!(v["type"], "settings");
        assert_eq!(v["payload"]["sound_tick"], "bell");
        assert_eq!(v["id"], 3);

        let v = parse(handle_client_message(r#"{"type":"setSetting","key":"x"}"#, &b, &c));
        assert_eq!(v["type"], "error");
    }

    #[test]
    fn themes_stats_and_sessions() {
        let b = MockBackend::default();
        let c = authed(&b);
        let v = parse(handle_client_message(
            r##"{"type":"saveTheme","name":"Dusk","colors":{"--color-accent":"#ff8800"}}"##,
            &b,
            &c,
        ));
        assert_eq!(v["type"], "themes", "a paired phone can save a theme on the desktop");
        let v = parse(handle_client_message(r#"{"type":"saveTheme","name":"Dusk"}"#, &b, &c));
        assert_eq!(v["type"], "error", "colors are required");
        let v = parse(handle_client_message(r#"{"type":"deleteTheme","name":"Dusk"}"#, &b, &c));
        assert_eq!(v["type"], "themes");
        let v = parse(handle_client_message(r#"{"type":"getThemes"}"#, &b, &c));
        assert_eq!(v["payload"][0]["name"], "Nord");

        let v = parse(handle_client_message(r#"{"type":"getStats","kind":"heatmap","id":9}"#, &b, &c));
        assert_eq!(v["type"], "stats");
        assert_eq!(v["kind"], "heatmap");
        assert_eq!(v["payload"]["rounds"], 3);
        assert_eq!(v["id"], 9);

        let v = parse(handle_client_message(r#"{"type":"clearSessions"}"#, &b, &c));
        assert_eq!(v["type"], "ok");
        assert!(b.calls.lock().unwrap().contains(&"clearSessions".to_string()));
    }

    #[test]
    fn audio_and_background_blobs() {
        let b = MockBackend::default();
        let c = authed(&b);
        let v = parse(handle_client_message(r#"{"type":"getAudio","cue":"tick"}"#, &b, &c));
        assert_eq!(v["type"], "audio");
        assert_eq!(v["cue"], "tick");
        assert_eq!(v["payload"]["mime"], "audio/wav");
        assert_eq!(v["payload"]["origin"], "test-tick");
        assert_eq!(v["payload"]["data_b64"], "AQID");

        let v = parse(handle_client_message(r#"{"type":"getAudio","cue":"bogus"}"#, &b, &c));
        assert_eq!(v["type"], "error");

        let v = parse(handle_client_message(
            r#"{"type":"setAudio","cue":"work_alert","name":"a.wav","data_b64":"AQID"}"#, &b, &c,
        ));
        assert_eq!(v["type"], "ok");
        assert_eq!(v["payload"], "a.wav");

        let v = parse(handle_client_message(
            r#"{"type":"setBackground","name":"bg.png","data_b64":"!!!"}"#, &b, &c,
        ));
        assert_eq!(v["type"], "error", "invalid base64 must error");

        let v = parse(handle_client_message(
            r#"{"type":"setBackground","name":"bg.png","data_b64":"AQID"}"#, &b, &c,
        ));
        assert_eq!(v["type"], "ok");
        let v = parse(handle_client_message(r#"{"type":"clearBackground"}"#, &b, &c));
        assert_eq!(v["type"], "ok");
        let v = parse(handle_client_message(r#"{"type":"nextBackground"}"#, &b, &c));
        assert_eq!(v["type"], "ok");
        let v = parse(handle_client_message(r#"{"type":"getBackground"}"#, &b, &c));
        assert!(v["payload"].is_null());

        let calls = b.calls.lock().unwrap();
        assert!(calls.contains(&"audio:work_alert:a.wav:3".to_string()));
        assert!(calls.contains(&"bg:next".to_string()), "a phone can advance the rotation");
        assert!(calls.contains(&"bg:bg.png:3".to_string()));
        assert!(calls.contains(&"bg:clear".to_string()));
    }

    #[test]
    fn unknown_type_after_auth_errors() {
        let b = MockBackend::default();
        let c = authed(&b);
        let v = parse(handle_client_message(r#"{"type":"unknownCommand","id":5}"#, &b, &c));
        assert_eq!(v["type"], "error");
        assert_eq!(v["id"], 5);
    }

    #[test]
    fn ping_pong() {
        let b = MockBackend::default();
        let v = parse(handle_client_message(r#"{"type":"ping","id":"p"}"#, &b, &ctx("t", false)));
        assert_eq!(v["type"], "pong");
        assert_eq!(v["id"], "p");
    }

    #[test]
    fn token_generation() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(constant_time_eq(a.as_bytes(), a.as_bytes()));
        assert!(!constant_time_eq(a.as_bytes(), b.as_bytes()));
    }

    #[test]
    fn ws_event_serializes_correctly() {
        let event = WsEvent::RoundChange { payload: make_snapshot() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"roundChange\""));
        assert!(json.contains("\"elapsed_secs\":60"));
        let event = WsEvent::Reset { payload: make_snapshot() };
        assert!(serde_json::to_string(&event).unwrap().starts_with(r#"{"type":"reset","payload""#));
    }

    // -- network-level integration test --

    #[tokio::test]
    async fn integration_auth_and_toggle_round_trip() {
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as TungMessage;

        // Minimal server with a mock backend and no Tauri app: reuse the
        // protocol handler through a tiny standalone router.
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::default());
        let (broadcast_tx, _) = broadcast::channel::<WsEvent>(8);
        let tx_for_handler = broadcast_tx.clone();
        let router = Router::new().route(
            "/ws",
            get(move |ws: WebSocketUpgrade, ConnectInfo(peer): ConnectInfo<SocketAddr>| {
                let backend = Arc::clone(&backend);
                let tx = tx_for_handler.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        let (mut sender, mut receiver) = socket.split();
                        let mut rx = tx.subscribe();
                        let ctx = ConnCtx {
                            authed: AtomicBool::new(false),
                            peer_is_loopback: peer.ip().is_loopback(),
                            token: "secret".into(),
                        };
                        loop {
                            tokio::select! {
                                Some(Ok(msg)) = receiver.next() => {
                                    if let Message::Text(text) = msg {
                                        if let Some(r) = handle_client_message(&text, backend.as_ref(), &ctx) {
                                            let _ = sender.send(Message::Text(r.into())).await;
                                        }
                                    }
                                }
                                Ok(ev) = rx.recv() => {
                                    if ev.is_public() || ctx.authed.load(Ordering::Relaxed) {
                                        let _ = sender.send(Message::Text(serde_json::to_string(&ev).unwrap().into())).await;
                                    }
                                }
                                else => break,
                            }
                        }
                    })
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });

        let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();

        // Unauthenticated getState still works.
        ws.send(TungMessage::Text(r#"{"type":"getState"}"#.into())).await.unwrap();
        let TungMessage::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("text") };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "state");

        // Private broadcast is withheld before auth, public one delivered.
        broadcast_tx.send(WsEvent::BackgroundChanged).unwrap();
        broadcast_tx.send(WsEvent::Tick { payload: TickPayload { elapsed_secs: 5, total_secs: 10 } }).unwrap();
        let TungMessage::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("text") };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "tick", "private event must be skipped before auth");

        // Auth then toggle.
        ws.send(TungMessage::Text(r#"{"type":"auth","token":"secret","id":1}"#.into())).await.unwrap();
        let TungMessage::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("text") };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "authOk");

        ws.send(TungMessage::Text(r#"{"type":"toggle","id":2}"#.into())).await.unwrap();
        let TungMessage::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("text") };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "ok");
        assert_eq!(v["id"], 2);

        // Private broadcasts now arrive.
        broadcast_tx.send(WsEvent::BackgroundChanged).unwrap();
        let TungMessage::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("text") };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "backgroundChanged");
    }

    #[test]
    fn focus_requests_reach_the_backend_with_their_arguments() {
        let b = MockBackend::default();
        let c = ctx("t", false);
        handle_client_message(r#"{"type":"auth","token":"t","id":0}"#, &b, &c);
        let v = parse(handle_client_message(r#"{"type":"focus","op":"setNote","note":"wrote the intro","id":3}"#, &b, &c));
        assert_eq!(v["type"], "ok");
        assert_eq!(v["id"], 3);
        assert_eq!(v["payload"], true);
        let e = parse(handle_client_message(r#"{"type":"focus","op":"nope","id":4}"#, &b, &c));
        assert_eq!(e["type"], "error");
        let missing = parse(handle_client_message(r#"{"type":"focus","id":5}"#, &b, &c));
        assert_eq!(missing["type"], "error", "an op is required");
        assert!(b.calls.lock().unwrap().contains(&"focus:setNote".to_string()));
    }

    #[test]
    fn focus_requests_need_authentication() {
        let b = MockBackend::default();
        let c = ctx("t", false);
        let v = parse(handle_client_message(r#"{"type":"focus","op":"tasks","id":1}"#, &b, &c));
        assert_eq!(v["type"], "error");
        assert!(b.calls.lock().unwrap().is_empty());
    }
}
