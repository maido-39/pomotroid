use rusqlite::{Connection, Result};

/// Full schema for version 1. Tables use IF NOT EXISTS so the batch is
/// idempotent, but the schema_version check in `run()` prevents re-execution.
const MIGRATION_1: &str = "
    CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS sessions (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        started_at    INTEGER NOT NULL,
        ended_at      INTEGER,
        round_type    TEXT NOT NULL CHECK(round_type IN ('work', 'short-break', 'long-break')),
        duration_secs INTEGER NOT NULL CHECK(duration_secs > 0),
        completed     INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0, 1))
    );

    CREATE TABLE IF NOT EXISTS custom_themes (
        id     INTEGER PRIMARY KEY AUTOINCREMENT,
        name   TEXT NOT NULL UNIQUE,
        colors TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_sessions_started_at ON sessions(started_at);
    CREATE INDEX IF NOT EXISTS idx_sessions_round_type ON sessions(round_type);

    INSERT INTO schema_version VALUES (1);
";

/// Migrates timer duration storage from minute-resolution keys to second-resolution keys.
/// Reads existing `time_*_mins` rows, multiplies by 60, writes `time_*_secs`, then deletes
/// the old keys so key names align with the Settings struct field names.
const MIGRATION_2: &str = "
    INSERT OR IGNORE INTO settings (key, value)
        SELECT 'time_work_secs', CAST(CAST(value AS INTEGER) * 60 AS TEXT)
          FROM settings WHERE key = 'time_work_mins';
    INSERT OR IGNORE INTO settings (key, value)
        SELECT 'time_short_break_secs', CAST(CAST(value AS INTEGER) * 60 AS TEXT)
          FROM settings WHERE key = 'time_short_break_mins';
    INSERT OR IGNORE INTO settings (key, value)
        SELECT 'time_long_break_secs', CAST(CAST(value AS INTEGER) * 60 AS TEXT)
          FROM settings WHERE key = 'time_long_break_mins';
    DELETE FROM settings WHERE key IN
        ('time_work_mins', 'time_short_break_mins', 'time_long_break_mins');
    INSERT INTO schema_version VALUES (2);
";

/// Seeds the `check_for_updates` setting for users upgrading from a version
/// that did not have this setting. Fresh installs get it via seed_defaults.
const MIGRATION_3: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('check_for_updates', 'true');
    INSERT INTO schema_version VALUES (3);
";

/// Seeds the `global_shortcuts_enabled` setting for all installs. Defaults to
/// 'false' — global shortcuts are now opt-in. This is a breaking change for
/// existing users who relied on shortcuts being active by default; they must
/// re-enable them in Settings → Shortcuts.
const MIGRATION_4: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('global_shortcuts_enabled', 'false');
    INSERT INTO schema_version VALUES (4);
";

/// Seeds the `short_breaks_enabled` and `long_breaks_enabled` settings for
/// users upgrading from a version that did not have these settings.
/// Both default to 'true' — existing behaviour is preserved.
const MIGRATION_5: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('short_breaks_enabled', 'true');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('long_breaks_enabled', 'true');
    INSERT INTO schema_version VALUES (5);
";

/// Seeds the seven local shortcut key bindings for users upgrading from a version
/// that did not have this feature. These shortcuts are handled entirely by the frontend
/// (keydown listeners) and require no Rust-side dispatch logic.
const MIGRATION_6: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('local_shortcut_toggle', ' ');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('local_shortcut_reset', 'ArrowLeft');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('local_shortcut_skip', 'ArrowRight');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('local_shortcut_volume_down', 'ArrowDown');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('local_shortcut_volume_up', 'ArrowUp');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('local_shortcut_mute', 'm');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('local_shortcut_fullscreen', 'F11');
    INSERT INTO schema_version VALUES (6);
";

/// Seeds the settings added for sound presets, the custom background image
/// and LAN access to the WebSocket server. `INSERT OR IGNORE` keeps any value
/// a user already has.
const MIGRATION_7: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('sound_work_alert', 'default');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('sound_short_break_alert', 'default');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('sound_long_break_alert', 'default');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('sound_tick', 'default');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_enabled', 'false');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_opacity', '60');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_blur', '8');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_brightness', '70');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_name', '');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('websocket_lan_enabled', 'false');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('websocket_token', '');
    INSERT INTO schema_version VALUES (7);
";

/// Seeds the framing settings for the background image. Existing users keep
/// the centred, unzoomed framing they have been looking at.
const MIGRATION_8: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_pos_x', '50');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_pos_y', '50');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_image_zoom', '100');
    INSERT INTO schema_version VALUES (8);
";

/// Seeds the launch-at-login setting. Off, so upgrading never starts adding
/// the app to a user's login items behind their back.
const MIGRATION_9: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('autostart_enabled', 'false');
    INSERT INTO schema_version VALUES (9);
