#!/bin/sh
# Builds Sidefleet.app into macos/dist. On a Mac, with Xcode 16 or newer (or its command line tools).
#   scripts/bundle.sh                 ad-hoc signed: runs here, and elsewhere with Gatekeeper's permission
#   SIGN_ID="Developer ID Application: …" scripts/bundle.sh    signed for distribution
#   UNIVERSAL=1 scripts/bundle.sh     for Apple silicon and Intel
set -eu
cd "$(dirname "$0")/.."

app=dist/Sidefleet.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

if [ "${UNIVERSAL:-}" = 1 ]; then
  # One build per architecture, joined with lipo. `swift build --arch arm64 --arch x86_64` hands
  # the build to a different build system that rejects the app target's Swift 5 language mode.
  for arch in arm64 x86_64; do
    swift build -c release --product Sidefleet --arch "$arch"
  done
  bin="$(swift build -c release --product Sidefleet --arch arm64 --show-bin-path)"
  x86_bin="$(swift build -c release --product Sidefleet --arch x86_64 --show-bin-path)"
  lipo -create "$bin/Sidefleet" "$x86_bin/Sidefleet" -output "$app/Contents/MacOS/Sidefleet"
else
  swift build -c release --product Sidefleet
  bin="$(swift build -c release --product Sidefleet --show-bin-path)"
  cp "$bin/Sidefleet" "$app/Contents/MacOS/Sidefleet"
fi
# SwiftPM resource bundles sit beside the binary; the app has none today, but a dependency could.
for b in "$bin"/*.bundle; do [ -e "$b" ] && cp -R "$b" "$app/Contents/Resources/"; done

# Sparkle, the updater, is a framework: it goes in Contents/Frameworks, where the binary must look.
# Its macOS build is already universal, so the arm64 build's copy serves both.
mkdir -p "$app/Contents/Frameworks"
ditto "$bin/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
if ! otool -l "$app/Contents/MacOS/Sidefleet" | grep -q "@executable_path/../Frameworks"; then
  install_name_tool -add_rpath "@executable_path/../Frameworks" "$app/Contents/MacOS/Sidefleet"
fi

# Sparkle compares CFBundleVersion to tell whether a release is newer, so it has to be the version
# itself, not a commit hash; the commit goes in a key of its own.
version="${VERSION:-0.1.0}"
commit="$(git rev-parse --short HEAD 2>/dev/null || echo dev)"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$version/" -e "s/__COMMIT__/$commit/" Resources/Info.plist > "$app/Contents/Info.plist"

# Every icon size from the one master.
iconset="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" Resources/AppIcon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" Resources/AppIcon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

# Sign from the inside out: Sparkle's helpers, then the framework, then the app. The downloader keeps
# the entitlements Sparkle gave it.
if [ -n "${SIGN_ID:-}" ]; then
  sign() { codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$@"; }
else
  sign() { codesign --force --sign - "$@"; }
fi
fw="$app/Contents/Frameworks/Sparkle.framework/Versions/B"
for xpc in "$fw"/XPCServices/*.xpc; do
  [ -e "$xpc" ] && sign --preserve-metadata=entitlements "$xpc"
done
sign "$fw/Autoupdate"
sign "$fw/Updater.app"
sign "$app/Contents/Frameworks/Sparkle.framework"
sign "$app"
echo "$app"
