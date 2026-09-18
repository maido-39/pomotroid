"""Install the hand-written Android layer into the generated Gradle project.

`tauri android init` generates `src-tauri/gen/android/` from a template. This
script adds the parts Pomotroid needs on top of that template and is safe to
re-run after a regeneration:

  1. copies `src-tauri/android-plugin/*.kt` into the app module
     (`app/src/main/java/com/splode/pomotroid/`)
  2. adds the permissions and the `<service>` declaration to AndroidManifest.xml
  3. allows cleartext traffic in release builds (LAN pairing uses ws://)
  4. adds the AndroidX dependencies the service needs

Usage (from the repo root):  python scripts/sync_android.py
"""

from __future__ import annotations

import os
import re
import shutil
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GEN = os.path.join(REPO, "src-tauri", "gen", "android")
APP = os.path.join(GEN, "app")
PKG_DIR = os.path.join(APP, "src", "main", "java", "com", "splode", "pomotroid")
PLUGIN_SRC = os.path.join(REPO, "src-tauri", "android-plugin")
MANIFEST = os.path.join(APP, "src", "main", "AndroidManifest.xml")
BUILD_GRADLE = os.path.join(APP, "build.gradle.kts")

PERMISSIONS = [
    "android.permission.INTERNET",
    "android.permission.POST_NOTIFICATIONS",
    "android.permission.FOREGROUND_SERVICE",
    "android.permission.FOREGROUND_SERVICE_SPECIAL_USE",
    "android.permission.WAKE_LOCK",
    "android.permission.CAMERA",
]

SERVICE_XML = """        <service
            android:name=".TimerForegroundService"
            android:exported="false"
            android:foregroundServiceType="specialUse">
            <property
                android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE"
                android:value="Keeps the Pomodoro timer running and shows its countdown while the app is in the background." />
        </service>
"""

DEPS = [
    'implementation("androidx.core:core-ktx:1.13.1")',
]


def fail(msg: str) -> None:
    sys.exit(f"error: {msg}")


def copy_kotlin() -> None:
    if not os.path.isdir(PKG_DIR):
        fail(f"app package dir not found: {PKG_DIR}\nRun `npm run tauri android init` first.")
    os.makedirs(PKG_DIR, exist_ok=True)
    for name in sorted(os.listdir(PLUGIN_SRC)):
        if not name.endswith(".kt"):
            continue
        shutil.copy2(os.path.join(PLUGIN_SRC, name), os.path.join(PKG_DIR, name))
        print(f"copied {name} -> app/src/main/java/com/splode/pomotroid/")


def patch_manifest() -> None:
    if not os.path.isfile(MANIFEST):
        fail(f"manifest not found: {MANIFEST}")
    xml = open(MANIFEST, encoding="utf-8").read()
    original = xml

    # 1. permissions (inserted before <application>)
    missing = [p for p in PERMISSIONS if f'android:name="{p}"' not in xml]
    if missing:
        block = "".join(f'    <uses-permission android:name="{p}" />\n' for p in missing)
        idx = xml.index("    <application")
        xml = xml[:idx] + block + xml[idx:]
        print("added permissions:", ", ".join(p.rsplit(".", 1)[-1] for p in missing))

    # 2. foreground service declaration (inserted before </application>)
    if ".TimerForegroundService" not in xml:
        idx = xml.index("    </application>")
        xml = xml[:idx] + SERVICE_XML + xml[idx:]
        print("added <service> TimerForegroundService")

    # 3. cleartext traffic — the template uses a manifest placeholder that is
    #    "false" for release; LAN pairing needs plain ws:// in release too.
    xml = xml.replace(
        'android:usesCleartextTraffic="${usesCleartextTraffic}"',
        'android:usesCleartextTraffic="true"',
    )

    if xml != original:
        open(MANIFEST, "w", encoding="utf-8", newline="\n").write(xml)
        print("patched AndroidManifest.xml")
    else:
        print("AndroidManifest.xml already up to date")


def patch_gradle() -> None:
    if not os.path.isfile(BUILD_GRADLE):
        fail(f"build.gradle.kts not found: {BUILD_GRADLE}")
    text = open(BUILD_GRADLE, encoding="utf-8").read()
    original = text

    missing = [d for d in DEPS if d.split(":")[1] not in text]
    if missing:
        m = re.search(r"\ndependencies\s*\{", text)
        if not m:
            fail("no dependencies block in app/build.gradle.kts")
        insert_at = m.end()
        block = "".join(f"\n    {d}" for d in missing)
        text = text[:insert_at] + block + text[insert_at:]
        print("added dependencies:", ", ".join(d for d in missing))

    # Release builds must also permit cleartext (LAN ws://).
    text = text.replace(
        'manifestPlaceholders["usesCleartextTraffic"] = "false"',
        'manifestPlaceholders["usesCleartextTraffic"] = "true"',
    )

    if text != original:
        open(BUILD_GRADLE, "w", encoding="utf-8", newline="\n").write(text)
        print("patched app/build.gradle.kts")
    else:
        print("app/build.gradle.kts already up to date")


def main() -> None:
    if not os.path.isdir(GEN):
        fail(
            f"{GEN} does not exist.\n"
            "Run `npm run tauri android init` first (from a LOCAL path — the Tauri "
            "mobile CLI rejects mapped network drives)."
        )
    copy_kotlin()
    patch_manifest()
    patch_gradle()
    print("\nAndroid layer synced.")


if __name__ == "__main__":
    main()
