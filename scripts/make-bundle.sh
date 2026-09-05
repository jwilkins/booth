#!/usr/bin/env bash
#
# Assemble Booth.app from binaries that have already been built.
#
#   scripts/make-bundle.sh <booth-binary> <cli-binary> <output-dir> [version]
#
# This is only file copying and a plist substitution, so it runs anywhere. That
# is deliberate: the bundle layout is the part worth checking, and keeping it
# free of macOS-only tools means it can be checked without a Mac.
# package-macos.sh does the parts that genuinely need one.

set -euo pipefail

if [ "$#" -lt 3 ]; then
	echo "usage: $0 <booth-binary> <cli-binary> <output-dir> [version]" >&2
	exit 2
fi

BOOTH_BIN=$1
CLI_BIN=$2
OUT_DIR=$3
VERSION=${4:-0.1.0}

REPO=$(cd "$(dirname "$0")/.." && pwd)
RESOURCES=$REPO/packaging/macos
APP=$OUT_DIR/Booth.app

for binary in "$BOOTH_BIN" "$CLI_BIN"; do
	if [ ! -f "$binary" ]; then
		echo "no such binary: $binary" >&2
		exit 1
	fi
done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# The library window is what launching the app runs. The command-line tool
# rides along in the same bundle so that one download gives you both, and so the
# two can never be different versions of each other.
install -m 755 "$BOOTH_BIN" "$APP/Contents/MacOS/booth"
install -m 755 "$CLI_BIN" "$APP/Contents/MacOS/booth-cli"

install -m 644 "$RESOURCES/icon.icns" "$APP/Contents/Resources/icon.icns"

sed "s/__VERSION__/$VERSION/g" "$RESOURCES/Info.plist" >"$APP/Contents/Info.plist"

# Ancient, and still what Finder looks at to decide a directory is an app.
printf 'APPL????' >"$APP/Contents/PkgInfo"

echo "$APP"
