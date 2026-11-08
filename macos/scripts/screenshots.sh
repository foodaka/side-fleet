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
  ( sleep 90; kill "$pid" 2>/dev/null ) &
  watchdog=$!
  wait "$pid" || echo "the $mode tour ended early"
  kill "$watchdog" 2>/dev/null || true
done
ls "$out"
