# musicai

A local command-line audio tool in Rust. It does two things:

- **Normalize** mp3, flac and wav files to a consistent loudness (EBU R128 / LUFS), either by
  re-encoding or by writing ReplayGain tags and leaving the audio untouched.
- **Separate** a mix into three stems — vocals, melody and drums.

Everything runs on your machine. Nothing is uploaded, and nothing is downloaded at run time.
There is no ffmpeg dependency: decoding is [Symphonia](https://github.com/pdeljanov/Symphonia),
and the encoders (LAME for mp3, `flacenc` for flac, `hound` for wav) are built into the binary.

## Install

```sh
cargo build --release
# binary at ./target/release/musicai
```

## Analyze

Report loudness and peaks without touching anything:

```sh
musicai analyze ~/Music/album
```

```
file                                       length   loudness    range   true peak
album/01 - opener.flac                       3:41  -9.8 LUFS   6.2 LU    0.4 dBTP
album/02 - the quiet one.flac                4:12 -18.3 LUFS   9.1 LU   -2.1 dBTP
```

`--json` emits one object per line for scripting.

## Normalize

Two modes, chosen with `--mode`.

### `--mode reencode` (default)

Applies the gain to the samples and writes a new file. Defaults to **-14 LUFS** (the level most
streaming services normalize to) with a **-1.0 dBTP** ceiling.

```sh
# Write album/01 - opener-normalized.flac next to each input
musicai normalize ~/Music/album

# Collect the results elsewhere, converting to mp3 on the way
musicai normalize ~/Music/album -o ~/Music/normalized --format mp3 --bitrate 256
```

When the loudness target would push peaks past the ceiling, `--on-peak` decides what gives:

- `attenuate` (default) — apply less gain and stay under the ceiling. The track ends up quieter
  than the target, and the shortfall is reported rather than hidden.
- `limit` — apply the full gain and pull the transients back with a look-ahead limiter. Hits the
  target, at the cost of reshaping peaks.

```sh
musicai normalize ~/Music/album --target -16 --on-peak limit
```

Outputs never overwrite an existing file unless you pass `--force`, and `--dry-run` measures and
reports without writing anything.

### `--mode replaygain`

Measures loudness and writes ReplayGain 2.0 tags into the existing files. The audio stays
bit-for-bit identical, so this is the right choice for a lossy library — there is no re-encode
generation loss. Defaults to the -18 LUFS reference the ReplayGain spec defines.

```sh
musicai normalize ~/Music/album --mode replaygain --album
```

`--album` additionally computes album gain, grouping files by the directory they sit in. Album
gain is pooled across the whole album's gating blocks rather than averaged per track, so a short
quiet track does not drag the album figure down out of proportion.

Tags go into ID3v2 `TXXX` frames for mp3 and Vorbis comments for flac. Wav has no standard
ReplayGain tag, so this mode rejects wav files and tells you to use `--mode reencode`.

## Stems

```sh
musicai stems ~/Music/track.flac
# -> stems/track/vocals.wav
#    stems/track/melody.wav
#    stems/track/drums.wav
```

Options worth knowing: `--only vocals,drums` to write a subset, `--format flac` to change the
output codec, `-o DIR` to change where they land.

### The built-in separator (`--backend dsp`, default)

Pure signal processing: no model files, no network, no Python. A five-minute stereo track
separates in about 23 seconds on a multi-core machine.

It is Driedger & Müller's cascade of two harmonic/percussive separations at different
resolutions. The first pass, at the shorter window, splits vertical spectrogram streaks (drum
hits) from horizontal ones (anything pitched) — that gives the **drums**. The second pass runs
on the leftovers at a longer window, where a steadily-pitched instrument is still a clean
horizontal line but a sung note smears vertically because its pitch is never quite still. The
smeared half is the **vocals**, the steady half is the **melody**. On stereo input the vocal
mask is additionally weighted by how centred each frequency bin is, since lead vocals are almost
always panned to the middle.

Two consequences worth setting expectations around:

- **The three stems always add back up to the original mix**, exactly. Every split uses a pair of
  masks that sum to one, and the STFT reconstructs to within floating-point error. This is
  checked by tests.
- **It is not a neural separator.** Drums come out cleanly; vocals and melody are usable for
  practice, remixing and analysis, but there is audible bleed both ways, and anything with a
  steady pitch and no vibrato — a held organ note, a synth pad — will read as melody even if a
  person sang it. Use the demucs backend when you need it clean.

`musicai stems --help` lists the tuning knobs under "Built-in separator tuning". The defaults
were chosen by measuring how cleanly a set of synthetic mixes was routed, and they are not the
obvious values — see `tests/separation_quality.rs`. Two in particular:

- `--drum-fft` cannot be very small. At 1024 points the bins are 43 Hz wide, chord tones in the
  low-mid range fall inside a single bin, and the whole chord reads as a transient and ends up
  in the drum stem.
- `--voice-bandwidth` has to be narrow (tens of hertz). Anything much wider than a
  vibrato-smeared partial swallows the lead along with the steady notes, and the vocal stem
  stops tracking the voice.

Memory scales with track length: the whole track and several spectrograms of it are held at
once. A five-minute stereo track peaks around 1.1 GB. Passing `--overlap 2` brings that to about
0.8 GB and roughly halves the runtime, at a small cost in masking smoothness — the audio buffers
that dominate the total do not shrink with overlap, so it is not a 2x saving.

### The demucs backend (`--backend demucs`)

If you have [demucs](https://github.com/adefossez/demucs) installed, `--backend demucs` shells
out to it for much better quality:

```sh
pipx install demucs
musicai stems track.flac --backend demucs
```

Nothing is installed or downloaded on your behalf; if the binary is not there, you are told to
use `--backend dsp`. Demucs produces four stems, so its `bass` and `other` are summed to make our
`melody`. `--demucs-bin`, `--demucs-model` and `--demucs-device` are there when you need them.

## Notes

- Supported formats in and out: mp3, flac, wav. Files named explicitly on the command line are
  processed whatever their extension; when scanning a directory, only those three are picked up.
  Use `-r` to descend into subdirectories.
- Files are processed in parallel; `-j N` caps the worker threads. Stem separation runs one file
  at a time because each one already uses every core and holds a lot of memory.
- A file that fails to decode is reported and skipped; the rest of the batch still runs, and the
  exit status is non-zero.
- 16-bit output is dithered (TPDF) by default. `--no-dither` turns that off; `--bit-depth 24`
  makes it moot.

## Tests

```sh
cargo test --release
```

Unit tests cover the STFT round trip, the median filters, the limiter's ceiling guarantee and
the loudness maths. `tests/end_to_end.rs` drives the real binary over real encoded files, and
`tests/separation_quality.rs` pins how well the default separator routes known sources.

## License

MIT
