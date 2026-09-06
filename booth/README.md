# Booth

A DJ library that prepares tracks and writes the drives a Pioneer/AlphaTheta
player reads. It is the interface described in
[`docs/rekordbox-replacement-spec.md`](../docs/rekordbox-replacement-spec.md),
built on the analysis and export code in the `booth-cli` crate next to it.

```
cargo run -p booth
```

Anything named on the command line is imported at startup, and files can be
dropped onto the window.

On Linux the audio output needs ALSA's headers at build time
(`apt install libasound2-dev`, or `alsa-lib-devel`). macOS and Windows need
nothing extra. Without a working output device the window still opens and does
everything else; the transport says `no audio out` instead.

On macOS, `scripts/package-macos.sh` builds this window into `Booth.app` — a
universal binary with the `booth-cli` command-line tool alongside it in the same
bundle, and a disk image to install it from. See the packaging section of the
[top-level README](../README.md#the-macos-app).

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

**Names.** The inspector's title, artist, album and year are editable. What
reaches the file itself is a setting with three levels:

- **never** — the collection keeps the names to itself.
- **only where the file is blank** (the default) — a fingerprint lookup that
  names an untagged file writes those names in; nothing already there is
  touched.
- **always** — every edit and every match rewrites the file's tags.

The middle one is the default because filling in a blank is not the same act as
overwriting somebody's answer. A lookup that identifies an untagged file has
found out something true about it, and leaving that only in the collection means
the file stays anonymous to every other program that opens it — while replacing
an artist somebody typed by hand is exactly the thing worth being careful about.

The inspector's **Write these into the file** always overwrites, whatever the
setting says: pressing it is somebody saying "these ones, now". FLAC, MP3, AIFF
and M4A — a WAV has nowhere to put them, and says so rather than appearing to
work.

An `.m4a` carries iTunes-style atoms, and the freeform ones go under
MusicBrainz Picard's names, so anything else that reads them finds them where
it looks. A file that will not parse says which kind of not-parsing it is — a
renamed download, a truncated file, or a protected purchase — rather than
"cannot tell what kind of file this is".

**Tags.** Free-form, and the collection's own — they never reach the drive, so
tagging a track does not queue a 40 MB rewrite. A tag already in use is offered
as one click, and a new one that differs only in case joins the existing tag
rather than starting a second one that queries miss.

## Doing one track at a time

Every batch job has a single-track twin. The prep editor under the waveform has
**Analyse**, **Look up tags** and **Render stems** for whatever is selected, and
right-clicking any row offers the same three plus play, copy-into-library, copy
the path, and remove.

A job that has already been done says so: the button reads **Re-analyse** rather
than **Analyse**, because the honest answer to "will this take twenty minutes"
is different for a first pass and a re-run. Re-analysing is the same code path
as analysing — there is no separate re-do, which is how the two would come to
disagree.

Right-clicking does not move the selection, and the menu names the track it will
act on. A stem companion offers only play and copy-path: it is a file its parent
owns, and analysing it separately would put a second answer beside the one it
inherited.

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

### What the path says

A file's own path is evidence, and usually good evidence.
`Peverelist/Tessellations/02 - Roll With The Punches.flac` names the artist,
the release, the track number and the title, in a layout that has been the same
since people started keeping music in folders. So it is read: at import, for
the fields the tags leave empty, and at identification, where it does two
things a fingerprint cannot.

- **AcoustID has never heard of it.** Which is the normal state of affairs for
  a white label, a promo, an edit, a bootleg — most of what is actually played.
  There is no match to weigh, and a strongly structured path is then the best
  evidence on the disk. It is taken, and the log says so.
- **AcoustID says something else.** A fingerprint is about the audio and a path
  is about what somebody filed it as, so a confident disagreement between them
  is a question rather than something to settle by rule — an edit filed under
  the original's name, or a fingerprint that landed on the wrong pressing. The
  sheet shows both answers and a **Use the path** button beside **Use this**.

A path is only acted on when it names both an artist and a title from
somewhere that means one. Folders that are a filing system rather than a name —
`Music`, `Downloads`, `FLAC`, `320`, `Various Artists`, `CD2` — are never taken
for an artist, a hyphen inside a word stays inside it (`Re-Up`, `Jean-Michel`),
a leading `02 - ` is a track number, and `(www.somewhere.com)` is trimmed off.
Nothing read off a path ever overwrites what the file's own tags say.

### Before writing tags into a file

Tagging is the one thing here that writes to your files, so it has a check of
its own: if a file's name has almost nothing in common with the names about to
go into it, the write is held back and put to you first. `track04.mp3` about to
become *Peverelist — Roll With The Punches* is the shape of a fingerprint that
found the wrong record, and forty of those is a bad afternoon. A file named
after the title alone passes — plenty are — and so does one whose name differs
only in case and punctuation.

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
every sort, and exist only when all three stem files are on disk. **They play**:
double-click an acapella and you hear the vocal stem. An instrumental is the
melody and drum stems summed as it loads, because the separator writes parts and
never a mix of some of them — there is no single file to point at, and playing
one of the two would be an instrumental missing half of itself.

The Location column and the right-click **copy the file path** on a companion
name its stems rather than its parent's file: a row that names somebody else's
path is worse than one that names none.

## The energy meter

Five blocks, a rank rather than a measurement: what it has to do is sort a crate
so the tools are at one end and the peak-time records at the other. It is taken
from the onset density of the loudest fifteen seconds of the track — the peak,
because what decides where a record sits in a crate is how hard it goes at its
best, not how much of it is intro.

Measured off the frames rather than off the detected sections, so a track with
no grid still gets one: not knowing where the bars are is no reason to claim not
to know how busy it is. Everything above 16 kHz is left out, because it is the
first thing a lossy codec throws away and counting it would score the same
record lower as an MP3 than as a FLAC — a fact about the file, not the music.
An empty meter means nothing was measured, which is not the same as the quietest
possible record.

The five thresholds are a calibration table, and the raw figure is shown beside
the rank in the inspector (`3/5 · 0.072`) and logged for every analysis. If a
library comes out lopsided, that number is the evidence for moving them.

Location is the whole path, shortened at `~`. When the column is too narrow it
is trimmed from the **front**, so the file name is always the part that
survives — a path clipped at the end is every file in a folder looking
identical.

## Where you are in a track

The transport reads `12.3 · -172`: bar 12, beat 3, with 172 bars left. Bars and
beats both count from one, the way a DJ counts out loud — `1.1 1.2 1.3 1.4 2.1`
— and it fits in the width a timecode would take. The elapsed half is a
position and the remaining half is a count, so they are deliberately written
differently: printing both as `12.3` would invite reading a remainder as a
place in the track. The clock is on the hover.

Settings switches the whole thing to beats. A track with no grid has no bars to
count in, so it shows the clock instead of inventing a position.

## Zooming the waveform

The wheel zooms about the pointer — what you were looking at stays where it is
rather than sliding off while you chase it. Shift, or a sideways wheel, pans.
The phrase strip underneath always shows the whole track, so it doubles as the
map: the window is drawn on it, and clicking anywhere on it jumps there. **fit**
next to the colour modes, or **esc**, goes back to the whole track.

While something is playing, the view follows the playhead — but only once the
playhead has actually left it. Recentring every frame would be a scrolling
waveform, which is a different instrument; what is wanted here is that the thing
you zoomed in on does not vanish while you listen to it.

The picture is cached at the scrolling resolution — 150 columns a second, the
same detail the player draws from — rather than as a fixed 1,200 columns for the
whole track. At a normal window it is the same picture either way, because a
pixel takes the peak of whatever it covers; the difference is that zooming in
has something to find. It costs about 135 kB a track, against 30 MB for its
stems. Zooming stops where the picture runs out: past about one stored column
per two pixels it would be stretching rather than revealing, and a staircase
drawn confidently invites placing a cue against an edge that is not there.

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

## Stem quality

Two choices in Settings, **high** by default:

- **high** — `htdemucs_ft` with two shifts. Four specialist models rather than
  one, each run twice more at small offsets and averaged: roughly eight times
  the work of demucs' own defaults, and noticeably cleaner.
- **standard** — `htdemucs`, no shifts. Demucs' own defaults.

A kit is rendered once and then played for years, so the slow one is the
default; the fast one is for a first pass over a whole library. On the command
line it is `--quality high|standard`, and `--demucs-model` / `--demucs-shifts`
still override whichever it picked.

## Where stems go

**Beside the track** by default — `Sirens.flac` yields `Sirens-vocals.mp3` in
the same folder. A kit belongs to one record, so keeping it next to that record
means copying the folder takes the stems with it, every other tool sees them,
and there is no second place to remember to back up.

The alternative is one folder for all of them, which is the case for a library
on a small disk and stems on a big one. Both places are searched whichever is
set, so changing the setting never makes a rendered kit disappear — it is
minutes of work a track, and a preference must not look like a delete.

## Importing a rekordbox library

rekordbox keeps its library in `master.db`, a SQLCipher-encrypted SQLite file.
The key is the same on every installation — it is not derived from your machine
or your licence — and this build carries it, so importing a library is a matter
of pointing at the file. The Settings field and `REKORDBOX_KEY` are there for
the day AlphaTheta changes the key; the constant is `BUNDLED_KEY` in
`proto/musicai/src/rekordbox/mod.rs`, and blanking it builds a program that asks for one.

Settings → **Import a rekordbox library** brings across tracks, playlists and
their folders, beat grids, hot cues, keys, ratings, play counts and My Tags.
On the command line, `booth-cli rekordbox read <path>` lists what is in one
without changing anything.

**Nothing already here is overwritten.** Tracks are matched by file path —
the only thing the two libraries genuinely share — and what comes across is
what is *missing*: names on an untitled file, a grid where there is none, cues
where there are none. rekordbox's opinion of a file is not better for being
older, and where this program has measured something itself, that measurement
is the one the waveform was drawn from and the cues were placed against. Half
of each would be worse than either.

Some things have no equivalent and are taken whenever rekordbox has more of
them: play counts, because they are history this program was not around for;
My Tags, which become tags; and the star rating, which becomes a tag like `4★`
because there are no stars here and losing it entirely would be worse.

Importing the same library twice changes nothing the second time.

## OneLibrary, and what it would take

`exportLibrary.db` is what a CDJ-3000X reads instead of `export.pdb`. It is
SQLCipher too, and its key is fixed as well, so the encryption is not what
stands in the way — and neither, any more, is the schema. AlphaTheta has
published nothing, but other people have taken the format apart in public:
twenty-two tables, the DDL from a real export, the seed rows that draw the
player's browse screen, the analysis files, which player reads which database.
[`docs/onelibrary.md`](../docs/onelibrary.md) is that survey, with its sources
and with the claims — including some of this project's own — that turned out to
be wrong.

What is missing is evidence rather than knowledge. Hand-written drives have
been reported playing on a CDJ-3000X and on a CDJ-3000, by the projects that
wrote them; nobody has published a test of a OneLibrary-only drive on a
OneLibrary-only player, or checked whether the player used the grids and
waveforms it was given or quietly measured its own. And writing the file means
using a recovered key, which is a decision this project has not made.

So a sync writes one: the same track list and the same playlist tree as
`export.pdb`, from one source, so the two files on the drive cannot come apart.
The key it is encrypted with is fixed for every drive there is and this build
carries it, as it carries the one for rekordbox's own library — they are
different keys, and both are in `proto/musicai/src/rekordbox/mod.rs` with where they came
from. The sheet says which databases a drive will carry before it writes them.

**A CDJ-3000X has read one.** Playlists, track list and key search all came up
off a drive this wrote, which is the first hardware evidence the database is
right — and it is evidence about the database, not the drive. Whether the
player used the analysis files it was given or measured its own on load is a
separate question, still open, and the one where this program is most likely to
be wrong.

## What a player will actually open

A CDJ-3000 takes **MP3 and AAC at 44.1–48 kHz**, and **WAV, AIFF, FLAC and
ALAC at 16 or 24-bit up to 96 kHz**. An `.m4a` is fine — it is an MP4 container
holding AAC or ALAC, and the player reads both.

What is not fine, and is checked when a file is imported rather than when a
drive is written:

| | |
| --- | --- |
| a container nothing opens | offered a conversion, if it can be decoded here |
| 32-bit float WAV | offered a conversion to FLAC |
| above 96 kHz | reported; resample it in an editor first |
| a protected purchase | reported; nothing here can convert one |

Import is the moment to ask, because it is the moment there is still time to do
something. The check before a write is the last chance to catch a file that
will not load, and by then the only answer is to leave it behind.

Converting writes a **FLAC beside the original** and points the collection at
it. The original is never touched, moved or deleted: a conversion that turns
out wrong should leave the thing it was made from behind. Names come across
with it.

Two things are deliberately not offered. **Resampling**, because there is no
resampler here worth writing a library through, and doing it badly once is
permanent in a way that saying so is not. And a **format nothing here
decodes** — an offer that would fail is worse than no offer, because it costs
the time to find out.

A protected file is not a broken file: it plays perfectly in whatever sold it.
It is encrypted, and the only way to a playable copy is to get an unprotected
one. It is flagged at import from the container's brand — including when it has
been renamed to `.m4a` — so it turns up when it is added rather than on the
night.

## The sync sheet

The delta in the dock is the difference between the drive's playlist and what
was last written to it, so a moved cue shows as *changed* rather than as a
re-add. Play counts and tags do not count as changes: they are not things the
player will see.

**Carry stems** puts each track's vocals, drums and melody on the drive with
it. They go in the same folder as the record they were cut from — filed under
its artist, not their own, which is what keeps them together when the stems are
wavs and have nowhere to keep a tag — and they follow it in the playlist, so
the browse list reads track, vocals, drums, melody and a companion is a turn of
the encoder away from the record it belongs to. Each one takes its parent's
grid, cues, key and phrases rather than being listened to alone: a vocal with
no drums under it would produce a grid of its own, and a hot cue that does not
line up with the one on the track is worse than no cue at all. Only tracks with
a whole kit rendered are affected. It is about three times the audio, and the
same again in analysis — a stem is a row on the player, with a waveform and a
grid of its own on the drive.

A drive is written once and then added to, so the second write is given only
what changed: the rest of the database is carried through from what the last
write recorded, rather than every track being decoded again. A stem is carried
on the same terms as the track it came from and never on its own — it takes the
parent's grid and cues, so a track whose prep changed is three stems whose
analysis is now wrong, and they go on again with it. Anything that cannot be
carried — a collection written before its rows were kept, a kit re-rendered to
another format, a write that failed — is simply prepared afresh, which costs a
decode and is always right. **Write it all again** drops the record entirely,
for a drive something else has been at.

The sheet lists what would happen, then the preflight — every check on it is a
state that looks fine in a file browser and fails in a booth: 32-bit float
WAVs, sample rates above 96 kHz, paths longer than a player will follow, formats
a player will not open, and whether the write will fit.

The verification that matters runs *after* the write, and belongs to the export
command: the database and every analysis file are read back off the drive by a
parser that shares no code with the writer. Until that passes, the drive is not
finished.

## The collection against its files

A collection is a set of claims about files other programs can also move,
retag and delete. **Check** reads them back and says where the two have come
apart: a file that is not where it should be, one that changed size, one
rewritten at the same length (which only a thorough check can see, since it
means reading every byte), a stem the kit lists and the disk has not got, and a
tag that answers something the collection answers differently.

Most of those are facts about the file, so the collection is simply out of date
and *Take the files' word* brings it up to date in one go. Nothing there is
written back to a file: a name goes into a file's tags through the tag
write-back and nowhere else.

One kind is not a fault, and is treated differently. **A file's own name is
evidence too**, and when it says something else entirely — the collection has
*Peverelist — Roll With The Punches*, the file is filed as
`Batu/Marius/01 - Marius.flac` — there are two answers and both might be right.
It could be a fingerprint that landed on the wrong record, a file somebody
renamed, or an edit filed under the original's name; only a person knows which.
So it is shown as a pair of radio buttons, the tags on one and the file name on
the other, and nothing happens until you press the button that takes the ones
you switched. It is never swept up by *Take the files' word*, which is a button
somebody pressed to mean something else.

That comparison is only made when the path names both an artist and a title.
`Downloads/track04.flac` disagrees with everything and has nothing to offer in
place of it, and reporting that would be a complaint about somebody's filing
rather than a difference they can settle.

## The log

The dock shows the last line. **LOG** opens the whole thing in a window of its
own — a real one, so it can go on a second screen and stay open beside the
browser without taking anything from the collection. Filter by level, follow the
tail or park it, clear, or copy everything shown.

Every line starts with the date and time it happened — `20260904 21:14:03` —
in UTC. A log is read next to things that have clocks of their own: a file's
modification time, yesterday's log, somebody saying their drive stopped working
about half nine. "412.008 seconds into some run" cannot be lined up with any of
those. The window keeps the run's own elapsed clock beside it for the times you
want to know how long something took.

Everything also goes to `booth.log` in the data directory, and the previous
run's is kept beside it as `booth.log.1` — the run worth reading is usually the
one that just ended badly. `BOOTH_LOG=warn` turns it down; `off` turns it off.
The default is everything, and everything means everything: which file was
imported and what its path was taken to mean, what each drive state came out
as and which file changed it, every track a check disagreed with, each session
read off a drive, and the machine and version at the top of every run.

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
  crate moves the waveform instead of re-analysing each row. At 150 columns a
  second, so it can be zoomed into: about 135 kB for a five-minute track.
- `stems/` — rendered stem kits, when they are set to go in one folder rather
  than beside their tracks.
- `waveforms/*.stems` — per-stem loudness, for colouring by what is playing.
- `booth.log`, `booth.log.1` — this run and the one before it.

The collection itself never writes audio or changes a tag — it describes what is
on disk, which is what makes it safe to rebuild from a rescan at any point. The
two things that do touch files, copying music in and writing tags, are jobs you
asked for, and neither ever moves or deletes an original.

## Copies of every drive

A stick is hours of work living on the cheapest thing in the booth. The audio
on it is replaceable; the cues, the grids, the playlist order and the history a
player wrote back after a gig are not, and they are a few files under
`PIONEER`.

So when a drive is written, or a prepared one is plugged in while this is
running, those files are copied into `booth-drives` beside the library —
databases, analysis, artwork, settings, the lot. The audio is **linked** to the
library's own copy rather than copied, so a 64 GB stick costs a few megabytes
and a directory entry per track. Each copy is a plain folder with a
`backup.json` saying what was found, so getting one back onto a stick is a copy
with no tool in the middle.

A drive is stored once per state: leaving it plugged in does nothing, writing
to it and plugging it in again stores the new state beside the old one.

What counts as a state is the drive's own files under `PIONEER` — their paths,
sizes and modification times — and deliberately *not* the breadcrumbs an
operating system leaves on a mounted volume. macOS writes `.DS_Store` and `._`
companions on a stick as soon as anything looks at one, and rewrites them
afterwards; counted as changes, they make a drive that is different every time
it is looked at, and a drive copied every time it is looked at. A drive that
changes on its own anyway is copied at most once every five minutes, and the
log says which file will not hold still. A drive this program writes is never
held back by that: a write is a real change and is stored at once.

A track counts as the library's if a file there has the same name and length,
which is true of everything on a drive this wrote. Where that fails the drive's
file is hashed the way the duplicate finder hashes one — the audio alone, tags
skipped — so somebody else's copy of a record you own is linked rather than
stored again, however they named it.

What is left is music the library genuinely has not got, and there are three
things Settings can do with it:

- **Note what was on it** (the default) — the databases and analysis are kept
  and the music is named in the manifest, not stored. Costs nothing, and the
  music is gone if the drive is.
- **Copy it into the backup** — the copy holds the music too and can be put
  back on a stick as it was. Costs whatever the drive holds that you do not.
- **Copy it into the library** — the music lands under the library's own artist
  folders and joins the collection, where it can be analysed and played. A file
  already there is never written over.

The default is the cheap one deliberately: plugging in a stranger's stick is
not a decision to spend gigabytes.

Nothing is archived yet. A whole drive cannot be, since a zip holds contents
rather than links — but the copied half can: `PIONEER` could become one
`PIONEER.zip` with the links beside it, which is the next step once there are
enough of these to see what they cost. The log line after each one says how
much was carried, which is the number that decides it.

## What was played comes back

A player writes a history to the stick it played from: every track it loaded,
in the order it loaded them, one session a night. It is the only thing on a
drive the collection cannot produce for itself, and it is the record of what was
actually played rather than what was prepared — so when a drive is copied, its
history is read back and becomes playlists.

They land in a folder named after the drive, with each session under the name
the player gave it, which is a date. Reading the same drive again replaces those
playlists rather than making a second set. A track the drive played that the
library does not have is left out and counted, rather than making a playlist
with holes in it that look like tracks.

Two limits worth knowing. The folder is `History/<drive>` — the collection's
playlists have one level of folder, not two, so that is a name with a prefix
rather than a folder inside a folder. And the history is read from the
OneLibrary database, which is where a CDJ-3000X and every other newer player
writes; a CDJ-3000 writes its history into `export.pdb` instead, and reading
individual rows back out of that format is a parser this does not have yet. A
drive played only on older hardware has a history nothing here can see.

## What it does not do yet

- **A CDJ-3000 cannot read a drive this writes.** The legacy `export.pdb` is
  refused outright by one on firmware 2.05 — the file parses under two
  independent parsers, so what is wrong is something a parser tolerates and a
  player does not. Only the newer players work today.
- Phrase data and the three-band waveform have not been seen on a player. The
  colour waveform, beat grid and hot cues have; those two sit in parts of the
  analysis files nobody has reported on yet.
- The waveform colours are a judgement call rather than a match: what rekordbox
  puts in those three bits for given audio is not published, and ours has never
  been compared against a real export column by column.
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
- Ogg, Opus and WMA are recognised as unplayable but cannot be converted here:
  nothing in this build decodes them.
- Nothing resamples. A file above 96 kHz is reported and left alone.