";

/// Seeds the window opacity setting. Fully opaque, so nobody's window turns
/// translucent from an upgrade alone.
const MIGRATION_10: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('window_opacity', '100');
    INSERT INTO schema_version VALUES (10);
";

/// Seeds the pairing address setting. Empty keeps the automatic pick.
const MIGRATION_11: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('websocket_host', '');
    INSERT INTO schema_version VALUES (11);
";

/// Gives each background image its own framing.
///
/// One framing for every image only works while there is one image. Cycling
/// through a folder puts a portrait photo and a landscape one behind the same
/// two numbers, and whichever was not framed is wrong. Framing therefore
/// leaves the settings table for a row per image, keyed by how the image is
/// identified: the fixed stem for the single stored image, the file's path for
/// one out of a folder. An image with no row is framed automatically, which is
/// what the centred, unzoomed values mean.
///
/// The framing the user already adjusted becomes the row for the stored image,
/// so upgrading does not silently re-centre the picture they set up.
const MIGRATION_12: &str = "
    CREATE TABLE IF NOT EXISTS background_framing (
        key   TEXT PRIMARY KEY,
        pos_x INTEGER NOT NULL,
        pos_y INTEGER NOT NULL,
        zoom  INTEGER NOT NULL
    );
    INSERT OR IGNORE INTO background_framing (key, pos_x, pos_y, zoom)
    SELECT 'custom_background',
           COALESCE((SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'bg_image_pos_x'), 50),
           COALESCE((SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'bg_image_pos_y'), 50),
           COALESCE((SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'bg_image_zoom'), 100);
    INSERT INTO schema_version VALUES (12);
";

/// Seeds the background folder rotation.
///
/// No folder, so an upgrade shows exactly the picture it showed before. The
/// rotation defaults describe what someone who *does* pick a folder almost
/// certainly wants: a shuffled order, a new picture when the round changes,
/// and no clock-driven switching on top of it.
const MIGRATION_13: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_folder', '');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_shuffle', 'true');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_rotate_on_round', 'true');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('bg_rotate_mins', '0');
    INSERT INTO schema_version VALUES (13);
";

/// Seeds the clock style.
///
/// The ring everyone has been looking at, so an upgrade changes nothing about
/// how the timer looks. The other styles exist because the ring is the one
/// shape that fights a background image: it draws a hard circle across the
/// middle of the picture.
const MIGRATION_14: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('clock_style', 'dial');
    INSERT INTO schema_version VALUES (14);
";

/// Seeds the font choices.
///
/// Empty means the bundled faces, so an upgrade keeps the typography it had.
/// Two settings rather than one: the timer's digits want a face whose figures
/// are all the same width, or the time jitters sideways every second, and that
/// is a different requirement from the rest of the interface.
const MIGRATION_15: &str = "
    INSERT OR IGNORE INTO settings (key, value) VALUES ('font_ui', '');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('font_digits', '');
    INSERT INTO schema_version VALUES (15);
";

/// Seeds the timer layout: everything below the clock, which is where it has
/// always been, so an upgrade moves nothing.
const MIGRATION_16: &str = r#"
    INSERT OR IGNORE INTO settings (key, value)
        VALUES ('layout', '{"above":[],"below":["label","controls","footer"],"hidden":[]}');
    INSERT INTO schema_version VALUES (16);
"#;

/// Focus tools: tasks, notes, the daily goal, flow mode, focus sounds and
/// duration presets.
///
/// Sessions gain a task and a note; both stay NULL for every round recorded
/// before, which the statistics read as "no task" and "no note". Every new
/// setting starts in the state that changes nothing — no goal, no flow, no
/// sound, no active task — except the presets, which only add a switcher.
const MIGRATION_17: &str = r#"
    CREATE TABLE IF NOT EXISTS tasks (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
        color      TEXT NOT NULL DEFAULT '',
        archived   INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0, 1)),
        created_at INTEGER NOT NULL
    );
    ALTER TABLE sessions ADD COLUMN task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL;
    ALTER TABLE sessions ADD COLUMN note TEXT;
    CREATE INDEX IF NOT EXISTS idx_sessions_task_id ON sessions(task_id);
    INSERT OR IGNORE INTO settings (key, value) VALUES ('daily_goal', '0');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('flow_mode', 'false');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('ambient_sound', 'off');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('ambient_volume', '40');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('ambient_in_breaks', 'false');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('ambient_custom_name', '');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('active_task', '0');
    INSERT OR IGNORE INTO settings (key, value) VALUES ('notes_prompt', 'true');
    INSERT INTO schema_version VALUES (17);
"#;

