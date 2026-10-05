# Working on Booth

## A branch per set of work

**Start a new branch for each set of work, and open the pull request when that
set is complete.** Not a branch per session and not a pull request per commit:
one branch for the thing being asked for, however many commits that takes, and
one pull request when it is finished.

Why it is per set rather than per commit: a shared long-lived branch means
every push lands in whatever pull request happens to be open, and a merge that
arrives mid-set splits the work across two of them. That happened three times
in one morning — grid tools landing in a pull request about memory cues, a
start-cue fix missing the merge it was written for — and each time the fix was
a new pull request for commits that should never have been separated.

**Always open the pull request once the set is done.** Not when asked to. A
branch pushed without one is work nobody has been told about, and the default
is to propose it rather than to wait to be asked.

Merging is still the owner's call unless they say otherwise.

Before the pull request goes up, all three of these pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
env -u BOOTH_WHISPER_MODEL -u BOOTH_WHISPER_BIN cargo test --workspace
```

The environment variables are unset deliberately. A developer's machine exports
`BOOTH_WHISPER_MODEL` from the session-start hook and CI does not, so a test
that reads it passes here and fails there — which has happened, for four pushes
running. See `config::Whisper::chosen`.

## What a pull request body says

What changed and **why it is right**, not a list of files. Where a decision
could have gone the other way, say what the other way was and why it lost.

End with what is worth a second opinion: the thresholds picked by reasoning
rather than measurement, the claim that rests on one observation, the thing
that will be wrong first. A body with nothing in that section is usually a body
that has not been read back properly.

## Measuring rather than reasoning

Where a service, a player or a file format can be asked directly, ask it, and
put the answer in the commit message or the doc comment. The live-service tests
in `proto/musicai/tests/live_services.rs` are `#[ignore]`d and run on purpose;
adding one is how a measurement gets kept.

Two faults in the lyrics lookup survived a full round of review and shipped,
because both were reasoned about and neither was tried against the real
service. One request would have caught either.

## Comments

Comments say **why**, not what. The reader can see what the code does.

Test names are sentences that state the claim the test makes.
