#!/usr/bin/env bash
# Publishes a GitHub release with DMG asset.
# Usage: scripts/publish-release.sh [version]
# Requires: gh auth login (or GH_TOKEN env var)
set -euo pipefail

cd "$(dirname "$0")/.."
source scripts/lib/version.sh

VERSION="${1:-$(get_version)}"
APP_NAME="小小万年历"
DMG_PATH="dist/${APP_NAME}-${VERSION}.dmg"
TAG="v${VERSION}"
NOTES_FILE="docs/releases/${VERSION}.md"

if ! command -v gh >/dev/null 2>&1; then
    echo "error: gh CLI not found. Install with: brew install gh" >&2
    exit 1
fi

if ! gh auth status >/dev/null 2>&1; then
    echo "error: not logged in to GitHub. Run: gh auth login" >&2
    exit 1
fi

if [[ ! -f "$DMG_PATH" ]]; then
    echo "error: $DMG_PATH not found. Run: scripts/build-dmg.sh --version $VERSION" >&2
    exit 1
fi

if [[ ! -f "$NOTES_FILE" ]]; then
    echo "error: $NOTES_FILE not found" >&2
    exit 1
fi

if ! git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "error: tag $TAG not found. Create with: git tag -a $TAG -m \"Release $VERSION\"" >&2
    exit 1
fi

if ! git ls-remote --exit-code --tags origin "$TAG" >/dev/null 2>&1; then
    echo "Pushing tag $TAG..."
    git push origin "$TAG"
fi

if gh release view "$TAG" >/dev/null 2>&1; then
    echo "Release $TAG already exists. Uploading DMG asset..."
    gh release upload "$TAG" "$DMG_PATH" --clobber
else
    echo "Creating release $TAG..."
    gh release create "$TAG" \
        "$DMG_PATH" \
        --title "小小万年历 ${VERSION}" \
        --notes-file "$NOTES_FILE"
fi

echo "Published: https://github.com/cjhuaxin/tiny-chinese-lunar-calendar/releases/tag/${TAG}"
