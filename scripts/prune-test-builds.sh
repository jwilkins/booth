#!/bin/sh
#
# Removes the test builds that a newer build of the same test has replaced.
#
#     scripts/prune-test-builds.sh          # prune target/ now
#     scripts/prune-test-builds.sh --hook   # the same, in front of a build
#
# Cargo never removes a build. It keys each one by everything that went into
# it, so `cargo test -p booth` and `cargo test --workspace` unify features
# differently and link a second copy of booth's tests, `CARGO_INCREMENTAL=0`
# changes the profile and links a third, and the older ones stay. Each copy of
# a test here carries every dependency's debug info: 400 to 730 MB apiece. One
# day of builds took a cloud session's target directory to 26 GB and filled its
# disk allowance.
#
# What is old, exactly: every test executable in `deps/` has a fingerprint
# directory under `.fingerprint/` with the same hash, and that directory names
# the package and the unit — `booth/test-lib-booth`,
# `booth-cli/test-integration-test-word_times`. Of the builds of one unit,
# the newest is kept and the others are removed, with their dep-info and their
# fingerprint. Binaries are not tests and are left alone, and so is anything
# cargo did not leave a fingerprint for.
#
# Switching back to a way of building whose copy was removed rebuilds that one
# test crate, not its dependencies: their libraries are kept.
#
# Nothing is removed while cargo is running. Cargo lets go of its lock once it
# has compiled, and runs the tests after, so a run in the background would find
# a binary it was about to start gone.
#
# With `--hook` it reads a Claude Code tool call on stdin and does nothing
# unless the call runs a cargo command that builds, and it says what it removed
# as the hook's message. Either way it exits 0: a hook that exits 2 stops the
# command it was in front of.

if [ "${1:-}" = "--hook" ]; then
  hook=1
  # The tool input is JSON, and a command mentioning cargo is all that has to
  # be recognised in it.
  cat | grep -Eq '(^|[^[:alnum:]_-])cargo( +\+[^ ]+)? +(build|b|test|t|bench|run|r|nextest|clippy|check|c)([^[:alnum:]_-]|$)' ||
    exit 0
else
  hook=0
fi

if command -v pgrep >/dev/null 2>&1 && pgrep -x cargo >/dev/null 2>&1; then
  [ "$hook" = 1 ] || echo "cargo is running; nothing removed"
  exit 0
fi

root=${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
target=${CARGO_TARGET_DIR:-$root/target}

# Newlines only, so that a checkout under a directory with a space in its name
# still splits into paths.
IFS='
'
removed=0
freed_kb=0

# `debug` and `release`, and the same under a target triple for a cross build.
for fingerprints in "$target"/*/.fingerprint "$target"/*/*/.fingerprint; do
  [ -d "$fingerprints" ] || continue
  deps=${fingerprints%/.fingerprint}/deps
  [ -d "$deps" ] || continue

  tests=$(find "$deps" -maxdepth 1 -type f -perm -u+x -name '*-????????????????')
  [ -n "$tests" ] || continue

  # Newest first, so the first build seen of a unit is the one to keep.
  kept=' '
  for test in $(ls -t $tests); do
    hash=${test##*-}
    unit=$(ls -d "$fingerprints"/*-"$hash" 2>/dev/null | head -n 1)
    [ -n "$unit" ] || continue
    kind=$(ls "$unit" | grep -E '^(test|bench)-' | grep -v '\.json$' | head -n 1)
    [ -n "$kind" ] || continue
    package=$(basename "$unit")
    key="${package%-"$hash"}/$kind"
    case $kept in
      *" $key "*) ;;
      *)
        kept="$kept$key "
        continue
        ;;
    esac

    size=$(du -k "$test" | cut -f1)
    rm -rf "$test" "$test".* "$unit"
    removed=$((removed + 1))
    freed_kb=$((freed_kb + size))
  done
done

builds=builds
[ "$removed" = 1 ] && builds=build
said="Removed $removed old test $builds ($((freed_kb / 1024)) MB)."
if [ "$hook" = 0 ]; then
  echo "$said"
elif [ "$removed" -gt 0 ]; then
  printf '{"systemMessage": "%s"}\n' "$said"
fi

exit 0
