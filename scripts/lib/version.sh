#!/usr/bin/env bash
# Shared version helpers. Source from other build scripts.

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
    echo "Updated Cargo.toml version: $current -> $new_version"
}

# Applies an optional requested version to Cargo.toml, then returns the resolved version.
resolve_version() {
    local requested="${1:-${VERSION:-}}"
    if [[ -n "$requested" ]]; then
        set_version "$requested"
    fi
    cargo pkgid | sed 's/.*@//'
}
