# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Pomotroid is a Pomodoro timer built with **Tauri 2 + Rust** (backend) and **SvelteKit + Svelte 5** (frontend). The app has been fully rewritten from Electron+Vue to this stack.

The same code base produces three targets:

- **desktop** (Windows/macOS/Linux) — the primary target
- **Android** — Tauri 2 mobile; same Rust core, plus a Kotlin foreground service
- **browser** — the desktop serves its own UI over the LAN port for testing the pairing protocol (no Tauri runtime)

## Commands

```bash
# Development (runs Vite dev server + Tauri)
npm run tauri dev

# Production build
npm run tauri build

# Frontend-only Vite dev (no Tauri IPC available)
npm run dev

# Format all frontend source files
npm run format

# Type-check Svelte + TypeScript
npm run check

# Rust unit tests (settings, migrations, audio, background, websocket protocol, remote)
cd src-tauri && cargo test

# Android (see docs/ANDROID.md — must run from a local path, not a mapped drive)
npm run tauri android init && python scripts/sync_android.py
npm run tauri android dev
npm run tauri android build --debug --target aarch64

# Regenerate i18n types after editing inlang message files
npm run paraglide:compile
```

Rust has unit tests (`cargo test`); the frontend has none.

## Architecture

### Three windows (desktop) / three routes (mobile + web)

- **`main`** (`src/routes/+page.svelte`) — the timer window
- **`settings`** (`src/routes/settings/+page.svelte`) — opened via `new WebviewWindow('settings', ...)` from `Titlebar.svelte`
- **`stats`** (`src/routes/stats/+page.svelte`) — statistics window

On Android and in the browser build there is a single webview, so the same
routes are navigated in-app with `goto()`. `src/lib/utils/platform.ts` decides:
`isDesktopWindow` guards window APIs (`show()`, resize handles, always-on-top),
`useInAppNav` switches navigation, and `isWeb` switches the backend transport.

### IPC layer

Components import from **`$lib/backend`**, never from `$lib/ipc` directly.
`$lib/backend` is a facade that picks the transport:

- inside Tauri → `src/lib/ipc/index.ts` (typed `invoke()` / `listen()` wrappers)
- in the browser build → `src/lib/remote/web.ts` (WebSocket client speaking the
  protocol in `src-tauri/src/websocket/protocol.rs`, re-dispatching the same
  event names)

Never call `invoke()` directly in components.

Rust commands live in `src-tauri/src/commands.rs`. All commands return `Result<T, String>`.

**Tauri events emitted by Rust:**

- `timer:tick` — `{ elapsed_secs, total_secs }` — fires every second while running
- `timer:paused` — `{ elapsed_secs }`
- `timer:resumed` — `{ elapsed_secs }`
- `timer:round-change` — full `TimerSnapshot`
- `timer:reset` — full `TimerSnapshot` (used to sync frontend after settings changes)
- `settings:changed` — full `Settings` object
- `themes:changed` — `Theme[]`
- `sessions:cleared` — history wiped
- `sessions:changed` — a session was recorded (stats refresh)
- `audio:changed` — cue key whose sound changed (`"all"` after a reset)
- `background:changed` — background image set or cleared
- `remote:status` — `RemoteStatus` for follow mode (mobile)
- `websocket:clients` — `{ connected, authed }` when a phone connects/disconnects

### TypeScript ↔ Rust type contract

`src/lib/types.ts` mirrors Rust structs (snake_case field names). When modifying `Settings` in `src-tauri/src/settings/mod.rs` or `TimerSnapshot` in `src-tauri/src/timer/mod.rs`, update `types.ts` to match.

**Important conversions** — Rust converts on load/save, frontend always sees the converted form:

- Time: stored in DB as **minutes**, `Settings` struct holds **seconds**
- Volume: stored in DB as **0–100**, `Settings` struct holds **0.0–1.0**

### Remote follow mode

`src-tauri/src/remote/mod.rs` lets one instance (normally the Android app)
follow a desktop over its LAN WebSocket server.

Two rules keep this from leaking into the UI:

1. Commands in `commands.rs` start with `remote::forward::<T>(...)`, which
   returns `Some(result)` in remote mode (the request ran on the desktop) and
   `None` locally, so each command falls through to its `*_impl` function.
2. Events received from the desktop are re-emitted under the **same** Tauri
   event names the local engine uses, so components need no special casing.

The `*_impl(app, ..)` functions are the shared implementation: Tauri commands,
the WebSocket server's `Backend` trait and the remote client all go through
them. Adding a command means adding an `*_impl` too if a paired phone should
be able to use it.

