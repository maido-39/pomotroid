/// Mobile stand-in: there are no global shortcuts on Android/iOS.
use tauri::AppHandle;

use crate::settings::Settings;

pub fn register_all(_app: &AppHandle, _settings: &Settings) {}