/// Apply any pending migrations. Each migration is wrapped in a transaction
/// so a partial failure leaves the database unchanged.
pub fn run(conn: &Connection) -> Result<()> {
    let version = current_version(conn)?;

    if version < 1 {
        log::info!("[db/migrations] applying MIGRATION_1: initial schema");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_1} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_1 complete");
    }

    if version < 2 {
        log::info!("[db/migrations] applying MIGRATION_2: timer durations minutes → seconds");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_2} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_2 complete");
    }

    if version < 3 {
        log::info!("[db/migrations] applying MIGRATION_3: seed check_for_updates setting");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_3} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_3 complete");
    }

    if version < 4 {
        log::info!("[db/migrations] applying MIGRATION_4: seed global_shortcuts_enabled setting");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_4} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_4 complete");
    }

    if version < 5 {
        log::info!("[db/migrations] applying MIGRATION_5: seed short_breaks_enabled and long_breaks_enabled");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_5} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_5 complete");
    }

    if version < 6 {
        log::info!("[db/migrations] applying MIGRATION_6: seed local shortcut key bindings");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_6} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_6 complete");
    }

    if version < 7 {
        log::info!("[db/migrations] applying MIGRATION_7: seed sound presets, background image and LAN settings");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_7} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_7 complete");
    }

    if version < 8 {
        log::info!("[db/migrations] applying MIGRATION_8: seed background image framing settings");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_8} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_8 complete");
    }

    if version < 9 {
        log::info!("[db/migrations] applying MIGRATION_9: seed autostart setting");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_9} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_9 complete");
    }

    if version < 10 {
        log::info!("[db/migrations] applying MIGRATION_10: seed window opacity setting");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_10} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_10 complete");
    }

    if version < 11 {
        log::info!("[db/migrations] applying MIGRATION_11: seed pairing address setting");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_11} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_11 complete");
    }

    if version < 12 {
        log::info!("[db/migrations] applying MIGRATION_12: per-image background framing");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_12} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_12 complete");
    }

    if version < 13 {
        log::info!("[db/migrations] applying MIGRATION_13: seed background folder rotation");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_13} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_13 complete");
    }

    if version < 14 {
        log::info!("[db/migrations] applying MIGRATION_14: seed clock style");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_14} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_14 complete");
    }

    if version < 15 {
        log::info!("[db/migrations] applying MIGRATION_15: seed font choices");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_15} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_15 complete");
    }

    if version < 16 {
        log::info!("[db/migrations] applying MIGRATION_16: seed timer layout");
        conn.execute_batch(&format!("BEGIN; {MIGRATION_16} COMMIT;"))?;
        log::info!("[db/migrations] MIGRATION_16 complete");
    }

    if version < 17 {
        log::info!("[db/migrations] applying MIGRATION_17: tasks, notes and focus tools");
        // The preset list is JSON; bind it rather than splice it into SQL.
        let applied = conn
            .execute_batch(&format!("BEGIN; {MIGRATION_17}"))
            .and_then(|_| {
                conn.execute(
                    "INSERT OR IGNORE INTO settings (key, value) VALUES ('presets', ?1)",
                    [crate::settings::DEFAULT_PRESETS_JSON],
                )
            })
            .and_then(|_| conn.execute_batch("COMMIT;"));
        if let Err(e) = applied {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e);
        }
        log::info!("[db/migrations] MIGRATION_17 complete");
    }

    Ok(())
}

/// Returns the current schema version, or 0 if the database is fresh.
fn current_version(conn: &Connection) -> Result<i64> {
    let table_exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
        [],
        |row| row.get(0),
    )?;

    if !table_exists {
        return Ok(0);
    }

    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        // Second run must not error (version check prevents re-application).
        run(&conn).unwrap();
        let v: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 17);
    }

    #[test]
    fn framing_carries_over_from_the_settings_table() {
        let conn = Connection::open_in_memory().unwrap();
        // Stop one migration short, set a framing the way the old build did,
        // then let MIGRATION_12 run over it.
        conn.execute_batch(&format!("BEGIN; {MIGRATION_1} COMMIT;")).unwrap();
        conn.execute_batch(
            "INSERT OR REPLACE INTO settings (key, value) VALUES
                ('bg_image_pos_x', '20'), ('bg_image_pos_y', '80'), ('bg_image_zoom', '175');",
        )
        .unwrap();
        run(&conn).unwrap();

        let (x, y, z): (u32, u32, u32) = conn
            .query_row(
                "SELECT pos_x, pos_y, zoom FROM background_framing WHERE key = 'custom_background'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((x, y, z), (20, 80, 175), "the framing the user set is kept");
    }

    #[test]
    fn all_tables_created() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        for table in &["settings", "sessions", "custom_themes", "schema_version"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table '{table}' was not created");
        }
    }
}
