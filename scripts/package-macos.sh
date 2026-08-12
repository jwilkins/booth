#!/usr/bin/env bash
#
# Build musicai.app and a disk image to install it from.
#
#   scripts/package-macos.sh [--target-dir DIR]
#
# Produces dist/musicai.app and dist/musicai-<version>.dmg.
#
# Signing is optional and controlled by the environment:
#
#   MUSICAI_SIGN_IDENTITY   A Developer ID Application identity. Without it the
#                           app is ad-hoc signed, which is enough to run on the
#                           machine that built it and not enough for anyone
#                           else's.
#   MUSICAI_NOTARY_PROFILE  A `notarytool` keychain profile. With it, the disk
#                           image is submitted for notarization and stapled, so
#                           that Gatekeeper opens it without argument.
#
# Neither is needed to get a working app on your own machine.

set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
DIST=$REPO/dist
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

if [ "$(uname -s)" != "Darwin" ]; then
	echo "this script builds a macOS app and has to run on macOS" >&2
	echo "(scripts/make-bundle.sh assembles the bundle anywhere, if that is what you want)" >&2
	exit 1
fi

VERSION=$(awk -F'"' '/^version = /{print $2; exit}' "$REPO/gui/Cargo.toml")

echo "==> building musicai $VERSION for ${TARGETS[*]}"
for target in "${TARGETS[@]}"; do
	# Missing targets are the usual first-run failure, and the fix is one
	# command, so say so rather than letting cargo's own error stand alone.
	if ! rustup target list --installed | grep -qx "$target"; then
		echo "missing target $target; run: rustup target add $target" >&2
		exit 1
	fi
	# Both packages are named explicitly. The workspace's default member is
	# the command-line tool alone — that is what keeps `cargo build` free of a
	# window toolkit — so a bare `--bin musicai-gui` is not found.
	cargo build --release --target "$target" -p musicai --bin musicai
	cargo build --release --target "$target" -p musicai-gui --bin musicai-gui
done

# One binary that runs natively on both Apple silicon and Intel. Users should
# not have to know which one they have.
mkdir -p "$DIST/universal"
for binary in musicai musicai-gui; do
	inputs=()
	for target in "${TARGETS[@]}"; do
		inputs+=("$REPO/target/$target/release/$binary")
	done
	lipo -create -output "$DIST/universal/$binary" "${inputs[@]}"
	lipo -info "$DIST/universal/$binary"
done

echo "==> assembling the bundle"
APP=$("$REPO/scripts/make-bundle.sh" \
	"$DIST/universal/musicai-gui" \
	"$DIST/universal/musicai" \
	"$DIST" \
	"$VERSION")

echo "==> signing"
if [ -n "${MUSICAI_SIGN_IDENTITY:-}" ]; then
	# The hardened runtime is what notarization requires. Nothing here loads
	# plugins or JIT-compiles, so no entitlements are needed with it.
	codesign --force --deep --options runtime --timestamp \
		--sign "$MUSICAI_SIGN_IDENTITY" "$APP"
	codesign --verify --strict --verbose=2 "$APP"
else
	# Apple silicon refuses to run an unsigned binary at all, so even a local
	# build needs at least this.
	codesign --force --deep --sign - "$APP"
	echo "    ad-hoc signed: fine on this machine, not distributable"
fi

echo "==> building the disk image"
DMG=$DIST/musicai-$VERSION.dmg
STAGE=$DIST/dmg
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
# The drag-to-install gesture everyone already knows.
ln -s /Applications "$STAGE/Applications"

hdiutil create \
	-volname "musicai" \
	-srcfolder "$STAGE" \
	-ov \
	-format UDZO \
	"$DMG"
rm -rf "$STAGE"

if [ -n "${MUSICAI_NOTARY_PROFILE:-}" ]; then
	echo "==> notarizing (this waits on Apple, and can take a few minutes)"
	xcrun notarytool submit "$DMG" --keychain-profile "$MUSICAI_NOTARY_PROFILE" --wait
	# Stapling puts the ticket in the file, so the first launch works offline.
	xcrun stapler staple "$DMG"
	xcrun stapler validate "$DMG"
else
	echo "==> not notarized"
	echo "    Gatekeeper will refuse this on another Mac until it is."
	echo "    To open it anyway: right-click the app and choose Open."
fi

echo
echo "app: $APP"
echo "dmg: $DMG"
