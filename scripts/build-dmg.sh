#!/usr/bin/env bash
# Builds 小小万年历.dmg for distribution.
# Usage: scripts/build-dmg.sh [--version X.Y.Z] [--skip-app-build]
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib/version.sh

APP_NAME="小小万年历"
APP_DIR="dist/${APP_NAME}.app"
STAGING_DIR="dist/dmg-staging"

REQUESTED_VERSION=""
SKIP_APP_BUILD=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            REQUESTED_VERSION="${2:-}"
            if [[ -z "$REQUESTED_VERSION" ]]; then
                echo "error: --version requires a value" >&2
                exit 1
            fi
            shift 2
            ;;
        --skip-app-build)
            SKIP_APP_BUILD=true
            shift
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            echo "usage: scripts/build-dmg.sh [--version X.Y.Z] [--skip-app-build]" >&2
            exit 1
            ;;
    esac
done

if [[ "$SKIP_APP_BUILD" == true && -n "${REQUESTED_VERSION:-${VERSION:-}}" ]]; then
    echo "error: --version cannot be used with --skip-app-build" >&2
    exit 1
fi

VERSION="$(resolve_version "$REQUESTED_VERSION")"
DMG_PATH="dist/${APP_NAME}-${VERSION}.dmg"

if [[ "$SKIP_APP_BUILD" == false ]]; then
    bash scripts/build-app.sh ${REQUESTED_VERSION:+--version "$REQUESTED_VERSION"}
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
