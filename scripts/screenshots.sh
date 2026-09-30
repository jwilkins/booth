#!/usr/bin/env bash
#
# Re-take the pictures in the READMEs.
#
# The window is photographed rather than described, so every change to it makes
# the pictures a little more wrong, and there is no way to tell by looking at a
# diff. This is the answer: one command, a fixed collection, and the same shots
# every time, so re-taking them is cheap enough to do whenever the window has
# moved.
#
#     scripts/screenshots.sh
#
# Needs an X server. On Linux `xvfb-run` supplies one and this uses it if it is
# there; on macOS the window opens for a frame and closes again.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

out="${BOOTH_SHOTS:-docs}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "building with the layout-check hooks in"
cargo build --release -p booth --features screenshot
cargo run --release -q -p booth --example demo_library -- "$work" >/dev/null

booth="$root/target/release/booth"
# The window writes its first frame and exits, so each shot is a whole run.
shot() {
    local name="$1"
    shift
    local runner=(env)
    if command -v xvfb-run >/dev/null; then
        runner=(xvfb-run -a -s "-screen 0 1600x1000x24" env)
    fi
    "${runner[@]}" \
        BOOTH_DATA_DIR="$work" \
        EFRAME_SCREENSHOT_TO="$out/$name.png" \
        LIBGL_ALWAYS_SOFTWARE=1 \
        "$@" \
        "$booth" >/dev/null 2>&1 || true
    if [ -s "$out/$name.png" ]; then
        echo "  $out/$name.png"
    else
        echo "  $name FAILED" >&2
        return 1
    fi
}

mkdir -p "$out"
echo "taking them into $out"
shot booth BOOTH_SELECT_FIRST=1
shot booth-sync BOOTH_OPEN_SYNC=1
shot booth-settings BOOTH_OPEN_SETTINGS=1
echo "done"
