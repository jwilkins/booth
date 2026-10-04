#!/usr/bin/env bash
#
# Re-take the pictures the website uses.
#
# Same idea as scripts/screenshots.sh, and the same fixed demo collection, with
# two differences: these are rendered at twice the size, because a screenshot on
# a web page is looked at on a display that will happily show every pixel of it;
# and some of them are cut down to the one part of the window a section of the
# page is about.
#
#     scripts/site-media.sh
#
# Needs an X server (xvfb-run on Linux) and Python with Pillow for the crops.
# Writes into site/media/, which is what the pages load.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

out="${BOOTH_SITE_MEDIA:-site/media}"
# Named rather than a mktemp, because the window shows the path of the track it
# is inspecting and a screenshot of somebody's scratch directory looks like a
# mistake. This one reads like a music folder because that is what it is.
work="${BOOTH_SITE_WORK:-$HOME/booth-demo-library}"

echo "building with the layout-check hooks in"
cargo build --release -q -p booth --features screenshot
rm -rf "$work"
cargo run --release -q -p booth --example demo_library -- "$work" >/dev/null

booth="$root/target/release/booth"
mkdir -p "$out"

# Twice the window's own size. winit is told the display is a 2x one and the
# X server is made big enough to hold the result; eframe then renders every
# glyph and every waveform at that scale rather than stretching a small one.
shot() {
    local name="$1"
    shift
    local runner=(env)
    if command -v xvfb-run >/dev/null; then
        runner=(xvfb-run -a -s "-screen 0 3400x2100x24" env)
    fi
    "${runner[@]}" \
        BOOTH_DATA_DIR="$work" \
        EFRAME_SCREENSHOT_TO="$out/$name.png" \
        LIBGL_ALWAYS_SOFTWARE=1 \
        WINIT_X11_SCALE_FACTOR=2 \
        "$@" \
        "$booth" >/dev/null 2>&1 || true
    if [ -s "$out/$name.png" ]; then
        echo "  $out/$name.png"
    else
        echo "  $name FAILED" >&2
        return 1
    fi
}

echo "taking them into $out"
shot booth BOOTH_SELECT_FIRST=1
shot booth-sync BOOTH_OPEN_SYNC=1
shot booth-words BOOTH_SELECT_FIRST=1 BOOTH_OPEN_SHEET=words
shot booth-stems-ahead BOOTH_SELECT_FIRST=1 BOOTH_OPEN_SHEET=stems

# The crops. Each one is a region of the whole-window shot above, in that
# picture's own pixels, so moving a panel means re-taking the shot and nothing
# else. A section of the page that is about one panel gets that panel.
echo "cutting the crops"
python3 - "$out" <<'PY'
import glob
import os
import sys

from PIL import Image

out = sys.argv[1]
window = Image.open(f"{out}/booth.png")
w, h = window.size

# Fractions rather than pixels: the window is rendered at whatever scale the
# machine taking the shots ends up with, and these follow it.
regions = {
    # The inspector's "what it keeps saying" panel: the lines a track comes
    # back to, how often, and where each one lands.
    "words-panel": (0.826, 0.606, 1.0, 0.790),
    # The waveform with its cue markers, the beat ruler and the phrase bar.
    "waveform-phrases": (0.148, 0.640, 0.828, 0.925),
    # The drive dock along the bottom.
    "dock": (0.0, 0.948, 1.0, 1.0),
}
for name, (x0, y0, x1, y1) in regions.items():
    box = (round(x0 * w), round(y0 * h), round(x1 * w), round(y1 * h))
    window.crop(box).save(f"{out}/{name}.png")
    print(f"  {out}/{name}.png")

# Lossless, and about a fifth off each file. A screenshot of a window full of
# small text is one of the few things on a web page that genuinely wants a PNG,
# so the way to make it smaller is to pack it better rather than to throw pixels
# or colours away.
for file in sorted(glob.glob(f"{out}/*.png")):
    was = os.path.getsize(file)
    Image.open(file).save(file, optimize=True)
    print(f"  {file} {was // 1024} -> {os.path.getsize(file) // 1024} KB")
PY

echo "done"
