# booth-core

The engine: analysis, loudness, stems, tagging and the drive writer, with the `booth-cli`
command-line tool wrapped around it. [Booth](../../booth/README.md) is built on this crate, and the macOS app is
Booth — see [the top-level README](../../README.md). This is the interface that came first, which
is all the `proto/` in the path — and the directory's old name — mean.

It does three things on its own:

- **Normalize** mp3, flac, wav, m4a and aiff files to a consistent loudness (EBU R128 / LUFS), either by
  re-encoding or by writing ReplayGain tags and leaving the audio untouched.
- **Separate** a mix into three stems — vocals, melody and drums, using demucs.
- **Tag** files by identifying them from their sound, via acoustic fingerprinting and
  MusicBrainz.

Two more commands are the start of something larger: writing a USB drive a Pioneer /
AlphaTheta DJ player can browse and play. `export` builds a whole drive; `anlz` writes
just the per-track analysis files. See [Export](#export) for what works and what does
not yet.

By default it does the first three, over everything you point it at:

```sh
booth-cli ~/Music/album
```

Everything runs on your machine, but two commands need something beyond the binary. `stems` drives
a locally installed [demucs](https://github.com/adefossez/demucs); `tag` fingerprints locally but
must ask AcoustID and MusicBrainz to turn that fingerprint into metadata, so it needs the network.
`analyze` and `normalize` need neither.

There is no ffmpeg dependency: decoding is
[Symphonia](https://github.com/pdeljanov/Symphonia), and the encoders (LAME for mp3, `flacenc`
for flac, `hound` for wav) are built into the binary.

## Install

```sh
cargo build --release -p booth-core
# binary at ./target/release/booth-cli
```

`-p booth-core`, because a plain `cargo build` in this workspace builds Booth. Building this crate on
its own pulls in none of a window toolkit, which is why it is a crate of its own.

## The default: all of it, over everything

With no subcommand, `booth-cli` runs the whole pipeline over the files and folders you name. It is
the same as `booth-cli run`, which is where the options live.

```sh
booth-cli ~/Music/album -r
```

```
12 files through normalize -> tag -> stems
== 1/3 normalize ==
album/01 - opener.flac: -9.8 LUFS, track gain -8.20 dB, tagged
...
== 2/3 tag ==
album/01 - opener.flac -> Opener — Some Band (0.98)
...
== 3/3 stems ==
wrote stems/01 - opener-vocals.flac
```

The whole file list is gathered before any work starts, so the run can say how much there is to do
and a folder is walked once rather than once per step. Each step then runs over every file before
the next begins, and announces itself as it starts.

**The order is deliberate.** Tagging happens before separation, so the stems inherit the tags that
were just written rather than whatever was there before. Normalizing happens first, so everything
downstream sees the finished audio. `--steps normalize,stems` runs a subset; they always run in
that order regardless of how they are listed.

Three things about the defaults are worth knowing:

- **Loudness is tagged, not re-encoded.** `--mode replaygain` is the default here, unlike the
  `normalize` subcommand, because a pipeline that re-encodes leaves a second copy of your whole
  library behind. The audio is untouched and there is no generation loss. Pass `--mode reencode`
  to write new files instead — the later steps then follow the new files, not the originals, so
  the stems come from the audio you actually normalized.
- **Wav files skip the loudness step.** Wav has nowhere to put a ReplayGain tag. Rather than
  failing once per file, the run says so once and carries them on to tagging and separation.
- **Tagging is skipped without an AcoustID key**, with a note saying so, because every lookup
  would otherwise fail. Set `ACOUSTID_API_KEY` or pass `--acoustid-key`.

`--dry-run` reports what every step would do and writes nothing. Separation is the slow part; if
you only want the other two, `--steps normalize,tag`.

## Analyze

Report loudness and peaks without touching anything:

```sh
booth-cli analyze ~/Music/album
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
booth-cli normalize ~/Music/album

# Collect the results elsewhere, converting to mp3 on the way
booth-cli normalize ~/Music/album -o ~/Music/normalized --format mp3 --bitrate 256
```

When the loudness target would push peaks past the ceiling, `--on-peak` decides what gives:

- `attenuate` (default) — apply less gain and stay under the ceiling. The track ends up quieter
  than the target, and the shortfall is reported rather than hidden.
- `limit` — apply the full gain and pull the transients back with a look-ahead limiter. Hits the
  target, at the cost of reshaping peaks.

```sh
booth-cli normalize ~/Music/album --target -16 --on-peak limit
```

Outputs never overwrite an existing file unless you pass `--force`, and `--dry-run` measures and
reports without writing anything.

### `--mode replaygain`

Measures loudness and writes ReplayGain 2.0 tags into the existing files. The audio stays
bit-for-bit identical, so this is the right choice for a lossy library — there is no re-encode
generation loss. Defaults to the -18 LUFS reference the ReplayGain spec defines.

```sh
booth-cli normalize ~/Music/album --mode replaygain --album
```

`--album` additionally computes album gain, grouping files by the directory they sit in. Album
gain is pooled across the whole album's gating blocks rather than averaged per track, so a short
quiet track does not drag the album figure down out of proportion.

Tags go into ID3v2 `TXXX` frames for mp3 and Vorbis comments for flac. Wav has no standard
ReplayGain tag, so this mode rejects wav files and tells you to use `--mode reencode`.

## Stems

```sh
booth-cli stems ~/Music/track.flac
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
  different audio. Run `booth-cli normalize --mode replaygain` on the stems if you want correct
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
booth-cli stems track.flac
```

The `numpy` is not optional. Demucs 4.1.0 imports numpy but does not list it among its
dependencies, and torch no longer pulls it in, so a plain `install demucs` produces something
that dies on first run with `ModuleNotFoundError: No module named 'numpy'`. `booth-cli` recognises
that failure and tells you how to fix it, and its own installer adds numpy on every route.

`--demucs-bin`, `--demucs-model` and `--demucs-device` are there when you need them.

#### Installing it for you (macOS)

On macOS, if demucs is missing, `booth-cli` offers to install it rather than just complaining:

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
A freshly pipx-installed demucs is not on the `PATH` this process inherited, so `booth-cli` looks in
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

`booth-cli stems --help` lists its tuning knobs under "Built-in separator tuning". The defaults were
chosen by measuring how cleanly synthetic mixes were routed — see `tests/separation_quality.rs` —
and are not the obvious values. Two in particular:

- `--drum-fft` cannot be very small. At 1024 points the bins are 43 Hz wide, chord tones in the
  low-mid range fall inside a single bin, and the whole chord reads as a transient and ends up in
  the drum stem.
- `--voice-bandwidth` has to be narrow (tens of hertz). Anything much wider than a vibrato-smeared
  partial swallows the lead along with the steady notes.

Memory scales with track length: a five-minute stereo track peaks around 1.1 GB, or 0.8 GB with
`--overlap 2`, which also roughly halves the runtime.

### Stems stay under full scale

Separated stems peak **above** the mix they came from — the split redistributes energy, so a stem
may exceed full scale even when the original never did. Two things happen so that this does not turn
into clipping:

- Demucs is asked for `--float32` output. Left to itself it writes 16-bit wav and clips the stem at
  the source, before `booth-cli` ever sees it — audible on loud masters. Float carries the peaks
  through intact.
- Before writing, the stems are pulled down together by a single gain so the loudest sample across
  all of them sits just under full scale. One shared gain rather than one per stem, so they stay in
  balance and still add back up to the track — which is what lets an acapella and an instrumental be
  played together in time. The attenuation is reported once:

```
wrote stems/track-vocals.flac (stems attenuated -2.4 dB to stay under full scale)
```

Stems default to **24-bit** wav/flac for the same reason — they are already-processed audio, and
there is no reason to quantise them to 16.

### Getting cleaner separation from demucs

The default model, `htdemucs`, is a good all-rounder. Two levers trade time for fewer artefacts:

- `--demucs-model htdemucs_ft` — the fine-tuned model. Noticeably cleaner, about four times slower.
- `--demucs-shifts 1` (or `2`) — separates the track again at small offsets and averages, smoothing
  artefacts, at a roughly linear cost in time.
- `--demucs-overlap 0.5` — more overlap between analysis windows, fewer seams, more compute.

On a GPU (`--demucs-device cuda` or `mps`) the time cost of these is much easier to absorb.

## Tag

Identify files by what they sound like, and write the resulting metadata into their tags:

```sh
export ACOUSTID_API_KEY=...        # free from https://acoustid.org/new-application
booth-cli tag ~/Music/unsorted -r
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
> than "invalid API key", so it is an easy mix-up; `booth-cli` spells out the difference if it
> happens. **MusicBrainz needs no key** — it is open for
non-commercial use — but it does require a descriptive User-Agent and no more than one request
per second, both of which this handles. That rate limit is why tagging a large library takes a
while: roughly one second per distinct track.

Fingerprinting alone needs no key and no network:

```sh
booth-cli tag ~/Music --print-fingerprint    # prints "<duration> <fingerprint>" per file
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
booth-cli tag ~/Music --min-score 0.9 --on-ambiguous skip   # cautious (skip is the default)
booth-cli tag ~/Music --min-score 0.5 --on-ambiguous best   # tag everything, best guess wins
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

## Export

Build a drive: the audio, the analysis, and the database that indexes them.

```sh
booth-cli export ~/Music/set -r --playlist "Sat 14/9" -o /Volumes/USB
```

```
set/01 - opener.flac -> /Contents/Peverelist/01 - opener.flac (128.02 BPM 8A, 1536 beats, 6 phrases, 5 cues)
set/02 - marius.flac -> /Contents/Batu/02 - marius.flac (130.00 BPM 4A, 1478 beats, 4 phrases, 4 cues)
wrote /Volumes/USB/PIONEER/rekordbox/export.pdb: 20 tables, 41 rows, verified
```

The drive comes out shaped the way a player expects:

```
/Contents/<artist>/<file>              the audio, copied, not re-encoded
/PIONEER/USBANLZ/P001/00000001/…       ANLZ0000.DAT, .EXT and .2EX per track
/PIONEER/rekordbox/export.pdb          the database that points at both
```

`--image drive.img` writes a disk image instead of a folder: a raw file with a
partition table and a FAT32 filesystem, which is what a player actually reads. It goes
onto a stick with `dd if=drive.img of=/dev/disk4 bs=4m`, or into the USB slot of an
emulator — see [Trying it on a player](#trying-it-on-a-player).

Each track is listened to on the way past: the beats are found, the track is divided into
phrases, and cue points are set at the phrase boundaries and where a voice comes in — see
[What it hears](#what-it-hears). `--bpm` overrides the tempo when the detector gets it
wrong; the beats are still tracked against the audio, so the grid stays where the music
is.

Title, artist, album, year and track number come from the file's own tags where it has
them. Everything named goes into one playlist, `--playlist` names it, and `--dry-run`
reports what would be written without touching the drive.

Files a player cannot open are refused rather than copied — a file that fails at the gig
is worse on the drive than off it — which today means checking the container and the
sample rate.

Variable-bitrate MP3s get a seek index (`PVBR`) so their hot cues land in the right place
on the player — see [What it hears](#what-it-hears). Every other format seeks without one.

Once the database is written it is read back off the drive and walked the way a player
walks it, and the command fails rather than reporting success if that does not work.

**What is missing before this plays in a club:** none of it has been tried on real
hardware. It has been checked against an independent parser, which is not the same thing.

## Trying it on a player

The nearest thing to a CDJ that is not a CDJ is
[cdj3k-emu](https://github.com/nsaintot/cdj3k-emu), which boots real CDJ-3000 firmware
under QEMU and gives it a virtual USB slot. A drive written here can be handed straight
to it:

```sh
booth-cli export ~/Music/set -r --image ~/rekordbox.img
```

then in the emulator, **USB → Attach virtual image** and pick `rekordbox.img`. The guest
mounts it the same way the firmware mounts a real stick: partition 1, FAT32, at
`/media/usb/sdb1`. What to look at, in the order that things break:

1. **Does the drive appear at all?** That is the database being readable —
   `export.pdb` parsed, the table list understood.
2. **Do the playlists and tracks list?** The playlist tree, the playlist entries, and the
   track rows, with their titles and artists off the interned tables.
3. **Does a track load?** The `file_path` in the row resolving to real audio.
4. **Is there a waveform, in three colours?** The `.EXT` and `.2EX` files being found
   through `analyze_path`, and the three-band data being what the CDJ-3000 expects.
5. **Are the cues on the beat, and named?** The beat grid, `PCO2`, and the analysis.
6. **Are the phrases drawn under the waveform?** `PSSI`, including the mask.

Two things are worth knowing before setting time aside for this. The emulator is **Apple
Silicon macOS only** — it uses HVF, vmnet and CoreAudio, so there is no Linux or Windows
build and it cannot run in CI. And it **ships no Pioneer firmware**: it needs a CDJ-3000
firmware update file and its decryption key, which you have to supply yourself and which
this project cannot help with either.

Failing that, the checks that can be run anywhere are the ones in [Tests](#tests):
everything written is parsed back by
[rekordcrate](https://github.com/Holzhaus/rekordcrate), and a disk image is additionally
read with [mtools](https://www.gnu.org/software/mtools/), so the filesystem is one that
something other than us agrees is a filesystem.

## What it hears

Analysis is signal processing rather than a model: it runs offline in a couple of seconds
a track, it explains itself, and everything it decides is visible and correctable.

**The beats.** Spectral flux gives an onset envelope; its autocorrelation, weighted
towards the tempo a listener would pick, gives the period; and a dynamic program then
chooses the sequence of beat times that best balances landing on the onsets against
keeping time. That last step is Ellis's, from 2007, and it is hard to beat without a
neural network. The bar lines go where the kicks are.

A tempo read off the autocorrelation can only be a whole number of frames, which is a
step of more than a beat per minute — too coarse to hold a mix together. So the tempo is
read back off the tracked beats instead, and if a straight line fits them, the grid
becomes that line. A track made to a click comes out at one exact tempo; one that drifts
keeps its drift, and each beat carries the tempo measured around it.

**The phrases.** Each bar is described by where its energy sits, every bar is compared
with every other, and the moments where the music stops resembling what came before are
the boundaries — Foote's method. Boundaries snap to four bars, because arrangements are
built in fours. Sections are then named from how much is happening in them: intro, build,
drop, break, outro, which are the phrase types the format calls a "high mood" track and
the words a DJ uses about a record. Those names are heuristic and the positions are not;
a breakdown that leads into a drop can honestly be called either.

**The key.** A chromagram folds the spectrum down to how much of each of the twelve pitch
classes is present, averaged over the track; a profile-matching method then correlates
that against a template of what each of the twenty-four keys sounds like, and the closest
match wins. Two details are for real music rather than tidiness: the chromagram is read
*at* each note's frequency and interpolated, not by dropping FFT bins into the nearest
class — in the bass, where dance music carries its key, a semitone is a few hertz wide and
the nearest-class approach lands wrong as often as right — and the key templates are
[Sha'ath's](https://www.ibrahimshaath.co.uk/keyfinder/), the ones KeyFinder uses, which
were tuned on popular and electronic music and tell major from minor on a bass-heavy track
where the classical templates flip them. The output is the Camelot code a DJ mixes by (`8A`) and the
classical name under it (`Am`), with a confidence, and a runner-up when the two best were
a hair apart — usually the relative major/minor, which shares every note. A track with no
tonal centre — a drum tool — is left without a key rather than assigned a wrong one.

**The cues.** A memory cue at the first downbeat, where a player parks when the track
loads, and up to eight hot cues: one at each phrase boundary, and one where a voice comes
in. When there are more than eight candidates the drops and the first vocal survive and
the builds are dropped. Everything lands on a beat, and on a downbeat where there is one
close by.

Finding the voice without separating the stems means measuring energy that is both
centred in the stereo image and in the range a voice occupies. That finds a sung line
entering over a backing; it will also fire on a centred lead synth. Separating the stems
properly would answer it better and costs minutes a track rather than milliseconds —
which is the trade [the spec](../../docs/rekordbox-replacement-spec.md) proposes making later,
in the background, for the tracks that are going to a gig.

**Seeking a VBR MP3.** A variable-bitrate MP3 has no fixed relationship between a moment in
the music and a byte in the file — each frame holds the same audio but takes a different
number of bytes — so a player cannot jump to a hot cue by arithmetic. rekordbox writes a
table of byte offsets, and so does this: it walks the MP3's frame headers (without decoding
anything) and builds the 401-entry seek index the CDJ reads. A constant-bitrate file, which
seeks fine by arithmetic, gets the same empty stub rekordbox writes. This only concerns MP3;
FLAC, WAV and AIFF carry their own seek information.

**Measuring it.** The tests above use synthetic audio, which proves the code does what it
was written to do but not that it agrees with a human. `cargo run --release --example eval
-- rekordbox.xml` reads a rekordbox collection export, runs the analysis over the tracks
it references, and reports how often the detected key and tempo match the ones already in
the library — exact, and within a Camelot neighbour or a half/double. `--dry-run` first
shows how much of the library is reachable before committing to the decode. Against a real
8,300-track v7.2.17 export, every one of the 8,209 stored key labels parsed; the audio
side runs on the machine the library lives on.

## Anlz

Write the per-track analysis files a Pioneer / AlphaTheta player reads: the beat grid,
the cues, and the waveforms it draws.

```sh
booth-cli anlz track.flac
# -> track.DAT   track.EXT   track.2EX
```

```
track.flac: 128.02 BPM 8A, 1536 beats, 6 phrases, 5 cues
  track.DAT: 6 sections
  track.EXT: 10 sections
  track.2EX: 3 sections
```

Between them the three files carry a beat grid with per-beat tempo, memory cues and hot
cues with colours and comments, saved loops, phrase analysis, and seven waveforms — the
monochrome preview a 2009 player draws, the colour ones the nexus 2 line introduced, and
the three-band low/mid/high pair the CDJ-3000 shows. Each file is read back off disk
after it is written, by a parser that shares no code with the writer, and the command
fails rather than reporting success if that read-back does not work.

This writes the analysis files and nothing else, which is useful for looking at one
track. For a drive a player can browse, use [`export`](#export).

The file format is not published by its vendor. It has been reverse-engineered in public
and in detail by [Deep Symmetry's DJ Link Ecosystem
Analysis](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html),
which is what this is written against, and the tests check the output by parsing it back
with [rekordcrate](https://github.com/Holzhaus/rekordcrate) — a separate implementation
by different people, so that a misunderstanding of the format cannot be symmetrical and
invisible. The database format is documented in the same place and checked the same way.

[`docs/rekordbox-replacement-spec.md`](../../docs/rekordbox-replacement-spec.md) is the wider
plan this belongs to, and [`docs/onelibrary.md`](../../docs/onelibrary.md) is what is known
about OneLibrary — the database the CDJ-3000X and the other newer players read instead
of `export.pdb`. `export` writes one alongside the legacy database, under the key every
drive uses (overridable with `--onelibrary-key` or `ONELIBRARY_KEY`). A CDJ-3000X has
browsed one; whether it used the analysis files is still unproven.

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
cargo test --release -p booth-core
```

Unit tests cover the STFT round trip, the median filters, the limiter's ceiling guarantee, the
loudness maths and the parsing of AcoustID and MusicBrainz responses. `tests/end_to_end.rs` drives
the real binary over real encoded files, `tests/separation_quality.rs` pins how well the default
separator routes known sources, and `tests/tagging.rs` round-trips tags through real flac and mp3
files. The macOS bundle's layout is checked by `booth/tests/packaging.rs`, next to the app it
describes. The pipeline has its own end-to-end tests: that the steps run in order and announce
themselves, that re-encoding hands the new files to the next step rather than the originals, that
a wav survives a ReplayGain run, and that a missing key skips tagging instead of failing every
file.

The batch window has its own tests, in its own crate:

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
export BOOTH_TEST_TRACK=/path/to/a-song-with-vocals.flac
cargo test --release --test stem_isolation -- --ignored --nocapture
```

It holds no expected text of its own. The reference is whatever Whisper makes of the original mix
at run time, so it works on any track with a voice in it, and it reports word counts and
percentages rather than transcripts.

## License

The [Booth Public Source License](../../LICENSE): free to download, run and modify for personal,
educational and internal use, and free to redistribute through non-commercial and open-source
channels. Putting it inside a proprietary commercial product needs a separate licence from the
copyright holder. It is not an OSI-approved licence, so GitHub shows it as "Other".
