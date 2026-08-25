# Booth

A DJ library that prepares tracks and writes the drives a Pioneer/AlphaTheta
player reads. It is the interface described in
[`docs/rekordbox-replacement-spec.md`](../docs/rekordbox-replacement-spec.md),
built on the analysis and export code in the `musicai` crate next to it.

```
cargo run -p booth
```

Anything named on the command line is imported at startup, and files can be
dropped onto the window.

On Linux the audio output needs ALSA's headers at build time
(`apt install libasound2-dev`, or `alsa-lib-devel`). macOS and Windows need
nothing extra. Without a working output device the window still opens and does
everything else; the transport says `no audio out` instead.

## One window

There are no modes. The query bar is across the top, the collection down the
left, the browser in the middle with the prep editor beneath it rather than in
place of it, the inspector on the right, and the drive dock along the bottom
where the delta is always visible.

## The query bar is the browser

Every filter is text, which means every filter can be saved, pasted, and read.
A saved query *is* a smart playlist — there is no second concept and no rule
editor.

| Query | Finds |
| --- | --- |
| `bpm:124-128` | a tempo range; `bpm:128` finds a 128.02 grid |
| `key:8A` | that key exactly |
| `key:~8A` | anything on the wheel that would mix with it |
| `tag:peak` | a tag |
| `added:<14d` | added in the last fortnight (`d`, `w`, `m`, `y`) |
| `played:never` | bought and never touched |
| `-played:30d` | not played in the last month |
| `missing:grid` | unanalysed, or no beat could be found |
| `missing:stems` | no stem kit rendered yet |
| `missing:key` `missing:cues` `missing:tags` | likewise |
| `in:drive:SANDISK` | what is actually on the stick (a prefix is enough) |
| `in:playlist:peak` | in a playlist; `in:peak` means the same |
| `format:mp3 bitrate:<256` | files that will not survive the booth |
| `energy:>3` | the loud end of a crate |
| `dupes:title+artist` | duplicate groups to merge |
| `peverelist` | a bare word searches artist, title and album |

Terms are combined with and. A leading `-` excludes. A term that does not parse
matches **nothing** and is struck through in red, rather than being ignored —
an ignored typo silently widens a search and looks like it worked.

## Editing

**Names.** The inspector's title, artist, album and year are editable. Saving
changes the collection; the file on disk is untouched unless you ask, either
with **Write these into the file** or by turning on the setting that makes every
save do it. FLAC and MP3 only — a WAV has nowhere to put them, and says so
rather than appearing to work.

**Tags.** Free-form, and the collection's own — they never reach the drive, so
tagging a track does not queue a 40 MB rewrite. A tag already in use is offered
as one click, and a new one that differs only in case joins the existing tag
rather than starting a second one that queries miss.

## Playing

Double-click a row, or press **space**, to hear the selected track. Clicking the
waveform moves the playhead and takes playback with it; pressing a cue jumps
both. That is the point of having a deck at all — a cue placed by eye is a
guess, and the ear is what says whether it is on the beat.

One deck, and it audition only: no pitch, no sync, no mixing. The track is
decoded whole in the background so that seeking anywhere is instant, which is
what checking cues is made of. A file whose rate differs from the output
device's is resampled on the way out.

**Cues.** Click the waveform to put the playhead somewhere, then press an empty
cue button to place a cue there. A full button jumps the playhead to it;
shift-click clears it; dragging its line on the waveform moves it. Everything is
snapped on the way in — hot cues to the beat, the memory cue to the bar, because
starting a track mid-bar is a different mistake from starting it four
milliseconds early. A track with no grid can still be marked up: the cue is what
you are sure of.

Cue times, names and the track's names all go through the drive delta, so an
edit shows as *changed* on the next sync — except a cue's name, which the player
never sees.

## Identifying tracks

