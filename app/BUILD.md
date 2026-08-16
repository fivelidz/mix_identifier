# MixID Android app (Tauri 2)

"Shazam for DJ mixes" — phone app. Owns only `app/`; the fingerprinting
engine lives in `crates/mixid-core` (built by another agent).

## Layout
```
app/
├── icon.png                  # 1024x1024 source icon (regen: bash scripts/gen_icon.sh)
├── scripts/gen_icon.sh       # icon generator (ImageMagick; PIL is broken locally)
├── ui/index.html             # frontend, no bundler — plain HTML/JS, withGlobalTauri
└── src-tauri/
    ├── Cargo.toml            # detached workspace ([workspace] table), dep on mixid-core
    ├── tauri.conf.json       # com.fivelidz.mixid, frontendDist ../ui
    ├── capabilities/default.json
    ├── src/{main.rs, lib.rs} # commands: list_mixes, get_mix, search_tracks,
    │                         #   analyze_path, index_folder (+ index-progress events)
    └── gen/android/          # generated Android Studio project (checked in)
        └── app/src/main/AndroidManifest.xml   # READ_MEDIA_AUDIO + legacy storage perms
        └── app/src/main/java/com/fivelidz/mixid/MainActivity.kt  # runtime permission request
```

## Build

```bash
export ANDROID_HOME=/home/fivelidz/Android/Sdk
export NDK_HOME=/home/fivelidz/Android/Sdk/ndk/28.2.13676358
export JAVA_HOME=/usr/lib/jvm/java-17-openjdk

cd app/
cargo tauri android build --apk --debug --target aarch64
# APK: src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

Desktop (Linux): `cd app/src-tauri && cargo run`

## ⚠️ mixid-core dependency status

`src-tauri/Cargo.toml` points at the real engine:
```toml
mixid-core = { path = "../../crates/mixid-core" }
```
At app-delivery time `crates/mixid-core` did not exist yet (parallel agent
still building it after 110+ min of waiting). Everything was validated against
a type-compatible stub (`/tmp/mixid-core-stub`): desktop build green, app
launches, Android APK builds. The delivered APK therefore contains the STUB —
UI works, fingerprinting is fake.

**Automatic rebuild:** `scripts/build_when_core_ready.sh` runs detached
(nohup) and rebuilds desktop check + Android APK as soon as the real crate
appears. Progress: `tail scripts/watch_build.log`. If the real API drifted
from the stub (watch `add_track`'s signature — assumed
`add_track(&mut self, title: &str, artist: &str, duration: f64, fp: &Fingerprint) -> anyhow::Result<i64>`),
fix the call sites in `src-tauri/src/lib.rs` and re-run the build command below.

## Install on the Redmi phone

Plain `adb install` FAILS on HyperOS (INSTALL_FAILED_USER_RESTRICTED). Use:
```bash
~/projects/phone_projects/camera_system/install_with_miui_dialog.sh <apk>
```

## On-device file access notes

- The dialog plugin's file/folder picker on Android returns SAF `content://`
  URIs in some cases, which Rust `std::fs` cannot read. The UI therefore also
  offers manual path inputs (e.g. `/storage/emulated/0/Music/mix.mp3`).
- MainActivity requests READ_MEDIA_AUDIO at runtime; with that grant, direct
  file-path reads under /storage/emulated/0 work on Android 11+.
