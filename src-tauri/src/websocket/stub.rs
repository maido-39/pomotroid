/// Mobile stand-in: the phone is only ever a WebSocket *client* (see
/// `crate::remote`), so the server API is a set of no-ops here.
use std::sync::Arc;

use tauri::AppHandle;

use super::protocol::WsEvent;
use crate::timer::TimerSnapshot;

pub struct WsState;

impl WsState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    pub fn attach(&self, _app: &AppHandle) {}

    pub fn broadcast(&self, _event: WsEvent) {}
}

#[derive(Clone, Debug)]
pub struct StartConfig {
    pub port: u16,
    pub lan: bool,
    pub token: String,
}

pub async fn start(_cfg: StartConfig, _app: AppHandle, _state: &Arc<WsState>) {}
pub async fn stop(_state: &Arc<WsState>) {}

pub fn broadcast_started(_state: &Arc<WsState>, _total_secs: u32) {}
pub fn broadcast_round_change(_state: &Arc<WsState>, _snapshot: TimerSnapshot) {}
pub fn broadcast_paused(_state: &Arc<WsState>, _elapsed_secs: u32) {}
pub fn broadcast_resumed(_state: &Arc<WsState>, _elapsed_secs: u32) {}
pub fn broadcast_reset(_state: &Arc<WsState>, _snapshot: TimerSnapshot) {}
pub fn broadcast_tick(_state: &Arc<WsState>, _elapsed_secs: u32, _total_secs: u32) {}

/// Pairing info is meaningless on the phone.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PairingInfo {
    pub hosts: Vec<String>,
    pub port: u16,
    pub token: String,
    pub url: String,
    pub lan_enabled: bool,
    pub clients: usize,
}

pub fn pairing_info(_app: &AppHandle) -> Result<PairingInfo, String> {
    Err("pairing info is only available on desktop".into())
}

pub fn generate_token() -> String {
    String::new()
}
