#!/bin/bash
#
# What this repository needs from the machine it is built on, fetched once
# per container.
#
# Three tiers, and they are deliberately different about failure. The build
# dependencies are not optional: without ALSA's headers `alsa-sys` does not
# compile and nothing in the workspace builds, so those are installed first
# and loudly. The speech recogniser is optional — only cueing a track from its
# words needs it, every test around that runs off fixtures — so it is fetched
# best-effort and a session where the download failed is a session that still
# works, minus one feature. The stem separator is not fetched at all; see
# below.
#
# Nothing lands in the working tree. The recogniser and its weights go to a
# cache directory, and where they went is written into the session's
# environment, which is what `booth::config::Whisper` already reads when its
# settings are empty.

set -uo pipefail

# The web only. On a developer's own machine this would be installing system
# packages behind their back, which is not a hook's business.
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

say() { printf '  %s\n' "$*"; }

CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/booth"
mkdir -p "$CACHE"

# -- what the workspace will not compile without --------------------------

# `cpal` reaches ALSA through `alsa-sys`, which is a `pkg-config` build script
# and not a vendored copy: with no `alsa.pc` on the machine the build script
# fails and takes the whole workspace with it, several crates before anything
# says the word "audio".
if pkg-config --exists alsa 2>/dev/null; then
  say "alsa headers already here"
else
  say "installing the build dependencies"
  export DEBIAN_FRONTEND=noninteractive
  if apt-get update -qq >/dev/null 2>&1 &&
    apt-get install -y -qq --no-install-recommends libasound2-dev pkg-config >/dev/null 2>&1; then
    say "installed libasound2-dev"
  else
    # Not fatal here, but it will be fatal at the first `cargo build`, so say
    # which one it was rather than leaving a build script's error to explain
    # itself later.
    say "WARNING: could not install libasound2-dev — cargo will fail on alsa-sys"
  fi
fi

# -- the speech recogniser, for cueing a track from its words --------------

# Skippable, because it is a source build and a download of a hundred-odd
# megabytes, and a session that is only going to run the test suite needs
# neither.
if [ "${BOOTH_SKIP_WHISPER:-}" = "1" ]; then
  say "skipping whisper (BOOTH_SKIP_WHISPER=1)"
  exit 0
fi

# whisper.cpp rather than OpenAI's: it needs no Python and no ffmpeg, it reads
# the 16 kHz wav the exporter hands it directly, and it runs offline once its
# weights are on disk. See booth/README.md, "Installing a recogniser".
WHISPER_DIR="$CACHE/whisper.cpp"
WHISPER_BIN="$WHISPER_DIR/build/bin/whisper-cli"

if [ -x "$WHISPER_BIN" ]; then
  say "whisper-cli already built"
else
  say "building whisper.cpp (a few minutes, once per container)"
  if [ ! -d "$WHISPER_DIR/.git" ]; then
    # Shallow: the history is of no use here and the clone is most of the
    # wait.
    git clone --depth 1 -q https://github.com/ggml-org/whisper.cpp "$WHISPER_DIR" 2>/dev/null
  fi
  if [ -d "$WHISPER_DIR" ]; then
    # Only the one target. Building everything means the examples, the server
    # and the test suite, none of which anything here calls.
    cmake -S "$WHISPER_DIR" -B "$WHISPER_DIR/build" -DCMAKE_BUILD_TYPE=Release \
      -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON >/dev/null 2>&1 &&
      cmake --build "$WHISPER_DIR/build" --target whisper-cli -j "$(nproc)" >/dev/null 2>&1
  fi
  if [ -x "$WHISPER_BIN" ]; then
    say "built whisper-cli"
  else
    say "WARNING: whisper.cpp did not build — cueing from the words will be unavailable"
  fi
fi

# The weights. `base.en` is what booth/README.md names as a good first choice:
# small enough to fetch in a moment, good enough to make out a sung line.
# Anything ggml-formatted works; the name is overridable for trying a bigger
# one against a stubborn record.
MODEL_NAME="${BOOTH_WHISPER_MODEL_NAME:-ggml-base.en.bin}"
MODEL="$CACHE/models/$MODEL_NAME"
mkdir -p "$CACHE/models"

if [ -s "$MODEL" ]; then
  say "$MODEL_NAME already here"
else
  say "fetching $MODEL_NAME"
  # To a part file first, moved into place only on success, so an interrupted
  # download cannot leave something that looks like a model behind and get
  # skipped on the next run.
  if curl -fsSL --retry 3 --retry-delay 2 \
    -o "$MODEL.part" \
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$MODEL_NAME" 2>/dev/null; then
    mv "$MODEL.part" "$MODEL"
    say "fetched $MODEL_NAME ($(du -h "$MODEL" | cut -f1))"
  else
    rm -f "$MODEL.part"
    say "WARNING: could not fetch $MODEL_NAME — whisper.cpp will not run without a model"
  fi
fi

# -- tell the session where they went --------------------------------------

# `Whisper::program` and `Whisper::model` fall back to these when the settings
# are empty, so nothing has to be typed into the window to try the feature.
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  [ -x "$WHISPER_BIN" ] && echo "export BOOTH_WHISPER_BIN=\"$WHISPER_BIN\"" >>"$CLAUDE_ENV_FILE"
  [ -s "$MODEL" ] && echo "export BOOTH_WHISPER_MODEL=\"$MODEL\"" >>"$CLAUDE_ENV_FILE"
  # A library is one language far more often than it is many, and a recogniser
  # left to guess takes its guess from the first few seconds of an isolated
  # vocal, which are usually a breath.
  echo 'export BOOTH_WHISPER_LANGUAGE="en"' >>"$CLAUDE_ENV_FILE"
fi

# -- what is deliberately not here -----------------------------------------
#
# demucs. Rendering stems needs it, and it pulls torch: gigabytes, minutes,
# and a wait on every cold container for something no test exercises. Install
# it by hand in a session that needs one:
#
#     pipx install demucs --preinstall numpy
#
# The numpy is not optional — demucs imports it without declaring it, so a
# clean install of demucs 4.1.0 cannot run. `stems/demucs.rs` recognises the
# resulting traceback and says so.
#
# ffmpeg, for the same kind of reason: only OpenAI's `whisper` shells out to
# it, and that is not the one this fetches.

exit 0
