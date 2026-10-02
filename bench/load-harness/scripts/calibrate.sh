#!/usr/bin/env bash
# Calibration sweep: every target, every bot count, REPEATS times, one server at a time.
# Targets are interleaved within each repeat so slow drift on the host (thermals, background
# load) hits every target alike instead of biasing whichever ran last.
#
# Usage: scripts/calibrate.sh <out-dir> [targets...]
#   env: BOTS="10 50" REPEATS=3 WARMUP=60 MEASURE=120 HEAP=2G COOLDOWN=15
#        PUMPKIN_BIN=../../target/release/pumpkin PUMPKIN_LABEL=<label> WORK_ROOT=<scratch dir>
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:?usage: calibrate.sh <out-dir> [targets...]}"
shift
targets=("$@")
[[ ${#targets[@]} -eq 0 ]] && targets=(vanilla neoforge pumpkin)

BOTS="${BOTS:-10 50}"
REPEATS="${REPEATS:-3}"
WARMUP="${WARMUP:-60}"
MEASURE="${MEASURE:-120}"
HEAP="${HEAP:-2G}"
COOLDOWN="${COOLDOWN:-15}"
MC_VERSION="${MC_VERSION:-26.3}"
NEOFORGE_VERSION="${NEOFORGE_VERSION:-26.3.0.40-beta}"
PUMPKIN_BIN="${PUMPKIN_BIN:-$here/../../target/release/pumpkin}"
PUMPKIN_LABEL="${PUMPKIN_LABEL:-pumpkin $(git -C "$here" rev-parse --short HEAD) release}"

harness="$here/target/release/mc-load-harness"
[[ -x "$harness" ]] || (cd "$here" && cargo build --release --locked)

server_for() {
    case "$1" in
        vanilla) echo "$here/servers/vanilla-$MC_VERSION/server.jar" ;;
        neoforge) echo "$here/servers/neoforge-$NEOFORGE_VERSION" ;;
        pumpkin) echo "$PUMPKIN_BIN" ;;
    esac
}
label_for() {
    case "$1" in
        vanilla) echo "vanilla $MC_VERSION" ;;
        neoforge) echo "neoforge $NEOFORGE_VERSION" ;;
        pumpkin) echo "$PUMPKIN_LABEL" ;;
    esac
}

mkdir -p "$out/raw" "$out/logs"
for rep in $(seq 1 "$REPEATS"); do
    for bots in $BOTS; do
        for t in "${targets[@]}"; do
            echo "=== repeat $rep/$REPEATS, $t, $bots bots ==="
            "$harness" run --target "$t" --server "$(server_for "$t")" --label "$(label_for "$t")" \
                --bots "$bots" --warmup-secs "$WARMUP" --measure-secs "$MEASURE" --java-heap "$HEAP" \
                --work-root "${WORK_ROOT:-$out/runs}" --out-dir "$out/raw" \
                2>&1 | tee -a "$out/logs/calibrate.log" || echo "run failed: $t $bots bots (repeat $rep)" | tee -a "$out/logs/calibrate.log"
            sleep "$COOLDOWN"
        done
    done
done

"$harness" report "$out/raw" --out "$out/report"
