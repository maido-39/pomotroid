# Android build

The Android app is the same Svelte UI and the same Rust core as the desktop
build, compiled with Tauri 2 mobile. It runs as a standalone timer and can
additionally pair with a desktop Pomotroid over the local network.

## What is native

The WebView is paused by Android when the app goes to the background, so
anything that must keep working while the phone is locked lives in Kotlin:

| Piece | File | Purpose |
| --- | --- | --- |
| `TimerServicePlugin` | `src-tauri/android-plugin/TimerServicePlugin.kt` | Tauri plugin; receives `update` / `roundChanged` / `playSound` / `stop` from Rust |
| `TimerForegroundService` | `src-tauri/android-plugin/TimerForegroundService.kt` | Foreground service, countdown notification with Pause/Skip actions, wake lock |
| Rust bridge | `src-tauri/src/mobile/timer_service.rs` | Called from the timer loop and from remote-follow mode |

Round-change sounds are played by `MediaPlayer` (not a notification-channel
sound) so user-supplied files and presets both work.

## Prerequisites

- **JDK 17+** — `JAVA_HOME` must point at it. The JBR bundled with Android
  Studio is not always complete; a standalone Temurin 17 is the safe choice.
- **Android SDK** with platform 34 or newer — `ANDROID_HOME`.
- **NDK r28+** — `NDK_HOME`. r28 and later link 16 KB-page-aligned binaries,
  which Android 15 and the Play Store require. With an older NDK, add
  `-C link-arg=-Wl,-z,max-page-size=16384` to the Android targets' rustflags.
- Rust targets:

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
```

### Important: build from a local path

The Tauri mobile CLI (cargo-mobile2) resolves the project directory to its
canonical form and refuses to continue when that differs from the path you
gave it. On a **mapped network drive** (for example `S:` → `\\server\share`)
`tauri android init` therefore fails with:

```
AssetDirOutsideOfAppRoot { asset_dir: "assets", root_dir: "…\src-tauri" }
```

Clone or copy the repository to a local disk (`C:\dev\pomotroid`) and run the
Android commands there. `src-tauri/gen/android/` is committed, so the generated
project can be copied back into the canonical checkout afterwards.

## First-time setup

```bash
npm install
npm run tauri android init
python scripts/sync_android.py
```

`sync_android.py` is idempotent and does the parts the Tauri template does not:

- copies the Kotlin sources into `gen/android/app/src/main/java/com/splode/pomotroid/`
- adds `POST_NOTIFICATIONS`, `FOREGROUND_SERVICE`,
  `FOREGROUND_SERVICE_SPECIAL_USE`, `WAKE_LOCK`, `CAMERA` and the
  `<service>` declaration to `AndroidManifest.xml`
- allows cleartext traffic in release builds (LAN pairing uses plain `ws://`)
- adds the `androidx.core:core-ktx` dependency

Re-run it after any `tauri android init` that regenerates the project, and
**after every edit to `src-tauri/android-plugin/`** — those files are the source
of truth, but Gradle compiles the copies under `gen/android/`, which are
committed. Commit both sides together: a plugin change whose generated copy is
left behind builds an app missing the new `@Command`s, and Rust only finds out
at runtime ("No command … found for plugin").

## Windows gotchas

Two environment problems will stop the build; neither is a code issue.

**Symlink permission.** `tauri android build` links the compiled `.so` into
`gen/android/app/src/main/jniLibs/<abi>/`, and Windows only allows creating
symlinks with Developer Mode enabled (Settings → System → For developers) or
from an elevated shell. Without it the build stops at:

```
Failed to create a symbolic link ... Creation symbolic link is not allowed for this system.
```

If you cannot enable Developer Mode, copy the library in yourself and let
Gradle skip the task that creates the link:

```bash
cp src-tauri/target/aarch64-linux-android/debug/libpomotroid_lib.so \
   src-tauri/gen/android/app/src/main/jniLibs/arm64-v8a/
cd src-tauri/gen/android
./gradlew assembleArm64Debug -x rustBuildArm64Debug
```

The cargo build must have run first; `tauri android build` does that before it
fails on the symlink, so simply re-running the Gradle command afterwards works.

**Gradle's JDK needs network access.** Gradle downloads its own distribution
and the Android Gradle Plugin on first use. If endpoint-security software
blocks the particular `java.exe` that `JAVA_HOME` points at, every download
fails with `java.net.SocketException: Permission denied: getsockopt` even
though the firewall is off and other tools reach the same hosts. Point
`JAVA_HOME` at a JDK that is already allowed (for example the one Gradle
manages under `~/.gradle/jdks/`) or have the security software allow the new
one.

## Build and run

```bash
# emulator or attached device, with hot reload
npm run tauri android dev

# debug APK for a phone
npm run tauri android build --debug --target aarch64
```

The APK is written to
`src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk`
(`x86_64/` for the emulator, `universal/` when no `--target` is given). Install
it with `adb install -r <path>`.

A debug APK is large (~150 MB) because the Rust library keeps its debug
symbols; a release build is a fraction of that.

The first Gradle run downloads the wrapper and the Android toolchain — budget
15–20 minutes and a few GB.

## Release APK

```bash
npm run tauri android build --target aarch64
```

The output is
`src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release.apk`,
about 19 MB. Release builds run R8; the Tauri plugin rules keep the
`@TauriPlugin` classes, their `@Command` methods and `@InvokeArg` fields, so a
command that does nothing on a release build but works on a debug one points
at a missing keep rule, not at the Kotlin code.

Without Developer Mode the same symlink workaround applies, with the release
library and task names:

```bash
cp src-tauri/target/aarch64-linux-android/release/libpomotroid_lib.so \
   src-tauri/gen/android/app/src/main/jniLibs/arm64-v8a/
cd src-tauri/gen/android
./gradlew assembleArm64Release -x rustBuildArm64Release
```

**Signing.** An unsigned release APK does not install. For a store or
long-lived build, follow the Tauri "Android code signing" guide and add
`gen/android/keystore.properties` (never commit it). For a sideload-only
build, signing the release with the standard Android debug key is enough:
add `signingConfig = signingConfigs.getByName("debug")` to the `release`
block of `gen/android/app/build.gradle.kts` in your local copy only. The
published 1.8.0 APK was built that way, which is why it says it is not a Play
Store build. Check the result with:

```bash
apksigner verify --print-certs app-arm64-release.apk
```

An APK signed with a different key cannot update an installed one; uninstall
first when switching keys.

## Pairing with a desktop

1. Desktop: **Settings → System → Allow Phone Connections (LAN)**. A QR code
   and a `http://<ip>:<port>/?token=…` link appear. Windows asks to allow
   Pomotroid through the firewall the first time.
2. Phone: **Settings → Desktop → Scan QR Code**, or paste the link.

While paired, the desktop is the source of truth: the phone mirrors its
timer, plays its sounds, shows its background image, and every setting the
phone changes is applied on the desktop. Switching back is manual
(**Disconnect**); a dropped connection keeps trying to reconnect.

On the emulator the host desktop is reachable at `10.0.2.2`, so
`10.0.2.2:1314` plus the token pairs without a real network.

## Notes and limits

- Notification permission is requested on first launch (Android 13+). Without
  it, round alerts and the ongoing timer notification are suppressed.
- Battery optimisation can still stop the foreground service on aggressive
  OEM builds; exempt Pomotroid if rounds end late.
- The Android picker returns `content://` URIs, which Rust cannot open, so
  custom sounds and background images are read in the WebView with
  `tauri-plugin-fs` and handed to Rust as bytes
  (`audio_set_custom_bytes` / `background_set_custom_bytes`).
