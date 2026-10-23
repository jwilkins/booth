#!/usr/bin/env sh
# Measure how long a drive takes to write, add to and read back, and put the
# numbers where the site reads them.
#
#   scripts/speed.sh                         # the host's temp dir, 25/100/400 tracks
#   scripts/speed.sh /Volumes/STICK          # a real stick, which is the number that counts
#   BOOTH_SPEED_SIZES=50,1000 scripts/speed.sh
#
# The measurement is the #[ignore]d test in proto/musicai/tests/speed.rs, run
# in release — a debug build is several times slower and the page says which it
# was, but a release number is the one worth publishing. The result lands in
# site/speed.json; commit it and the Pages workflow publishes it.
set -eu

cd "$(dirname "$0")/.."

if [ "${1:-}" != "" ]; then
    export BOOTH_SPEED_DRIVE="$1"
    export BOOTH_SPEED_MEDIUM="${BOOTH_SPEED_MEDIUM:-$(df -h "$1" 2>/dev/null | awk 'NR==2 {print $1}') mounted at $1}"
fi

export BOOTH_SPEED_OUT="${BOOTH_SPEED_OUT:-site/speed.json}"

cargo test -p booth-cli --release --test speed -- --ignored measure --nocapture
echo "results in $BOOTH_SPEED_OUT"
