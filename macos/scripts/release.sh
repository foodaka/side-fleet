#!/bin/sh
# Builds Sidefleet.dmg for download: universal, signed with a Developer ID, notarized by Apple and
# stapled, so it opens without a Gatekeeper warning, even offline. On a Mac with Xcode 16 or newer.
#
#   SIGN_ID       "Developer ID Application: Name (TEAMID)", in a keychain this user can reach
#   NOTARY_KEY    path to an App Store Connect API key (.p8)
#   NOTARY_KEY_ID, NOTARY_ISSUER_ID   that key's ID and its issuer ID
#   VERSION       e.g. 0.2.0 (optional)
set -eu
cd "$(dirname "$0")/.."
: "${SIGN_ID:?set SIGN_ID to your Developer ID Application identity}"
: "${NOTARY_KEY:?set NOTARY_KEY to the .p8 file}"
: "${NOTARY_KEY_ID:?set NOTARY_KEY_ID}"
: "${NOTARY_ISSUER_ID:?set NOTARY_ISSUER_ID}"

UNIVERSAL=1 SIGN_ID="$SIGN_ID" scripts/bundle.sh
codesign --verify --strict --deep --verbose=2 dist/Sidefleet.app

# A disk image with the app and a link to Applications: drag one onto the other.
stage="$(mktemp -d)"
cp -R dist/Sidefleet.app "$stage/"
ln -s /Applications "$stage/Applications"
dmg=dist/Sidefleet.dmg
rm -f "$dmg"
hdiutil create -volname Sidefleet -srcfolder "$stage" -ov -format UDZO "$dmg" >/dev/null
codesign --force --timestamp --sign "$SIGN_ID" "$dmg"

# Notarizing the image covers the app inside it. Stapling the ticket lets a Mac check it offline.
xcrun notarytool submit "$dmg" --key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" --wait --timeout 30m
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
echo "$dmg"
