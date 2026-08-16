#!/usr/bin/env bash
# Watcher: waits for crates/mixid-core/src/lib.rs to appear (built by the
# parallel core agent), then rebuilds the desktop check + Android APK with the
# REAL engine. Logs to app/scripts/watch_build.log. Started detached via nohup.
set -uo pipefail

ROOT=/home/fivelidz/projects/GLM_projects/mix_identifier
CORE="$ROOT/crates/mixid-core/src/lib.rs"
APP="$ROOT/app"
LOG="$APP/scripts/watch_build.log"

export ANDROID_HOME=/home/fivelidz/Android/Sdk
export NDK_HOME=/home/fivelidz/Android/Sdk/ndk/28.2.13676358
export JAVA_HOME=/usr/lib/jvm/java-17-openjdk

echo "$(date '+%F %T') watcher started, waiting for $CORE" >> "$LOG"
for i in $(seq 1 2160); do  # up to 6h @ 10s
  if [ -f "$CORE" ]; then
    sleep 60  # let the core agent finish writing all files
    echo "$(date '+%F %T') core detected — running cargo check" >> "$LOG"
    if (cd "$APP/src-tauri" && cargo check >> "$LOG" 2>&1); then
      echo "$(date '+%F %T') cargo check OK — building Android APK" >> "$LOG"
      if (cd "$APP" && cargo tauri android build --apk --debug --target aarch64 >> "$LOG" 2>&1); then
        echo "$(date '+%F %T') ANDROID APK REBUILT WITH REAL CORE:" >> "$LOG"
        echo "  $APP/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk" >> "$LOG"
      else
        echo "$(date '+%F %T') android build FAILED — see log above" >> "$LOG"
      fi
    else
      echo "$(date '+%F %T') cargo check FAILED (API drift vs stub? fix app/src-tauri/src/lib.rs call sites) — see log above" >> "$LOG"
    fi
    exit 0
  fi
  sleep 10
done
echo "$(date '+%F %T') watcher timed out after 6h" >> "$LOG"
