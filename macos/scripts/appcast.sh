#!/bin/sh
# Writes dist/appcast.xml, the feed the app's updater (Sparkle) reads, for one release's .dmg. Run after
# scripts/release.sh, which notarizes and staples the .dmg: the signature covers the final file.
#
#   VERSION              e.g. 0.2.0, the tag without its "v"
#   SPARKLE_PRIVATE_KEY  base64 EdDSA private key (the repository secret of that name)
set -eu
cd "$(dirname "$0")/.."
: "${VERSION:?set VERSION, e.g. 0.2.0}"
: "${SPARKLE_PRIVATE_KEY:?set SPARKLE_PRIVATE_KEY}"
repo="${REPO:-foodaka/side-fleet}"
dmg=dist/Sidefleet.dmg

# Sparkle's tools come with its Swift package.
sign_update="$(find .build/artifacts -type f -path '*/Sparkle/bin/sign_update' | head -1)"
if [ -z "$sign_update" ]; then
  echo "sign_update not found under .build/artifacts: build the app first" >&2
  exit 1
fi

# Prints: sparkle:edSignature="…" length="…"
attrs="$(printf '%s' "$SPARKLE_PRIVATE_KEY" | "$sign_update" --ed-key-file - "$dmg")"
case "$attrs" in *edSignature=*length=*) ;; *) echo "sign_update gave: $attrs" >&2; exit 1 ;; esac

cat > dist/appcast.xml <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Sidefleet</title>
    <item>
      <title>Sidefleet $VERSION</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$VERSION</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>https://github.com/$repo/releases/tag/v$VERSION</sparkle:releaseNotesLink>
      <enclosure url="https://github.com/$repo/releases/download/v$VERSION/Sidefleet.dmg" type="application/octet-stream" $attrs/>
    </item>
  </channel>
</rss>
XML
echo dist/appcast.xml
