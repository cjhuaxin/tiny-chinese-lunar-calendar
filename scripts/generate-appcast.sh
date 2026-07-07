#!/usr/bin/env bash
# Generates or updates appcast/appcast.xml for Sparkle auto-updates.
# Usage: scripts/generate-appcast.sh [version]
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib/version.sh

VERSION="${1:-$(get_version)}"
TAG="v${VERSION}"
BUILD_NUMBER="$(semver_to_build_number "$VERSION")"
ZIP_NAME="${RELEASE_DMG_NAME}-${VERSION}.zip"
ZIP_PATH="dist/${ZIP_NAME}"
APPCAST_FILE="appcast/appcast.xml"
PRIVATE_KEY_FILE="sparkle_private_key.txt"
GITHUB_REPO="cjhuaxin/tiny-chinese-lunar-calendar"
RELEASE_URL="https://github.com/${GITHUB_REPO}/releases/tag/${TAG}"
DOWNLOAD_URL="https://github.com/${GITHUB_REPO}/releases/download/${TAG}/${ZIP_NAME}"
NOTES_FILE="docs/releases/${VERSION}.md"

if [[ ! -x "sparkle-bin/sign_update" ]]; then
    echo "error: sparkle-bin/sign_update not found. Run: scripts/download-sparkle.sh" >&2
    exit 1
fi

if [[ ! -f "$ZIP_PATH" ]]; then
    echo "error: $ZIP_PATH not found. Run: scripts/build-update-zip.sh --version $VERSION" >&2
    exit 1
fi

if [[ ! -f "$PRIVATE_KEY_FILE" ]]; then
    echo "error: $PRIVATE_KEY_FILE not found. Run: ./sparkle-bin/generate_keys -x sparkle_private_key.txt" >&2
    exit 1
fi

SIGNATURE_LINE="$(./sparkle-bin/sign_update "$ZIP_PATH" -f "$PRIVATE_KEY_FILE")"
ED_SIGNATURE="$(echo "$SIGNATURE_LINE" | sed -n 's/.*sparkle:edSignature="\([^"]*\)".*/\1/p')"
FILE_LENGTH="$(echo "$SIGNATURE_LINE" | sed -n 's/.*length="\([^"]*\)".*/\1/p')"

if [[ -z "$ED_SIGNATURE" || -z "$FILE_LENGTH" ]]; then
    echo "error: failed to parse sign_update output: $SIGNATURE_LINE" >&2
    exit 1
fi

PUB_DATE="$(date -u '+%a, %d %b %Y %H:%M:%S +0000')"

mkdir -p appcast
cat > "$APPCAST_FILE" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:dc="http://purl.org/dc/elements/1.1/">
    <channel>
        <title>小小万年历</title>
        <link>${RELEASE_URL}</link>
        <description>Most recent updates to 小小万年历</description>
        <language>zh-cn</language>
        <item>
            <title>Version ${VERSION}</title>
            <link>${RELEASE_URL}</link>
            <sparkle:version>${BUILD_NUMBER}</sparkle:version>
            <sparkle:shortVersionString>${VERSION}</sparkle:shortVersionString>
            <sparkle:releaseNotesLink>${RELEASE_URL}</sparkle:releaseNotesLink>
            <pubDate>${PUB_DATE}</pubDate>
            <enclosure
                url="${DOWNLOAD_URL}"
                sparkle:edSignature="${ED_SIGNATURE}"
                length="${FILE_LENGTH}"
                type="application/octet-stream" />
        </item>
    </channel>
</rss>
EOF

echo "Generated $APPCAST_FILE"
echo "  version: ${VERSION} (build ${BUILD_NUMBER})"
echo "  enclosure: ${DOWNLOAD_URL}"
