"""Beats from two trackers that are not Booth's, for every record in a folder.

    python -I scripts/reference/beats.py <records> <out>

writes <out>/<record>.json for each .mp3, .flac or .wav in <records>:

    {"madmom":   {"version": ..., "beats": [[seconds, 1-4], ...]},
     "essentia": {"version": ..., "bpm": ..., "beats": [seconds, ...]}}

madmom's is a recurrent network and a bar-aware dynamic Bayesian network
(DBNDownBeatTracker), so it says where the bar starts as well. Essentia's
RhythmExtractor2013 votes several onset functions into a tempo and tracks from
that. Neither is the dynamic program Booth uses, so where they agree with each
other and not with Booth, that is Booth's to answer for.

See scripts/reference/README.md for the environment. A record already done is
skipped, so a run that stops can be started again.
"""

import json
import os
import sys

import essentia
import essentia.standard as es
import madmom
from madmom.features.downbeats import DBNDownBeatTrackingProcessor, RNNDownBeatProcessor

AUDIO = (".mp3", ".flac", ".wav")


def main(records, out):
    # It warns over three thousand times a record about a network it was never
    # asked to build.
    essentia.log.infoActive = False
    essentia.log.warningActive = False
    os.makedirs(out, exist_ok=True)
    rnn = RNNDownBeatProcessor()
    # Four to a bar only, as Booth assumes: the question is where in a bar of
    # four the one is, not whether there are four.
    dbn = DBNDownBeatTrackingProcessor(beats_per_bar=[4], fps=100)
    rhythm = es.RhythmExtractor2013(method="multifeature")
    for name in sorted(os.listdir(records)):
        stem, ext = os.path.splitext(name)
        if ext.lower() not in AUDIO or name.startswith("._"):
            continue
        dst = os.path.join(out, stem + ".json")
        if os.path.exists(dst):
            continue
        path = os.path.join(records, name)
        downbeats = dbn(rnn(path))
        bpm, ticks, _confidence, _estimates, _intervals = rhythm(
            es.MonoLoader(filename=path, sampleRate=44100)()
        )
        found = {
            "madmom": {
                "version": madmom.__version__,
                "beats": [[round(float(t), 4), int(b)] for t, b in downbeats],
            },
            "essentia": {
                "version": essentia.__version__,
                "bpm": round(float(bpm), 3),
                "beats": [round(float(t), 4) for t in ticks],
            },
        }
        # Written whole or not at all, so a stopped run leaves nothing a rerun
        # would take for done.
        with open(dst + ".partial", "w") as f:
            json.dump(found, f)
        os.replace(dst + ".partial", dst)
        print(f"{stem}: madmom {len(downbeats)} beats, essentia {bpm:.2f} BPM", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
