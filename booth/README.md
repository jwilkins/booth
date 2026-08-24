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

- `library.json` — the collection. Written through a temporary file, so an
  interrupted save leaves the previous one intact. A file that will not parse is
  an error rather than a fresh start.
- `waveforms/` — one cached three-band picture per track, so arrow-keying down a
  crate moves the waveform instead of re-analysing each row.
- `stems/` — rendered stem kits.

Nothing in the collection writes audio or changes a tag. It describes what is on
disk, which is what makes it safe to rebuild from a rescan at any point.

## What it does not do yet

- No player has read a drive this wrote. The format is validated against an
  independent parser, which is not the same as validation against hardware.
- A CDJ-3000X reads this format only in its compatibility mode. Device Library
  Plus has no public specification.
- Key detection is right about 37% of the time on a real library, and confuses
  a key with its relative major or minor about 18% of the time. It is shown with
  its confidence for that reason.
- Nothing is written back to the source files: tags, cues and grids live in the
  collection and on the drive.