### Settings storage

SQLite key/value table. DB column names differ from Rust struct field names (e.g., DB key `work_rounds` → struct field `long_break_interval`). Mappings are in `src-tauri/src/settings/mod.rs`. New settings need a DB key mapping, a default value in `settings/defaults.rs`, and a corresponding `settings_set` key handler.

Schema migrations are in `src-tauri/src/db/migrations.rs`. Add a new `MIGRATION_N` constant and increment the schema version check in `run()`. The current schema version is **11**.

### Timer engine

The timer runs in a background thread. `TimerController` (`src-tauri/src/timer/mod.rs`) is the public API registered as Tauri state. `SequenceState` (`src-tauri/src/timer/sequence.rs`) tracks round type and count — it has its own `work_rounds_total` field that must be kept in sync via `apply_settings()` when settings change.

### Theme system

Themes are JSON files in `static/themes/` (built-in) and `{app_data_dir}/themes/` (custom, user-created). Each theme is a JSON object with `name` and `colors` (CSS custom property map with `--` prefix keys). The Rust `themes` module watches the custom directory for changes and emits `themes:changed`. `applyTheme()` in `src/lib/stores/theme.ts` sets CSS custom properties on `:root`.

### Platform gating (desktop vs mobile)

Modules that cannot exist on Android are split rather than sprinkled with
`#[cfg]` at every call site: `tray/`, `shortcuts/`, `websocket/` and `audio/`
each have a `mod.rs` selecting `desktop.rs` or `stub.rs`/`mobile.rs` with an
identical public surface. `timer/mod.rs` therefore needs no gating at all.

Desktop-only crates (rodio, tiny-skia, axum, notify, global-shortcut, updater)
live under a `cfg(not(android/ios))` dependency table in `Cargo.toml`; mobile
adds `tauri-plugin-fs` and `tauri-plugin-barcode-scanner`. Keep new
dependencies in the right table — a crate in the shared table must compile for
Android.

`invoke_handler` in `lib.rs` lists every command once, for every platform, so a
`#[tauri::command]` must exist everywhere. Put the `#[cfg]` on the *implementation*
— give the mobile side of the module split a stub that returns nothing useful,
the way `websocket::stub` answers `pairing_info` and `network_interfaces` — and
never on the command itself. A `#[cfg(desktop)]` command compiles on the desktop
and fails the Android build with `cannot find macro __cmd__<name>`, which nothing
notices until the next APK.

### Capabilities / permissions

`src-tauri/capabilities/default.json` (desktop, `platforms` restricted) and
`src-tauri/capabilities/mobile.json` (Android/iOS). Any new IPC command or API
plugin used from the frontend must be allowlisted in the right one.

### Localization

Uses `@inlang/paraglide-js`. Message files are in `project.inlang/`. After editing messages, run `npm run paraglide:compile` to regenerate `src/paraglide/`. Import message functions from `$lib/locale.svelte.ts`.

### UI conventions

- Window is decoration-free; custom titlebar in `Titlebar.svelte`
- Compact mode: `isCompact = w < 300 || h < 300` — hides footer/label/play-pause, shows only dial
- `uiScale` applied via CSS `zoom` on `.timer` div (not `transform: scale`)
- Slider track alignment: use `calc(frac * (100% - 14px) + 7px)` to match native thumb position

### Rust modules

| Module          | Purpose                                                                              |
| --------------- | ------------------------------------------------------------------------------------ |
| `audio`         | `common` (cues, presets, custom files, resolution) + `desktop` (rodio) / `mobile` (Kotlin MediaPlayer) |
| `background`    | Custom background image storage and data-URL encoding                                |
| `mobile`        | Android foreground service bridge (`timer_service`)                                  |
| `remote`        | Follow-mode WebSocket client: forwards commands, re-emits desktop events             |
| `db`            | SQLite connection, migrations, queries                                               |
| `notifications` | OS notification wrapper                                                              |
| `settings`      | Settings struct, DB load/save, defaults                                              |
| `shortcuts`     | Global keyboard shortcut registration via tauri-plugin-global-shortcut               |
| `themes`        | Theme loading, custom theme watching (notify crate), tray icon rendering (tiny-skia) |
| `timer`         | Timer engine (background thread), sequence state, controller                         |
| `tray`          | System tray icon, tray menu, timer display in tray                                   |
| `websocket`     | `protocol` (wire types) + `desktop` (axum server, LAN pairing, static UI) / `stub`   |
