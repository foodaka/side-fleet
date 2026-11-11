#!/bin/sh
# Runs the app in demo mode, made-up machines and tasks and no network, and saves each screen to a
# folder in light and dark. Run scripts/bundle.sh first.
set -eu
cd "$(dirname "$0")/.."
out="${1:-dist/screenshots}"
mkdir -p "$out"
bin=dist/Sidefleet.app/Contents/MacOS/Sidefleet
for mode in --light --dark; do
  "$bin" --demo --tour "$PWD/$out" "$mode" &
  pid=$!
  ( sleep 150; kill "$pid" 2>/dev/null ) &
  watchdog=$!
  wait "$pid" || echo "the $mode tour ended early"
  kill "$watchdog" 2>/dev/null || true
done
ls "$out"
# Twelve stops in two appearances. Without this, a tour that saves nothing still passes.
count="$(find "$out" -name '*.png' | wc -l | tr -d ' ')"
if [ "$count" -ne 24 ]; then
  echo "expected 24 screenshots, got $count" >&2
  exit 1
fi
# A locked or sleeping screen captures as the same blank image every time.
distinct="$(find "$out" -name '*.png' -exec md5 -q {} + | sort -u | wc -l | tr -d ' ')"
if [ "$distinct" -lt 12 ]; then
  echo "only $distinct different screenshots: is the screen locked?" >&2
  exit 1
fi
