# musicai

An audio tool in Rust, as a command-line program and as a macOS app. It does three things:

- **Normalize** mp3, flac and wav files to a consistent loudness (EBU R128 / LUFS), either by
  re-encoding or by writing ReplayGain tags and leaving the audio untouched.
- **Separate** a mix into three stems — vocals, melody and drums, using demucs.
- **Tag** files by identifying them from their sound, via acoustic fingerprinting and
  MusicBrainz.

Everything runs on your machine, but two commands need something beyond the binary. `stems` drives
a locally installed [demucs](https://github.com/adefossez/demucs); `tag` fingerprints locally but
must ask AcoustID and MusicBrainz to turn that fingerprint into metadata, so it needs the network.
`analyze` and `normalize` need neither.

There is no ffmpeg dependency: decoding is
[Symphonia](https://github.com/pdeljanov/Symphonia), and the encoders (LAME for mp3, `flacenc`
for flac, `hound` for wav) are built into the binary.

## Install

```sh
cargo build --release
# binary at ./target/release/musicai
```

The window is a separate crate in the same workspace, so this builds only the command-line tool
and none of a window toolkit. For the app, see [The macOS app](#the-macos-app).

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
# -> stems/track-vocals.flac
#    stems/track-melody.flac
#    stems/track-drums.flac
```

Stems are named after the track they came from, so they stay identifiable once they leave the
directory they were written into.

Options worth knowing: `--only vocals,drums` to write a subset, `--format wav` to override the
output codec, `-o DIR` to change where they land.

### Stems look like the file they came from

Stems are written in the same format as their parent by default: an mp3 yields mp3 stems, a flac
yields flac. Pass `--format` to override that for every input.

They also inherit the parent's tags, so a stem lands in a library as a recognisable track rather
than as an untitled file by an unknown artist. Three details are not a straight copy:

- **The stem name is appended to the title**, giving `Some Song (vocals)`. Three files all called
  the same thing are worse than useless in a library.
- **ReplayGain tags are dropped.** They measure the loudness of the mix, and a stem is quieter
  than the mix it came from, so keeping them would have a player apply a figure taken from
  different audio. Run `musicai normalize --mode replaygain` on the stems if you want correct
  ones.
- **A `STEM` tag** (a Vorbis comment, or an ID3 `TXXX` frame) records which stem the file is.

When parent and stem share a format the whole tag comes across, including fields this tool has no
model for — genre, composer, comments, cover art. When they differ (a flac parent with `--format
mp3`, say) only the fields with an agreed meaning in both dialects survive. Wav stems carry no
tags at all, because wav has no standard place to put them; that is not treated as an error. Pass
`--no-tags` to skip the copy entirely.

### Demucs (default)

Separation runs through [demucs](https://github.com/adefossez/demucs), which you install
yourself:

```sh
uv tool install demucs --with numpy     # or: pipx install demucs && pipx inject demucs numpy
musicai stems track.flac
```

The `numpy` is not optional. Demucs 4.1.0 imports numpy but does not list it among its
dependencies, and torch no longer pulls it in, so a plain `install demucs` produces something
that dies on first run with `ModuleNotFoundError: No module named 'numpy'`. `musicai` recognises
that failure and tells you how to fix it, and its own installer adds numpy on every route.

`--demucs-bin`, `--demucs-model` and `--demucs-device` are there when you need them.

#### Installing it for you (macOS)

On macOS, if demucs is missing, `musicai` offers to install it rather than just complaining:

```
demucs is not installed. Install it now?

    pipx install demucs    # installs demucs into its own isolated environment

This installs software on your machine, and demucs downloads about 300 MB of model weights the
first time it runs.

Proceed? [y/N]
```

Nothing runs until you answer `y` — pressing return declines, and so does anything other than
`y`/`yes`. The prompt always lists the exact commands first. If pipx is missing but Homebrew is
present, `brew install pipx` is added to the list and shown alongside.

The offer only appears when there is a terminal to answer on, so scripts and CI get an error with
instructions instead of hanging on a prompt nobody can see. `--install-demucs` controls it:

| Value | Behaviour |
|---|---|
| `ask` (default) | Offer, on macOS, when attached to a terminal |
| `never` | Never offer; print instructions and stop |
| `yes` | Install without prompting — for scripts that have already decided |

Elsewhere, and when neither installer is available, you get the instructions and nothing is run.
A freshly pipx-installed demucs is not on the `PATH` this process inherited, so `musicai` looks in
pipx's own bin directory rather than telling you to open a new shell.

Demucs produces four stems, so its `bass` and `other` are summed to make our `melody`. It is slow
on a CPU — roughly four minutes per track — and much faster on a GPU via `--demucs-device cuda`.

Isolation is verified rather than asserted. `tests/stem_isolation.rs` transcribes the mix and each
stem with Whisper and checks that the sung words come back from the vocal stem and not from the
others; see [Tests](#tests) for how to run it.

### The built-in separator (`--backend dsp`)

Pure signal processing: no model files, no network, no Python, and about 25x faster than demucs on
a CPU. **Its quality is well short of demucs** — the voice bleeds into all three stems, audibly.
It is here for when demucs is not an option, not as a serious alternative to it.

It is Driedger &amp; Müller's cascade of two harmonic/percussive separations at different
resolutions. The first pass, at the shorter window, splits vertical spectrogram streaks (drum
hits) from horizontal ones (anything pitched) — that gives the **drums**. The second pass runs on
the leftovers at a longer window, where a steadily-pitched instrument is still a clean horizontal
line but a sung note smears vertically because its pitch is never quite still. The smeared half is
the **vocals**, the steady half is the **melody**. On stereo input the vocal mask is additionally
weighted by how centred each frequency bin is.

What it does well: drums come out cleanly, and the three stems always add back up to the original
mix exactly, since every split uses a pair of masks that sum to one over an invertible STFT.

What it does badly: anything sustained and pitched reads as melody whether or not a person sang
it, and the residue of the voice is spread across every stem. On a real track its vocal stem
measures 7 dB quieter than demucs' with half the loudness range — the signature of a residue
rather than an isolated voice.

`musicai stems --help` lists its tuning knobs under "Built-in separator tuning". The defaults were
chosen by measuring how cleanly synthetic mixes were routed — see `tests/separation_quality.rs` —
and are not the obvious values. Two in particular:

- `--drum-fft` cannot be very small. At 1024 points the bins are 43 Hz wide, chord tones in the
  low-mid range fall inside a single bin, and the whole chord reads as a transient and ends up in
  the drum stem.
- `--voice-bandwidth` has to be narrow (tens of hertz). Anything much wider than a vibrato-smeared
  partial swallows the lead along with the steady notes.

Memory scales with track length: a five-minute stereo track peaks around 1.1 GB, or 0.8 GB with
`--overlap 2`, which also roughly halves the runtime.

### Stems can clip

Separated stems peak **above** the mix they came from — the split redistributes energy, so a stem
may exceed full scale even when the original never did. On a loud master that means clipping on
the way into an integer format, and `musicai` says so per file:

```
wrote stems/track-melody.flac — warning: 52 samples clipped; the stem peaks above full scale
```

Raising `--bit-depth` does not help, since the limit is range rather than precision. Normalize the
stems afterwards, or separate a quieter copy of the track.

## Tag

Identify files by what they sound like, and write the resulting metadata into their tags:

```sh
export ACOUSTID_API_KEY=...        # free from https://acoustid.org/new-application
musicai tag ~/Music/unsorted -r
```

```
album/01.flac: Radiohead - Creep (Pablo Honey) (score 0.98) — wrote 14 fields
album/02.flac: no confident match
```

The pipeline is: compute a Chromaprint fingerprint locally, ask **AcoustID** which MusicBrainz
recording it is, ask **MusicBrainz** for the details, write them into the file. Filenames and
any existing tags are ignored — identification is from the audio alone, so a file called
`track03.flac` with no tags identifies just as well as a correctly named one.

### This command uses the network

Unlike everything else here, `tag` cannot work offline. It sends the fingerprint (not the audio)
and the track duration to AcoustID, then recording IDs to MusicBrainz, and with `--cover-art` it
fetches images from the Cover Art Archive. No audio ever leaves your machine, but the fingerprint
and the fact that you are looking a track up do.

**AcoustID needs a free API key**, from <https://acoustid.org/new-application>. Pass it with
`--acoustid-key` or set `ACOUSTID_API_KEY`.

> AcoustID issues **two** keys and only one of them works here. Lookups need the *application*
> API key, listed at <https://acoustid.org/my-applications>. The *user* API key in your account
> preferences is a different thing, used only for submitting fingerprints back to AcoustID.
> Both are short alphanumeric strings and the service rejects the wrong one with nothing more
> than "invalid API key", so it is an easy mix-up; `musicai` spells out the difference if it
> happens. **MusicBrainz needs no key** — it is open for
non-commercial use — but it does require a descriptive User-Agent and no more than one request
per second, both of which this handles. That rate limit is why tagging a large library takes a
while: roughly one second per distinct track.

Fingerprinting alone needs no key and no network:

```sh
musicai tag ~/Music --print-fingerprint    # prints "<duration> <fingerprint>" per file
```

### Existing tags

`--on-existing` decides what happens to fields a file already has. Empty fields are always
filled, whatever the setting.

- `keep` (default) — never touch a field that already has a value. Safe for a library you have
  curated by hand.
- `overwrite` — MusicBrainz wins. Good for a library you know is a mess.
- `report` — leave existing values alone, but print every case where MusicBrainz disagrees, so
  you can look before deciding.

### Confidence

AcoustID returns candidates with a confidence score from 0 to 1. `--min-score` (default `0.8`)
sets the bar, and `--on-ambiguous` decides what happens to files that fall short:

```sh
musicai tag ~/Music --min-score 0.9 --on-ambiguous skip   # cautious (skip is the default)
musicai tag ~/Music --min-score 0.5 --on-ambiguous best   # tag everything, best guess wins
```

`--dry-run` looks everything up and reports what it would write without touching a file.

### What gets written

Tag names follow the MusicBrainz Picard convention, so other software recognises them: Vorbis
comments for flac (`TITLE`, `ALBUMARTIST`, `MUSICBRAINZ_TRACKID`, …) and ID3v2.4 for mp3 (`TIT2`,
`TPE2`, `TXXX:MusicBrainz Album Id`, …). Wav has no standard metadata tag and is rejected.

Alongside the obvious fields — title, artist, album, album artist, date, track and disc numbers —
it writes the MusicBrainz recording, release, release-group and artist IDs, plus the AcoustID.
Those identifiers are what let you re-look-up or correct the metadata later without
re-fingerprinting.

`--cover-art` additionally fetches the front cover from the Cover Art Archive and embeds it
(a FLAC `PICTURE` block or an ID3 `APIC` frame). It is off by default because it means more
requests and materially bigger files. Re-tagging replaces the existing front cover rather than
adding a second one.

Only the tag blocks are rewritten; the audio is left byte-for-byte alone.

## The macOS app

Everything above, in a window. Pick a task along the top, drop files on the left, set the options
in the middle, press the button. Results appear in the log at the bottom as they arrive, and a
long job can be stopped without leaving a half-written file behind.

![The app, having normalized two files](docs/screenshot.png)

### Building it

```sh
scripts/package-macos.sh
# dist/musicai.app and dist/musicai-0.1.0.dmg
```

That builds for Apple silicon and Intel and `lipo`s them together, so the result runs natively on
either. It needs both targets installed:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

The bundle carries **both** binaries — the app you launch and the `musicai` command-line tool, at
`musicai.app/Contents/MacOS/musicai`. One download gives you both, and they can never be different
versions of each other. Symlink it onto your `PATH` if you want it there:

```sh
ln -s /Applications/musicai.app/Contents/MacOS/musicai /usr/local/bin/musicai
```

To run the window without packaging anything:

```sh
cargo run -p musicai-gui --release
```

Files named on the command line start out selected, which is also how Finder's *Open With* hands
over a selection.

### Signing

Without a Developer ID the app is ad-hoc signed: it runs on the machine that built it, and
Gatekeeper stops it anywhere else until you right-click and choose *Open*. With one, set the
environment and the script does the rest:

```sh
MUSICAI_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
MUSICAI_NOTARY_PROFILE=my-notary-profile \
    scripts/package-macos.sh
```

`.github/workflows/macos-app.yml` builds the same thing on a macOS runner, on a tag or on demand.

### What the window is, and is not

It is a front end, not a second implementation. Every option is the command-line tool's own
argument struct, filled in with the defaults clap would apply, and pressing the button calls the
same function the CLI calls. A default cannot drift between the two, and an option added to the
CLI without a control in the window is a compile error rather than a silent difference. The
command line that matches what you have set up is printed in the log when a job starts, so you can
run it once in the window and then copy the line into a script.

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

Unit tests cover the STFT round trip, the median filters, the limiter's ceiling guarantee, the
loudness maths and the parsing of AcoustID and MusicBrainz responses. `tests/end_to_end.rs`
drives the real binary over real encoded files, `tests/separation_quality.rs` pins how well the
default separator routes known sources, `tests/tagging.rs` round-trips tags through real
flac and mp3 files, and `tests/packaging.rs` assembles the macOS bundle and checks its layout,
its plist and the icon container.

The window has its own tests, which are not in the default workspace member and so need asking
for:

```sh
cargo test -p musicai-gui --release
```

They run real jobs through the worker thread and assert on what comes back — progress, per-file
failures, cancellation, and that the window is told to redraw. Nothing there needs a display.

That suite is entirely offline. Tests that talk to the real services are marked `#[ignore]` and
run separately:

```sh
cargo test --release --test live_services -- --ignored --test-threads=1
```

`--test-threads=1` is required — the rate limiter is shared per host, but running these in
parallel would still queue them all up behind each other for no benefit. The AcoustID test needs
`ACOUSTID_API_KEY` and skips itself without one.

`tests/stem_isolation.rs` is the one that decides whether separation actually works. It
transcribes the mix and each stem with Whisper and checks the sung words come back from the vocal
stem and not from the others:

```sh
pipx install demucs openai-whisper
export MUSICAI_TEST_TRACK=/path/to/a-song-with-vocals.flac
cargo test --release --test stem_isolation -- --ignored --nocapture
```

It holds no expected text of its own. The reference is whatever Whisper makes of the original mix
at run time, so it works on any track with a voice in it, and it reports word counts and
percentages rather than transcripts.

## License

MIT
