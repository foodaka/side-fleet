#!/bin/sh
# Builds Sidefleet.app into macos/dist. On a Mac, with Xcode 16 or newer (or its command line tools).
#   scripts/bundle.sh                 ad-hoc signed: runs here, and elsewhere with Gatekeeper's permission
#   SIGN_ID="Developer ID Application: …" scripts/bundle.sh    signed for distribution
#   UNIVERSAL=1 scripts/bundle.sh     for Apple silicon and Intel
set -eu
cd "$(dirname "$0")/.."

if [ "${UNIVERSAL:-}" = 1 ]; then
  set -- --arch arm64 --arch x86_64
else
  set --
fi
swift build -c release --product Sidefleet "$@"
bin="$(swift build -c release --product Sidefleet "$@" --show-bin-path)"

app=dist/Sidefleet.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin/Sidefleet" "$app/Contents/MacOS/Sidefleet"
# SwiftPM resource bundles sit beside the binary; the app has none today, but a dependency could.
for b in "$bin"/*.bundle; do [ -e "$b" ] && cp -R "$b" "$app/Contents/Resources/"; done

version="${VERSION:-0.1.0}"
build="$(git rev-parse --short HEAD 2>/dev/null || echo dev)"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" Resources/Info.plist > "$app/Contents/Info.plist"

# Every icon size from the one master.
iconset="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" Resources/AppIcon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" Resources/AppIcon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

if [ -n "${SIGN_ID:-}" ]; then
  codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$app"
else
  codesign --force --sign - "$app"
fi
echo "$app"
