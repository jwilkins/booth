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

Stem companions are indented under the track they came from, and exist only
when all three stem files are on disk. The energy meter is a rank from 1 to 5,
taken from the loudest phrase's onset strength: it sorts a crate from tool to
peak-time record and deliberately claims no more precision than that.

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

The collection itself never writes audio or changes a tag — it describes what is
on disk, which is what makes it safe to rebuild from a rescan at any point. The
two things that do touch files, copying music in and writing tags, are jobs you
asked for, and neither ever moves or deletes an original.

## What it does not do yet

- No player has read a drive this wrote. The format is validated against an
  independent parser, which is not the same as validation against hardware.
- A CDJ-3000X reads this format only in its compatibility mode. Device Library
  Plus has no public specification.
- Key detection is right about 37% of the time on a real library, and confuses
  a key with its relative major or minor about 18% of the time. It is shown with
  its confidence for that reason.
- Cues and grids are never written back to the source files — they live in the
  collection and on the drive. Names can be, on request.
- Nothing plays. The playhead is a position for placing cues against, not a
  transport.
- Cue colours are assigned by slot rather than chosen.
