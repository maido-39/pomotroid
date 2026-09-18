/// Mobile stand-in for the desktop tray module: keeps the same types and
/// function signatures, but there is no tray icon to draw.
use std::sync::{Arc, Mutex};

use tauri::AppHandle;

#[derive(Clone)]
pub struct TrayColors {
    pub background: [u8; 4],
    pub focus_round: [u8; 4],
    pub short_round: [u8; 4],
    pub long_round: [u8; 4],
    pub foreground: [u8; 4],
}

impl Default for TrayColors {
    fn default() -> Self {
        Self {
            background: [47, 56, 75, 255],
            focus_round: [226, 93, 96, 255],
            short_round: [53, 188, 174, 255],
            long_round: [89, 174, 209, 255],
            foreground: [255, 255, 255, 255],
        }
    }
}

impl TrayColors {
    pub fn from_colors_map(_colors: &std::collections::HashMap<String, String>) -> Self {
        Self::default()
    }
}

pub struct TrayState {
    pub colors: Mutex<TrayColors>,
    pub countdown_mode: Mutex<bool>,
}

impl TrayState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            colors: Mutex::new(TrayColors::default()),
            countdown_mode: Mutex::new(true),
        })
    }
}

pub fn create_tray(_app: &AppHandle, _state: &Arc<TrayState>) {}
pub fn destroy_tray(_state: &Arc<TrayState>) {}
pub fn update_icon(_state: &Arc<TrayState>, _round_type: &str, _paused: bool, _progress: f32) {}
pub fn update_menu_items(_state: &Arc<TrayState>, _is_running: bool, _is_paused: bool) {}
