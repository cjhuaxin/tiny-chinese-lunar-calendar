#!/usr/bin/env bash
# Creates a Sparkle update zip from the built .app bundle.
# Usage: scripts/build-update-zip.sh [--version X.Y.Z]
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib/version.sh

APP_NAME="小小万年历"

REQUESTED_VERSION=""
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
        *)
            echo "error: unknown argument: $1" >&2
            echo "usage: scripts/build-update-zip.sh [--version X.Y.Z]" >&2
            exit 1
            ;;
    esac
done

VERSION="$(resolve_version "$REQUESTED_VERSION")"
APP_DIR="dist/${APP_NAME}.app"
ZIP_PATH="dist/${RELEASE_DMG_NAME}-${VERSION}.zip"

if [[ ! -d "$APP_DIR" ]]; then
    echo "error: $APP_DIR not found. Run: scripts/build-app.sh --version $VERSION" >&2
    exit 1
fi

rm -f "$ZIP_PATH"
ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$ZIP_PATH"

echo "Built $ZIP_PATH"
