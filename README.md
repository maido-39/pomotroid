<div align="center">
  <img alt="Pomotroid" src=".github/images/pomotroid-title.png" width="800px">
</div>
<div align="center">
  <img alt="Pomotroid in action" src=".github/images/pomotroid-screens.png" width="800px">
</div>

<p align="center">Simple and visually-pleasing Pomodoro timer.</p>

---

- [Overview](#overview)
- [Features](#features)
- [Statistics](#statistics)
- [Themes](#themes)
- [Background image](#background-image)
- [Alert sounds](#alert-sounds)
- [Android app and phone pairing](#android-app-and-phone-pairing)
- [Install](#install)
- [WebSocket API](#websocket-api)
- [Development](#development)
- [License](#license)

## Overview

Pomotroid is a simple and configurable Pomodoro timer. It aims to provide a visually-pleasing and reliable way to track productivity using the Pomodoro Technique.

Built with [Tauri 2](https://tauri.app), [Rust](https://www.rust-lang.org), and [Svelte 5](https://svelte.dev).

## Features

- **Configurable timer** — customise work duration, break durations, and the number of rounds per long break
- **Statistics** — daily, weekly, and all-time session history with charts and a 52-week heatmap
- **38 bundled themes** — including Dracula, Nord, Tokyo Night, Catppuccin, Gruvbox, Rose Piné, and more; auto-switches with your OS light/dark preference
- **Custom themes** — drop a JSON file into the themes folder; applied instantly without a restart
- **Localization** — 8 languages: English, Spanish, French, German, Japanese, Chinese (Simplified), Turkish, and Portuguese; auto-detects OS language
- **Global shortcuts** — control the timer from anywhere, even when the window is hidden
- **Background image** — your own image, or a whole folder cycled each round, on a timer or by double-click; opacity, blur, brightness and per-image framing
- **Alert sounds** — five bundled presets per cue, preview buttons, or several files of your own played at random
- **Clock styles** — the classic ring, large digits with a progress bar, a line around the window edge, a flip clock, or a clock face
- **Fonts** — add your own font files, chosen separately for the interface and the timer's digits
- **Customize** — move the round name, the controls and the footer above or below the clock or hide them, and edit a theme's colours live
- **See-through window** — make the window translucent so what is behind it stays readable (Windows)
- **Android app** — the same timer on your phone, standalone or following your desktop over Wi-Fi
- **Tick sounds** — optional ticking during work and break rounds, independently toggleable
- **Dynamic tray icon** — progress arc updates in real time; reflects round type and pause state
- **Minimise / close to tray** — keep Pomotroid running in the background
- **Desktop notifications** — native OS alerts on round transitions
- **Compact mode** — a minimal set of controls appears when the window is resized small
- **Always on top** — optionally keep the timer above other windows
- **WebSocket server** — opt-in local server for stream overlays and external integrations
- **Diagnostic logging** — rotating log file with a one-click shortcut to the log folder

## Statistics

Pomotroid tracks every completed session and surfaces the data across three views: a daily summary with an hourly breakdown, a weekly bar chart with streak tracking, and an all-time 52-week heatmap.

<div align="center">
  <img alt="Pomotroid statistics window" src=".github/images/pomotroid-stats.png" width="800px">
</div>

## Themes

Pomotroid ships with 38 themes and supports fully custom themes with live hot-reload.

![Screenshots of Pomotroid using various themes](.github/images/pomotroid-themes-preview.png)

See [THEMES.md](./THEMES.md) for the full theme list and instructions on creating your own.

## Background image

Settings → Appearance → Background Image lets you pick any PNG, JPEG, WebP, GIF
or BMP file (up to 15 MB) and lay it behind the timer as a backdrop.

Three sliders control how it reads behind the UI:

| Control | Range | Effect |
| --- | --- | --- |
| Opacity | 0–100% | How strongly the image shows through |
| Blur | 0–40 px | Gaussian blur, so text stays legible over busy photos |
| Brightness | 20–150% | Dim a bright photo or lift a dark one |

The image is stored in the app data directory and applied to the timer,
settings and statistics windows, on desktop and on Android alike.

**Framing.** Drag the preview to choose which part of the image shows, and
scroll or pinch to zoom. Each image keeps its own framing, so a portrait and a
landscape photo can both be right.

**A folder of images.** Choose a folder instead of one file and the backdrop
cycles through every image in it, read where it sits — add or delete a picture
there and the rotation follows. Pictures change:

- when the round changes (on by default),
- every few minutes, from 1 to 120 (off by default),
- whenever you double-click the timer anywhere that is not a button.

Shuffle walks the folder in a random order and shows every picture before any
repeats; turning it off follows file names, so numbering the files sets the
order. A paired phone follows the desktop's rotation.

## Alert sounds

Each cue — work alert, short break alert, long break alert and the tick — can
use one of five bundled presets (Bell, Chime, Digital, Marimba, Soft), a file
of your own, or the original built-in sound. A ▶ button next to each cue plays
the current choice so you can compare them without waiting for a round to end.

Add several files to one cue and one of them plays each time, never the same
one twice in a row. Each cue keeps its own set, so focus and break can sound
different. The tick is the exception: it plays every second, so it always uses
the first of its files rather than varying.

Your own files always win over the preset; **Restore** removes them and returns
to the selected preset. The presets are generated by
[`scripts/gen_presets.py`](./scripts/gen_presets.py), so they can be tweaked
and regenerated.

## Clock styles

Settings → Appearance → Clock picks how the time is drawn. The ring suits a
plain window but draws a circle across the middle of a background image, so the
others trade how quickly the time reads against how much of the picture stays
visible:

| Style | Reads | Covers |
| --- | --- | --- |
| Ring | at a glance | the middle of the window |
| Large digits with a progress bar | easiest | very little |
| Line around the window edge | find where the line stops | nothing in the middle |
| Flip clock | from across a room | two cards |
| Clock face | the shrinking slice, no reading | about as much as the ring |

The countdown setting applies to all of them: on, the progress empties as the
round runs out; off, it fills.

## Fonts

Settings → Appearance → Fonts takes TTF, OTF, WOFF and WOFF2 files and copies
them into the app. The interface and the timer's digits are chosen separately,
because figures that are not all the same width make the time shift sideways
every second — a font whose numbers line up is worth picking for the digits even
when the rest of the interface uses another. A font that cannot be loaded falls
back to the bundled Mona Sans and the reason is written to the log.

## Customize

Settings → Customize arranges the timer and edits its colours, with the real
timer window as the preview.

**Layout.** The round name, the play/restart/skip buttons and the footer (round
count, reset and volume) each sit above the clock, below it, or hidden. Drag
them between those lanes, or use each item's lane picker and arrows, which work
from the keyboard and on a touch screen. On a window too narrow for the rest,
the timer keeps only the clock.

**Colours.** Start from any theme, change its colours, and every window shows the
result as you go; nothing is written until **Save Theme**, which also switches
to it. Saving under a built-in theme's name customises that theme, and deleting
the customised copy brings the original back. Only hex colours are accepted.

## Android app and phone pairing

Pomotroid also builds as a native Android app from the same Rust core and the
same Svelte UI. It works on its own — the timer, statistics, themes, sounds
and background image are all local — and keeps running in the background
through a foreground service with a live countdown notification.

Pair it with a desktop to follow that timer instead:

1. Desktop: **Settings → System → Allow Phone Connections (LAN)**, then scan
   the QR code that appears.
2. Phone: **Settings → Desktop → Scan QR Code**, or paste the link.

While paired, the desktop is the source of truth. The phone mirrors its rounds,
plays its alert sounds, shows its background image, and any setting changed on
the phone is applied on the desktop. Switching back to the phone's own timer is
manual, so a dropped Wi-Fi connection keeps retrying instead of silently
diverging.

The same pairing link opens the UI in a phone browser, which is handy for
trying the connection without installing the APK.

See [docs/ANDROID.md](./docs/ANDROID.md) for build instructions.

## Install

### Download

Download the latest release from the [releases](https://github.com/Splode/pomotroid/releases) page.

Available for **Windows** (installer + standalone exe), **macOS** (universal DMG), and **Linux** (`.deb` + AppImage).

> **Note:** Pomotroid is currently unsigned. Depending on your OS security settings you may see a warning on first launch — this is expected and can be safely dismissed.

### Linux notes

**System tray on GNOME:** GNOME does not display tray icons by default. To use the tray icon feature (Settings → System → Show in System Tray), install the [AppIndicator and KStatusNotifierItem Support](https://extensions.gnome.org/extension/615/appindicator-support/) extension, then log out and back in. On Fedora: `sudo dnf install gnome-shell-extension-appindicator`. On Debian/Ubuntu it is pre-installed. Other desktop environments (KDE Plasma, XFCE, Cinnamon, MATE) support tray icons natively with no extra steps.

### Homebrew (macOS)

```sh
brew install --cask pomotroid
```

> The Homebrew cask is maintained separately and may lag behind the latest release. Check the [releases](https://github.com/Splode/pomotroid/releases) page for the most current version.

## Custom Themes

Pomotroid supports user-created themes with automatic hot-reload — no restart required. See [THEMES.md](./THEMES.md) for directory paths, the full color reference, and a step-by-step guide.

## WebSocket API

Pomotroid exposes an optional WebSocket server (disabled by default) for integration with external tools, stream overlays, and automation scripts.

**Enable it** in Settings → System → WebSocket Server, then connect to `ws://127.0.0.1:<port>/ws` (default port: 1314).

### Messages

**Client → Server**

| Message                  | Description                     |
| ------------------------ | ------------------------------- |
| `{ "type": "getState" }` | Request the current timer state |
| `{ "type": "ping" }`     | Liveness check; answered with `pong` |

**Server → Client**

| Event         | Payload                            | Description                                      |
| ------------- | ---------------------------------- | ------------------------------------------------ |
| `state`       | `TimerState` object                | Response to `getState`                           |
| `started`     | `{ total_secs }`                   | A round started                                  |
| `tick`        | `{ elapsed_secs, total_secs }`     | Fired every second while running                 |
| `roundChange` | `TimerState` object                | Fired whenever the timer advances to a new round |
| `paused`      | `{ elapsed_secs }`                 | Timer paused                                     |
| `resumed`     | `{ elapsed_secs }`                 | Timer resumed                                    |
| `reset`       | `TimerState` object                | Timer reset                                      |
| `error`       | `{ message }`                      | Protocol error                                   |

`TimerState` fields: `elapsed_secs`, `total_secs`, `is_running`, `is_paused`, `round_type`, `previous_round_type`, `work_round_number`, `work_rounds_total`, `session_work_count`.

### Control access

Everything above is read-only and needs no authentication, so existing overlays
keep working. Controlling the timer or reading and writing settings requires a
pairing token — the same one the Android app uses:

```json
{ "type": "auth", "token": "<token from Settings → System>" }
```

After `authOk`, the connection may send `toggle`, `reset`, `skip`,
`restartRound`, `getSettings`, `setSetting`, `resetSettings`, `getThemes`,
`getStats`, `clearSessions`, `getBackground`, `setBackground`,
`clearBackground`, `getAudio`, `setAudio`, `clearAudio` and `getAudioInfo`.
Requests may carry an `id`, which the reply echoes. Connections from localhost
can authenticate with an empty token when LAN access is off. The full message
catalogue is documented in
[`src-tauri/src/websocket/protocol.rs`](./src-tauri/src/websocket/protocol.rs).

## Development

See [CONTRIBUTING.md](./CONTRIBUTING.md) for full setup instructions, project structure, and the release process.

### Quick start

```bash
# Install dependencies
npm install

# Run in development mode (hot-reload)
npm run tauri dev

# Build a production release
npm run tauri build
```

### Localization

UI strings live in `src/messages/<locale>.json` (en, es, fr, de, ja, zh, pt, tr). The compiled output in `src/paraglide/` is generated at build time and is not committed to the repository.

**During development**, the Paraglide Vite plugin compiles messages automatically whenever `npm run tauri dev` or `npm run tauri build` is run — no manual step required.

**After adding or changing message keys**, regenerate the output explicitly so that `svelte-check` and your editor can pick up the new types:

```bash
npm run paraglide:compile
```

This is also run automatically as part of `npm run check`.

## License

MIT &copy; [Christopher Murphy](https://github.com/Splode)
