#!/usr/bin/env bash
# Downloads the comparison servers into servers/ (git-ignored):
#   servers/vanilla-<ver>/server.jar       from Mojang's version manifest, sha1-checked
#   servers/neoforge-<ver>/                NeoForge installer run with --install-server
# Usage: scripts/fetch-servers.sh [vanilla] [neoforge]   (default: both)
set -euo pipefail

MC_VERSION="${MC_VERSION:-26.3}"
NEOFORGE_VERSION="${NEOFORGE_VERSION:-26.3.0.40-beta}"
JAVA="${JAVA:-java}"

here="$(cd "$(dirname "$0")/.." && pwd)"
servers="$here/servers"
mkdir -p "$servers"

sha1() { shasum -a 1 "$1" | cut -d' ' -f1; }

fetch_vanilla() {
    local dir="$servers/vanilla-$MC_VERSION"
    mkdir -p "$dir"
    local manifest version_url server_url server_sha
    manifest="$(curl -fsSL https://piston-meta.mojang.com/mc/game/version_manifest_v2.json)"
    version_url="$(python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(v["url"] for v in m["versions"] if v["id"]==sys.argv[1]))' "$MC_VERSION" <<<"$manifest")"
    read -r server_url server_sha < <(curl -fsSL "$version_url" | python3 -c 'import json,sys; d=json.load(sys.stdin)["downloads"]["server"]; print(d["url"], d["sha1"])')
    if [[ -f "$dir/server.jar" && "$(sha1 "$dir/server.jar")" == "$server_sha" ]]; then
        echo "vanilla $MC_VERSION already present"
        return
    fi
    curl -fsSL -o "$dir/server.jar" "$server_url"
    if [[ "$(sha1 "$dir/server.jar")" != "$server_sha" ]]; then
        echo "vanilla server.jar sha1 mismatch" >&2
        exit 1
    fi
    echo "vanilla $MC_VERSION -> $dir/server.jar"
}

fetch_neoforge() {
    local dir="$servers/neoforge-$NEOFORGE_VERSION"
    if [[ -d "$dir/libraries/net/neoforged/neoforge/$NEOFORGE_VERSION" ]]; then
        echo "neoforge $NEOFORGE_VERSION already installed"
        return
    fi
    mkdir -p "$dir"
    local base="https://maven.neoforged.net/releases/net/neoforged/neoforge/$NEOFORGE_VERSION"
    local installer="$dir/installer.jar"
    curl -fsSL -o "$installer" "$base/neoforge-$NEOFORGE_VERSION-installer.jar"
    local want
    want="$(curl -fsSL "$base/neoforge-$NEOFORGE_VERSION-installer.jar.sha1")"
    if [[ "$(sha1 "$installer")" != "$want" ]]; then
        echo "neoforge installer sha1 mismatch" >&2
        exit 1
    fi
    (cd "$dir" && "$JAVA" -jar installer.jar --install-server "$dir" >installer.log 2>&1) || {
        echo "neoforge installer failed, see $dir/installer.log" >&2
        exit 1
    }
    echo "neoforge $NEOFORGE_VERSION -> $dir"
}

targets=("$@")
[[ ${#targets[@]} -eq 0 ]] && targets=(vanilla neoforge)
for t in "${targets[@]}"; do
    case "$t" in
        vanilla) fetch_vanilla ;;
        neoforge) fetch_neoforge ;;
        *) echo "unknown target $t" >&2; exit 2 ;;
    esac
done