Analysis also fingerprints each track and asks AcoustID and MusicBrainz what it
is, filling in names the file does not carry. It needs a free key from
[acoustid.org](https://acoustid.org/new-application), in Settings or as
`ACOUSTID_API_KEY`; without one the rest still works and the window says so.

What happens to a match depends on how sure it is and on what the track already
claims:

- **No names of its own** — applied at or above the threshold, otherwise asked.
- **Named off its file name** — a file name is a guess, so a confident match
  replaces it.
- **Named from the file's own tags** — that is somebody's answer already, so a
  disagreement is always a question, at any confidence.
- **Agrees with what is there** — nothing to ask; the album and year get filled
  in if they were missing.
- **Below 50%** — never offered at all.

The threshold is a slider in Settings, defaulting to 90%: being wrong here
renames somebody's records without them noticing. Questions collect into one
sheet showing both sides and the score. Answering one changes the collection
only — never a file, unless you have turned on the tag write-back.

Lookups are paced to what the two services ask for (three a second, and one a
second respectively). That pacing is theirs and is not adjustable: getting
somebody's address blocked would be real harm.

## Keeping a local copy

A file played from a download folder, a network share or someone else's stick is
a file that can be gone on the night. So when music is reached for — imported,
analysed, separated — anything outside the library folder is **copied in by
default**.

A copy is a copy: the original is never moved or deleted. A file already in the
library is left alone, re-importing a folder does not double it, and a different
file that happens to share a name gets a suffix rather than overwriting
anything.

Settings (bottom left) has the three choices — copy, ask each time, or leave
everything where it is — along with the library folder itself, the stems folder,
and whether editing a name also rewrites the file. Tracks added before you
changed any of it can be brought in from there, or one at a time from the
inspector.

## What the columns mean

Artist, Title, BPM, Key, Energy, Stems, Location. **Click a heading to sort by
it**; click again to reverse. Names start forwards, measurements start at the
loud end, and the choice is remembered between runs.

A track with nothing in the column always sorts last, whichever way round — an
ungridded track is not slower than every other track, and reversing a sort must
not bury the ones still to be worked on. Keys sort around the wheel rather than
alphabetically, so 9A comes before 11B and the column is worth reading.

Stem companions are indented under the track they came from, stay under it in
every sort, and exist only when all three stem files are on disk. The energy
meter is a rank from 1 to 5, taken from the loudest phrase's onset strength: it
sorts a crate from tool to peak-time record and deliberately claims no more
precision than that.

Location is the whole path, shortened at `~`. When the column is too narrow it
is trimmed from the **front**, so the file name is always the part that
survives — a path clipped at the end is every file in a folder looking
identical.

## Colouring the waveform

Three modes, on the right of the cue row, remembered between runs:

- **bands** — low, mid and high stacked in their own colours. Easiest for
  finding the kick, because the low band is drawn on its own.
- **colour** — one shape, hue mixed from the frequency content: bass blue,
  mid-range amber, treble washing towards white. This is the picture the player
  itself draws, so it is the one to prep against.
- **stems** — one shape, hue from which stem is loudest: vocals rose, melody
  teal, drums amber. Needs a rendered kit, and measures the stem files
  themselves — a band split can say where the bass is, and only a separation can
  say where the *voice* is. Without a kit it falls back to frequency rather than
  drawing nothing.

## The sync sheet

The delta in the dock is the difference between the drive's playlist and what
was last written to it, so a moved cue shows as *changed* rather than as a
re-add. Play counts and tags do not count as changes: they are not things the
player will see.

The sheet lists what would happen, then the preflight — every check on it is a
state that looks fine in a file browser and fails in a booth: 32-bit float
WAVs, sample rates above 96 kHz, paths longer than a player will follow, formats
a player will not open, and whether the write will fit.

The verification that matters runs *after* the write, and belongs to the export
command: the database and every analysis file are read back off the drive by a
parser that shares no code with the writer. Until that passes, the drive is not
finished.

## The log

The dock shows the last line. **LOG** opens the whole thing in a window of its
own — a real one, so it can go on a second screen and stay open beside the
browser without taking anything from the collection. Filter by level, follow the
tail or park it, clear, or copy everything shown.

Everything also goes to `booth.log` in the data directory, and the previous
run's is kept beside it as `booth.log.1` — the run worth reading is usually the
one that just ended badly. `BOOTH_LOG=warn` turns it down; `off` turns it off.
The default is everything.

## Where things are kept

`$XDG_DATA_HOME/booth` on Linux, `~/Library/Application Support/Booth` on macOS,
or wherever `BOOTH_DATA_DIR` points.

- `config.json` — the settings: library folder, what to do about music from
  elsewhere, and whether name edits reach the files. Kept out of the collection
  because copying a library between machines should not bring the first
  machine's idea of where its music lives.
- `library.json` — the collection. Written through a temporary file, so an
  interrupted save leaves the previous one intact. A file that will not parse is
  an error rather than a fresh start.
- `waveforms/` — one cached three-band picture per track, so arrow-keying down a
  crate moves the waveform instead of re-analysing each row.
- `stems/` — rendered stem kits.
- `waveforms/*.stems` — per-stem loudness, for colouring by what is playing.
- `booth.log`, `booth.log.1` — this run and the one before it.

The collection itself never writes audio or changes a tag — it describes what is
on disk, which is what makes it safe to rebuild from a rescan at any point. The
two things that do touch files, copying music in and writing tags, are jobs you
asked for, and neither ever moves or deletes an original.

## What it does not do yet

- No player has read a drive this wrote. The format is validated against an
  independent parser, which is not the same as validation against hardware.
- A CDJ-3000X reads this format only in its compatibility mode. Device Library
  Plus has no public specification.
- Stem colouring needs the kit rendered first, which is minutes a track.
- Key detection is right about 37% of the time on a real library, and confuses
  a key with its relative major or minor about 18% of the time. It is shown with
  its confidence for that reason.
- Cues and grids are never written back to the source files — they live in the
  collection and on the drive. Names can be, on request.
- One deck, and no pitch, sync or mixing — it is for auditioning, not
  performing. The spec asks for two decks eventually.
- Playback resamples linearly, which is right for auditioning and is not what
  anyone would master through.
- Cue colours are assigned by slot rather than chosen.
