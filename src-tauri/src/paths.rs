//! Where the app keeps its data.
//!
//! Normally the per-user application data directory the OS hands out. A copy
//! that sits next to a folder called `data` is treated as portable and keeps
//! everything — database, background image, custom sounds, themes and logs —
//! inside that folder instead.
//!
//! That keeps a portable copy self-contained, and, just as important, keeps
//! it from sharing settings with an installed copy: running one to reproduce
//! a problem must not rewrite the other's configuration.
//!
//! Every path lookup goes through here. Calling `app_data_dir()` directly
//! would silently opt that one feature out of portable mode.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

/// Folder beside the executable that switches the app to portable mode.
const PORTABLE_DIR: &str = "data";

/// The portable data folder, when the executable sits next to one.
fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join(PORTABLE_DIR);
    dir.is_dir().then_some(dir)
}

/// True when this copy stores its data beside the executable.
pub fn is_portable() -> bool {
    portable_dir().is_some()
}

/// Directory for the database and user content.
pub fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(dir) = portable_dir() {
        return Ok(dir);
    }
    app.path().app_data_dir().map_err(|e| e.to_string())
}

/// Directory for log files.
pub fn log_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(dir) = portable_dir() {
        return Ok(dir.join("logs"));
    }
    app.path().app_log_dir().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_mode_follows_the_data_folder_beside_the_exe() {
        // The real answer depends on how this test binary is laid out, so the
        // useful invariant is that both helpers agree with each other.
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf()));
        let expected = exe_dir.map(|d| d.join(PORTABLE_DIR).is_dir()).unwrap_or(false);
        assert_eq!(is_portable(), expected);
        assert_eq!(portable_dir().is_some(), expected);
    }
}
