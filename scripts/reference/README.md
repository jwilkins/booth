# Reference beats

Two beat trackers that are not Booth's, kept to measure Booth's against.

- **madmom** 0.16.1: a recurrent network and a bar-aware dynamic Bayesian
  network (`DBNDownBeatTracker`). It gives the beats and where the bar starts.
- **Essentia** 2.1b6: `RhythmExtractor2013`, which votes several onset
  functions into a tempo and tracks beats from it.

Booth tracks with a dynamic program after Ellis (2007), so librosa's tracker,
the same algorithm, would mostly repeat Booth's mistakes and is not used.
Neither of these is a listener. Where they agree with each other and not with
Booth, Booth has to answer for it. Where they disagree, nothing is counted.

## The environment

madmom's last release builds on Python 3.8 and nothing later: its C sources
use `tp_print`, which 3.9 removed, and 3.11 has no `longintrepr.h`. It also
needs numpy below 1.24, scipy below 1.11 and Cython below 3, and pip and uv
build in an isolated environment of their own that fetches the newest of each.
So:

```sh
uv python install 3.8
uv venv --python 3.8 ~/.cache/booth/reference
uv pip install --python ~/.cache/booth/reference/bin/python -r scripts/reference/requirements.txt
uv pip install --python ~/.cache/booth/reference/bin/python --no-build-isolation madmom==0.16.1
```

## Running it

```sh
~/.cache/booth/reference/bin/python -I scripts/reference/beats.py <records> <beats>
```

This writes `<beats>/<record>.json` for each record. It takes about a quarter
of the length of the record, and a record already done is skipped. Then:

```sh
BOOTH_REFERENCE_RECORDS=<records> BOOTH_REFERENCE_BEATS=<beats> \
    cargo test --release -p booth-cli --test reference_beats -- --ignored --nocapture
```

The test prints each record's tempo by all three, what Booth's is against the
others ("same", "half", "3/2" and so on), how many of madmom's beats have one
of Booth's on them, and where madmom's ones fall in Booth's bar. It fails where
madmom and Essentia agree on a tempo and Booth's is a different one, except on
the records it names as known misses, and it says when one of those comes
right. A second test does the same with the genre model, where
`BOOTH_GENRE_MODEL` or `~/.cache/booth/models/` has one, analysing each record
as the app does with the genre switched on (the model, and else the file's
genre tag), and says which records a genre moved an octave and from what. It takes about fifteen seconds a record more.
