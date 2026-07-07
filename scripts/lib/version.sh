#!/usr/bin/env bash
# Shared version helpers. Source from other build scripts.

# ASCII-only DMG basename for GitHub releases (matches 0.1.1 naming).
RELEASE_DMG_NAME="xiaoxiao-wannianli"

release_dmg_path() {
    local version="$1"
    echo "dist/${RELEASE_DMG_NAME}-${version}.dmg"
}

get_version() {
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1
}

set_version() {
    local new_version="$1"

    if ! [[ "$new_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "error: invalid version: $new_version (expected semver like 1.2.3)" >&2
        return 1
    fi

    local current
    current="$(get_version)"
    if [[ "$current" == "$new_version" ]]; then
        return 0
    fi

    sed -i '' "s/^version = \".*\"/version = \"${new_version}\"/" Cargo.toml
    echo "Updated Cargo.toml version: $current -> $new_version" >&2
}

# Applies an optional requested version to Cargo.toml, then returns the resolved version.
resolve_version() {
    local requested="${1:-${VERSION:-}}"
    if [[ -n "$requested" ]]; then
        set_version "$requested"
    fi
    cargo pkgid | sed 's/.*@//'
}

# Maps semver X.Y.Z to a monotonically increasing CFBundleVersion / sparkle:version.
semver_to_build_number() {
    local v="$1"
    local major minor patch
    IFS='.' read -r major minor patch <<< "$v"
    echo $(( major * 10000 + minor * 100 + patch ))
}

# Returns the repository's default branch (e.g. main).
get_default_branch() {
    local branch
    branch="$(git rev-parse --abbrev-ref origin/HEAD 2>/dev/null | sed 's|^origin/||')"
    echo "${branch:-main}"
}

appcast_feed_url() {
    local repo="${1:-cjhuaxin/tiny-chinese-lunar-calendar}"
    echo "https://raw.githubusercontent.com/${repo}/$(get_default_branch)/appcast/appcast.xml"
}

# Returns the release tag immediately before v{version}, or empty if none.
previous_release_tag() {
    local version="$1"
    local target_tag="v${version}"
    local prev_tag=""

    while IFS= read -r tag; do
        if [[ "$tag" == "$target_tag" ]]; then
            break
        fi
        prev_tag="$tag"
    done < <(git tag -l 'v*' --sort=version:refname)

    echo "$prev_tag"
}
