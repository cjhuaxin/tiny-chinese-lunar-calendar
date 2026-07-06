#!/usr/bin/env bash
# Builds 小小万年历.dmg for distribution.
# Usage: scripts/build-dmg.sh [--skip-app-build]
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="小小万年历"
VERSION="0.1.1"
APP_DIR="dist/${APP_NAME}.app"
DMG_PATH="dist/${APP_NAME}-${VERSION}.dmg"
STAGING_DIR="dist/dmg-staging"

if [[ "${1:-}" != "--skip-app-build" ]]; then
    bash scripts/build-app.sh
fi

if [[ ! -d "$APP_DIR" ]]; then
    echo "error: $APP_DIR not found; run scripts/build-app.sh first" >&2
    exit 1
fi

# Stage the .app next to an /Applications symlink so users can drag-install.
rm -rf "$STAGING_DIR" "$DMG_PATH"
mkdir -p "$STAGING_DIR"
cp -R "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"

hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$STAGING_DIR" \
    -ov \
    -format UDZO \
    -imagekey zlib-level=9 \
    "$DMG_PATH"

rm -rf "$STAGING_DIR"

echo "Built $DMG_PATH"
du -sh "$DMG_PATH"
