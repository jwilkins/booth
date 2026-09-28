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

**Any of the five sections can go to a window of its own** — the collection,
the browser, the prep editor, the inspector, the drive dock. The **⧉** menu on
the query bar sends one out and brings it back; so does closing the window it
went to, so nothing can be shut out of existence. Which ones are out is
remembered between runs, because which screen the waveform lives on is exactly
the sort of thing that is annoying to set twice. The log has always had a
window of its own and is listed in the same menu.

The query bar stays in the main window whatever else leaves, and it still
filters a list that has gone to another screen: it is the browser, and the two
being in different windows does not change that.

**Drag a panel's edge** to resize it, and it stays there — the size is written
to the settings when the drag ends and is what the panel opens at next time.
Only a drag changes it: a panel is drawn at exactly the size it was given and
its contents are clipped to that, so a long path in the inspector or a long
playlist name in the collection no longer widens the panel it is in, and
squeezing the window does not overwrite what you dragged to.

**The collection panel scrolls.** Forty playlists are more than any panel is
tall; they used to run off the bottom of the screen, and once panels began
clipping to their own rectangles they stopped being drawn at all. Scrolling is
what turns being clipped into being reached. A track can be dragged onto a
playlist that had to be scrolled to, the same as onto one that was already
showing.

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

## Choosing rows

The arrow keys walk the list. **Hold shift** and they take everything they pass;
**shift-click** a row takes everything between it and where you started;
**⌘-click** (control on Linux and Windows) adds or removes one row on its own;
and **⌘A** takes everything the query has left showing — everything showing
rather than the whole collection, because narrowing to what you want and then
taking all of it is what the query bar is for.

A range grows from an anchor, which is the last row picked plainly, so holding
shift and pressing down four times takes five rows and coming back up again
gives one of them back rather than leaving it behind.

**One selected row is not a selection.** It is where the cursor happens to be,
so the batch buttons still read "Analyse 40" and act on everything showing.
Pick two or more and they narrow to those, and the label beside the playlist
field changes to say so. Stem companions come along with their parents rather
than being picked on their own: a stem is not a thing to analyse or put on a
drive by itself.

## Doing one track at a time

Every batch job has a single-track twin. The prep editor under the waveform has
**Analyse**, **Look up tags**, **Render stems** and **Cue from the words** for
whatever is selected, and right-clicking any row offers the same four plus play,
copy-into-library, copy the path, and remove.

A job that has already been done says so: the button reads **Re-analyse** rather
than **Analyse**, because the honest answer to "will this take twenty minutes"
is different for a first pass and a re-run. Re-analysing is the same code path
as analysing — there is no separate re-do, which is how the two would come to
disagree.

**Hold shift and the batch Analyse button becomes Re-analyse**, acting on
everything showing rather than only the tracks that have never been done. That
is the button to reach for when what analysis produces has changed — a new
waveform, a different grid — and every track in the collection is holding an
answer from the old code. The label changes with the key, not just the hover
text, because the two act on different numbers of tracks and a button has to
say what pressing it will do. The query bar still decides what the batch is, so
narrowing the list narrows the work.

Right-clicking does not move the selection, and the menu names the track it will
act on. A stem companion offers only play and copy-path: it is a file its parent
owns, and analysing it separately would put a second answer beside the one it
inherited.

## Playing

Double-click a row, or press **space**, to hear the selected track. Clicking the
waveform moves the playhead and takes playback with it; pressing a cue jumps
both. That is the point of having a deck at all — a cue placed by eye is a
guess, and the ear is what says whether it is on the beat.

**Switching between a track and its stems keeps your place.** Pick the acapella
while the track is playing and it comes in where the track had got to, still
running; go back to the original, or across to the drums, and the same. That is
what the comparison is for — is the vocal clean through the drop, is the groove
still there without it — and a comparison you have to re-cue by hand is not one
anybody makes twice. Paused, the playhead still moves across so pressing play
picks up there, without starting a sound nobody asked for. It works from the
arrow keys as well as the pointer, since a stem sits directly under its parent
in the list.

Between two different records nothing carries: the new one starts at its own
beginning rather than at wherever the last one's playhead happened to be.

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

**The phrase strip is editable.** A detector working from onset strength gets a
good many boundaries right and some plainly wrong, and a wrong one is worth
less than none — it is a lie about where the drop is, on the strip a player
draws. So **drag a boundary** to move it, snapped to the bar like the memory
cue: a section that starts three beats into a bar is a section in the wrong
place however carefully it was dragged. **Right-click a block** to rename it —
every name is offered, not just the ones the track already uses, because a menu
listing only what is there cannot correct anything — or to **split** it where
the pointer is, or **join** it to the one before. Splitting is how a boundary
the detector missed gets put in; joining is how one it invented is taken away.

Both halves of a split keep the name until one is given another: a split says
the boundary was missed, not that the name was wrong. A boundary cannot be
dragged over its neighbour, and a section too short to have two halves is not
split at all rather than split into a sliver.

**Each block says how long it runs for** — `BREAK 16`, `BUILD 8` — because a
DJ builds in eights and sixteens, and seeing that a breakdown is the usual
length is the difference between reading the strip and counting the bars. The
number is the count of red bar marks under the block, taken off the same grid
the picture draws rather than worked out again from the tempo: two answers to
one question come apart, and it showed as a section labelled sixteen with
fifteen marks under it. A boundary dragged into the middle
of a bar loses that bar rather than rounding up to it, which is what the marks
show too.

The strip scrolls as well as edits: away from a boundary, dragging moves the
view with the pointer, and what was under your finger stays under it. At full
width there is nowhere to scroll to, so nothing moves.

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

The header stays put when the list is scrolled, and it is where the columns are
worked. **Drag a boundary between two headings** to make one wider and the one
beside it narrower; the pair keeps its total, so nothing further along the row
shuffles under the pointer. **Right-click the header** for which columns there
are at all: every column is listed whether it is showing or not, so one turned
off can be turned back on, and **Reset to default** puts the whole lot back.
The last column cannot be turned off — the menu to undo it lives on the header.

Widths and which columns are on are remembered between runs, alongside the sort
and the panel sizes. The list always fills the window: what is left over goes to
the last column, and a window too narrow for the widths scales them all down
rather than pushing one off the edge, because a column you cannot see is
indistinguishable from one you turned off.

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
The phrase strip underneath goes through the same zoom, so a block stays under
the music it names; grab it away from a boundary and drag to scroll the view.
**fit** next to the colour modes, or **esc**, goes back to the whole track.

It used to draw the whole track at every zoom and carry the window as a box
over itself, which made it a map but put the drop's block nowhere near the
drop as soon as you zoomed in. A strip that lines up with the picture is worth
more than a map of a track you can already see the whole of at full width.

While something is playing, the view follows the playhead — but only once the
playhead has actually left it. Recentring every frame would be a scrolling
waveform, which is a different instrument; what is wanted here is that the thing
you zoomed in on does not vanish while you listen to it.

Only a **running** playhead pulls the view, and never while the pointer is
down. A parked one is not going anywhere, and pulling the view back to it
anyway undid every scroll on the frame after it was made — which showed up as
a phrase strip that stuttered and never got more than a few points from
wherever the playhead was sitting.

The picture is cached at the scrolling resolution — 150 columns a second, the
same detail the player draws from — rather than as a fixed 1,200 columns for the
whole track. At a normal window it is the same picture either way, because a
pixel takes the peak of whatever it covers; the difference is that zooming in
has something to find. It costs about 135 kB a track, against 30 MB for its
stems. Zooming stops where the picture runs out: past about one stored column
per two pixels it would be stretching rather than revealing, and a staircase
drawn confidently invites placing a cue against an edge that is not there.

## How tall the waveform draws

The picture is scaled against the track's own loudest content before anything
is drawn, so it says how a record is put together rather than how hot it was
mastered. Without that a modern master is a solid block: the height curve ran
out of room near the top and a drop, a build and a breakdown all drew at full
height, which is a waveform with the arrangement taken out of it.

**The height is a weighted blend of the bands, not the peak of the signal** —
0.6 low, 0.3 mid, 0.1 high. A modern master is limited, and limiting is
precisely the business of making the peak the same from moment to moment:
measured column by column across one loud section of one, the peak has a
spread of 0.00. It is a straight line, and no scaling or curve-bending recovers
a shape from a straight line, which is why a loud track drew as a solid block
however its gain was worked out. Through the same section the mid band, the
high band and the overall RMS sit at 0.04 to 0.07; the low band is at 0.36. On
a limited record the kick is the one thing still moving, and it is what a
waveform is read for, so it leads.

The other two still carry the level, which is what stops this being a bass
meter: a breakdown that takes the drums out but keeps a loud pad draws at about
a third of full height rather than the tenth the low band alone would give it.
It looks thinner than a section with the drums in, which is the useful part —
a passage with no kick in it *should*.

Two scales, though, not one. How tall a column is drawn and what it is made of
do not live on the same scale — the blend sits below any single band — so one
gain cannot serve both: set from the blend it drives the band bytes past the
top of their range, and set from the bands it leaves the height using a third
of the display. The colours are ratios between bands and do not care either way.

**The scrolling waveform and the overviews are scaled separately**, because
they are two different measurements of the same track. A scrolling column is a
peak; an overview column is a second of audio averaged, and on anything with
transients in it that sits well below the peaks. One reference cannot serve
both: worked out on the averages and applied to the peaks it comes out about
twice too large, and every hit flattens against the top until the picture is a
solid block. That is what it did.

For the scrolling waveform the reference is the 99th percentile rather than the
maximum: a single freak column — one clap that clips on an otherwise quiet
record — would otherwise set it, and the whole picture would be drawn against a
moment nobody is looking for. Not the loudest twentieth, though. That was the
setting when the height was a raw peak, and on a limited master half the track
sits within a hair of the peak, so the 95th percentile falls *inside* the
loudest passage and draws its median at 29 of 31 with the kicks clipped off
above it. Measured on one, 0.99 doubles the movement visible inside a loud
section and drops that median to 23, which is where a kick has somewhere to go.

For the overviews it is the maximum, because a second of audio averaged is not
something a single clap can carry, so the outlier a percentile guards against
cannot arise there. One overview reference across all three of them, so the
strip under the deck and the small picture in a browse list agree.

A very quiet transfer is lifted, up to thirty decibels, which is more than any
real recording needs and short of what it takes to make a noise floor look like
music.

One gain for the whole track and every band in it: the height of a column says
how loud that moment is, and the ratios between the bands are what the colour
is made of, so scaling them apart would wreck both. The consequence to know is
that the picture no longer says anything about how loud one record is against
another — it is about the shape of the one you are looking at. The loudness
figures in the columns are what answers the other question.

The overview pictures — the strip under the deck on a player, and the small one
in a browse list — average a window of about a second, wider than the spacing
between their columns, so consecutive columns overlap. At twelve hundred
columns a whole track, one column is a third of a second, which is less than a
beat at any tempo anybody plays: windows that merely touch each other resolve
individual kick drums and draw a comb whose spacing is the sampling rather than
the music. A second spans a beat at every tempo, so what is left is how much is
going on, which is the arrangement and what an overview is for. A drop's edge
blurs across about three columns of twelve hundred, which is the price of the
rest and not visible.

The scrolling waveform is not summarised that way. Its columns are a
hundred-and-fiftieth of a second, a kick drum is several of them wide, and the
peak is the right measurement at that size — which is exactly why it needs a
reference of its own rather than the overview's.

The same measurement makes the pictures on the drive, so what is on screen
while prepping is what will be on the CDJ's screen.

**The grid marks the downbeat red and the other three beats white**, which is
what a player draws and therefore what a DJ reads without having to think about
it. One colour for all four made the one indistinguishable from the rest at a
glance, which is the one thing a beat grid exists to show.

### What the collection keeps of a grid

An even grid is a tempo and a downbeat, and that is all the collection stores
for one: thousands of beat times saying what two numbers already say belong in
the analysis file on the drive, not in the library. The picture rebuilds the
rest, winding back to the head of the track in whole **bars** from the
downbeat. Winding back a beat at a time put beat one on whichever beat happened
to land nearest the top of the track, and the bar marks, the bar number in the
transport and the length on a phrase block then all counted from an offbeat:
right on a track that starts on the one, and three beats out on one that does
not.

**A grid that bends is kept beat for beat.** A live take, a disco record, or
one somebody bent by hand on a player is a grid no tempo can put back, and it
is usually the only copy of that work — rebuilding it from a tempo on the way
back out would hand the drive a flattened version of what the drive gave us.
Which it is gets decided on the way in, by asking whether an even grid through
the two ends misses any beat by more than five milliseconds. Five, because an
even grid rounded to whole milliseconds is already off by up to one (the
analyser here measures 0.85 ms across three minutes) and five is far below the
point where a beat sounds like it is somewhere else. A grid that speeds up and
comes back is caught by the bulge in the middle rather than passed for landing
in the right place.

**The drive gets every beat either way.** A CDJ reads beats, not tempos, so the
`PQTZ` section is always a full per-beat list with a bar position and a tempo
against each one — an even grid is written out beat by beat from its tempo, and
a bent one beat by beat from what was kept. No database column changes: the
OneLibrary `content` row carries `bpmx100` and points at the analysis file, and
the grid has always lived in the file.

**Where the one is has a field of its own.** Saying "the one is here" and
saying "start the track here" are two things a DJ does and rarely means the
other, so the grid's downbeat is no longer read off the memory cue. A
collection written before that field existed still phases off the cue, which is
where the phase used to come from.

Everything that crosses between a moment in the track and a place on the panel
goes through the zoom: the columns, the cue flags, the playhead, the grid, where
a click lands, and where a cue is grabbed. They did not all, and the ones that
did not were fine at full width and wrong by the width of the panel as soon as
you zoomed in — a cue drawn in one place and picked up in another, and a grid
that stayed put while the music moved out from under it.

**A stem row draws its own waveform.** An acapella is a different sound from the
record it came from, and a picture of the mix under the acapella's name is a
picture of something that is not playing. Everything else a stem row shows stays
the record's — the grid, the cues, the phrases, the key — because those are
properties of the record and a cue that did not line up with the one on the
track would be worse than no cue at all. The waveform is the one thing that
belongs to the file rather than to the record.

Analysing a record draws its stems with it, so a crate that has just been
prepared is prepared — waiting to click on each acapella in turn is not the same
thing. Rendering a kit does the same as soon as the stems exist.

A cached picture records what it is a picture of: the files it was drawn from,
and what they looked like. The cache is keyed by row, and without that note a
row whose audio changed underneath it goes on showing the old picture for ever
with nothing saying so. Two ways that happens — a kit rendered again, and the
bug this note was added for, where a stem row was drawn from its parent's mix
and the mix's picture was filed under the stem's name. A picture that cannot
vouch for itself is drawn again, which is what heals a library full of acapellas
showing the record. A file that cannot be reached is a different matter: nothing
can be said either way, and a row whose drive has been unplugged is better
showing the last picture of it than an empty strip that reads as silence.

## Colouring the waveform

**Red is the bass, green the mid-range, blue the treble** — the convention every
other DJ program draws waveforms in, and the one the EQ colour charts a DJ has
already learned are drawn from. It is not an arbitrary choice: with the three
bands on the three channels, a column made of two of them lands on the secondary
that names the pair.

| in the column | comes out |
| --- | --- |
| bass alone | red — baseline, intro, outro |
| bass + mid | yellow — melodic baseline, bridge |
| mid alone | green — vocals, melodies |
| mid + treble | cyan — vocals, verse |
| treble alone | blue — hi-hats, buildup |
| bass + treble | magenta — the beat |
| all three | white — the chorus |

Nothing cancels, because no two primaries sit opposite each other. That was the
fault this replaced: the palette was blue bass and amber mid, which *are*
opposite, so a column holding both — most music — cancelled to grey. A body with
a strong bass and a breakdown with none came out at hue 36.0 and hue 35.8, the
same colour for the two passages a DJ most needs to tell apart. The same two
columns now land 25 degrees apart, and a kick against a lead is a clean 120 —
the full distance between two primaries.

The same three numbers go into the analysis files, so the picture on screen and
the picture on the deck are the same picture. They had drifted apart, and the
comment claiming they had not was out of date by a release.

Brightness is deliberately left out of the colour. The height of the column is
already the loudness, and a colour that said it again would leave a quiet
breakdown too dark to read for the sake of repeating something the shape has
already shown.

The levels are also undone before they are mixed. They are stored bent by the
curve that makes heights readable, and it flattens the bands against each other
on the way — a column that is plainly a kick reads 0.95 low against 0.71 mid
through it. A colour is a set of proportions, so it has to be worked out on the
amplitudes the curve was applied to. And the sharpening that decides how much
the quieter bands may tint the mix went from three to two: at three the leading
band took so much of it that nothing could tint anything, and every column came
out the colour of whichever band led — the mid, nearly always.

Three modes, on the right of the cue row, remembered between runs:

- **bands** — low, mid and high stacked in their own colours, tallest first so
  the shorter ones land on top. Easiest for finding the kick, because the low
  band is drawn on its own.
- **colour** — one shape, hue added from the frequency content: bass red,
  mid-range green, treble blue, per the table above. What the player draws, so
  it is the one to prep against.
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

## Cues from the words

The **Words** button, and **Cue from the words** on a row, set a track's hot cues
from what is sung on it rather than from how loud it gets.

A cue placed by energy can only ever find the loud part. That is worth having —
the drop is where most mixes turn — but the moment a DJ actually reaches for is
usually the line the crowd sings, and a hook and the verse before it are the same
loudness, the same instruments and the same key. Nothing in a spectrum tells them
apart. The words do.

So the pass is three steps, each skipped when it has already been taken:

1. **Render the vocal stem**, if there is not one. A recogniser handed a club
   record transcribes the kick drum; an isolated voice is the only thing it has a
   chance with. This is the expensive step — minutes a track — and it is the same
   separation the **Stems** button runs, so a track that already has a kit skips
   straight past it.
2. **Read the stem**, with whichever Whisper is installed. What comes back is
   timed lines.
3. **Find what repeats.** Lines that say the same thing are grouped — loosely,
   because a recogniser writes the same sung phrase four slightly different ways
   and matching word-for-word would count a hook sung eight times as eight
   different lines. The group with the most separate airings is the hook.

**One cue per line, not one per airing.** A hook sung six times was six cues
saying the same thing — a player that could jump to one moment of the record,
with the drops and the breakdowns pushed out of the set entirely. Each of the
track's repeated lines is now cued once, where it first lands, and at most three
lines get a cue at all: the hook, a second line and a tag is already generous,
and everything past that is a slot taken from a drop.

Around them go **where the singing starts** and **the start of every phrase** the
arrangement analysis found — intro, build, break, drop, outro. A player holds
eight hot cues and a busy track offers more than eight moments, so they are
ranked: the hook outranks even the drop, because a drop can be found by looking
at the waveform and the line the crowd sings cannot be found by looking at
anything. The other lines sit below the breakdowns, so the arrangement keeps the
slots the words give back. Two moments that land on top of each other become one
cue, and it keeps the words: a drop that is also where a line falls says which
line.

Cues from the words are rounded **down** to the beat rather than to the nearest
one. A sung line rarely starts on the beat — a pickup is the whole point of a
pickup — and a hook cue that clips its own first word is one nobody presses
twice.

Two things it will not do. **An ad-lib is not a hook**: a line needs at least two
words, because "yeah" is the most repeated thing in half the vocal stems ever
recorded and marks nothing. And **a recogniser stuck in a loop does not invent
one**: airings less than four seconds apart are one airing, which is true of a
chorus that sings its line twice over and true of Whisper emitting "thanks for
watching" forty times over a breakdown.

The words are kept in the collection once they have been heard, so cueing the
same track again is instant and costs no stem render and no recogniser. The
memory cue is never moved — the grid is anchored to it — but the hot cues are
replaced wholesale, which is what the button says it does.

### Installing a recogniser

Booth does not ship a speech recogniser and will not download one. Set it up
under **Words** in Settings:

- **whisper.cpp** — `brew install whisper-cpp`, or build it. Needs no Python and
  no network, and needs a ggml model file naming: `ggml-base.en.bin` is a good
  first choice. This is the one to reach for.
- **OpenAI's `whisper`** — `pipx install openai-whisper`. Name the program
  `whisper` and it is called the Python way; anything else is treated as
  whisper.cpp. It downloads its weights the first time it runs, and shells out to
  ffmpeg, so it is only offline afterwards.

`BOOTH_WHISPER_BIN`, `BOOTH_WHISPER_MODEL` and `BOOTH_WHISPER_LANGUAGE` are used
when the corresponding setting is empty, so the feature can be tried without
editing a file. Setting the **language** is worth doing: left to itself the
recogniser guesses it off the first few seconds, and the first few seconds of an
isolated vocal are usually a breath.

Whether there is a recogniser is checked **before** anything is rendered. A
separation that finishes and only then finds there is nothing to hand the stem to
has wasted the expensive half of the work. Nothing else in Booth needs any of
this — only the words do.

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

**Which drive** is a menu in the dock, listing every one the collection knows
with the ones actually in a socket marked as such, and offering to add another
or forget the one in use. Forgetting takes a drive off that list and touches
nothing on the stick.

Adding used to be offered only while the list was empty, and switching only
while it held more than one, so setting up a first drive took away every way to
reach a second: one entry, no picker, no add button. A drive that is not there
now says *not plugged in* rather than sitting in the dock looking ready — a
remembered drive is a place and a history, not a stick, and the two read
identically until something asks.

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

### What goes on the drive is what the collection says

The exporter listens to every file it prepares, because it has to — the waveform
is of the audio and nothing else can supply it. But it used to write *everything*
it heard, including the cues and the phrases, so a cue moved by hand was
faithfully marked as changed, faithfully rewritten, and faithfully replaced with
whatever the analyser thought that time. The edit went nowhere and nothing said
so.

Now the collection's cues, phrases, key and tempo are what get written, and the
measured ones are the fallback for a track that has none. Cue names and colours
go with them, which they never did before — so renaming a cue now counts as a
change the drive has not got. **The first sync after this upgrade reports every
track as changed**, once, because the drive genuinely does not have them.

### When the player has edited it too

A CDJ-3000X can move a cue, re-grid a track or rename a phrase on the deck, and
it writes that back to the stick. So a sync is not a copy — it is two sides that
may both have moved.

Opening the sync sheet reads the drive first: for each track it was written, the
analysis files beside it and the `hasModified`, `cueUpdateCount`,
`analysisDataUpdateCount` and `informationUpdateCount` columns in the OneLibrary
database. Two independent pieces of evidence, because neither is enough alone.
The counters are the field the format keeps for exactly this question — but what
a player writes into them is not documented and nobody has published a reading
of one, so a drive showing no change there has not said it was not edited. The
analysis files cannot argue: a deck that rewrote a track's cues rewrote the file
that holds them.

Three cases, and only one of them is a question:

- **Changed only here** — the sync writes it, as it always did.
- **Changed only on the drive** — nothing here changed, so nothing here is
  going to be written over it, and the sheet says nothing.
- **Changed in both places** — the sheet asks. Each track is listed with when
  each side was last edited, starting on whichever is the later, and there are
  buttons to take all of one side.

Keeping the drive's copy does two things. The track's analysis files on the
stick are left exactly as the deck left them — it is not prepared again, and its
row and its place in the playlists carry through untouched — and **what the deck
did is read back into the collection**, so the cues, their names and colours, the
phrases and the tempo become the ones Booth shows. Keeping the drive's copy
therefore means having it, not merely not losing it. The decision is recorded, so
a settled question is not asked again on every sync.

The one thing that does not come back is the grid itself, only the tempo it was
written at. A collection keeps a tempo and a downbeat rather than thousands of
beat times, so a grid a deck has bent cannot be held here without being
flattened — which is exactly why the track is not written again afterwards. The
files on the stick stay as they are, and what comes back here is what can be
shown beside them.

If a track's analysis files cannot be read — pulled mid-write, or rewritten into
something this cannot parse — the drive's copy is still protected by leaving it
alone, and the sheet says that is all that happened rather than passing over it.

A drive written by a build from before any of this existed has no record of what
it looked like, so its tracks are never treated as edited: no evidence is not
evidence of a change, and the alternative would make every older drive
unwritable.

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

Every line starts with the date and time it happened, as ISO 8601 in UTC —
`2026-09-04T21:14:03Z`. A log is read next to things that have clocks of their
own: a file's modification time, yesterday's log, somebody saying their drive
stopped working about half nine. "412.008 seconds into some run" cannot be
lined up with any of those, and a stamp anything can parse means nobody has to
do the conversion in their head. The window keeps the run's own elapsed clock
beside it for the times you want to know how long something took.

Everything also goes to `booth.log` in the data directory, and the previous
run's is kept beside it as `booth.log.1` — the run worth reading is usually the
one that just ended badly. `BOOTH_LOG=warn` turns it down; `off` turns it off.
The default is everything, and everything means everything: which file was
imported and what its path was taken to mean, what each drive state came out
as and which file changed it, every track a check disagreed with, each session
read off a drive, and the machine and version at the top of every run.

Two things get written down in more detail than the rest, because they are the
two that take minutes and are hard to see into afterwards.

**A drive write** says where it is going and what it opened, and then, for each
file: what it decoded and how long that took, where on the drive it is going
and how much of the length a player will follow that uses, what its grid and
cues came out as — or, for a stem, whose grid it took — how many bytes were
copied and how long the copy took, and the analysis files written and read
straight back. Then the database: built, written, read back off the drive and
walked, in both formats, each timed. And a closing line with the whole run's
time and what ended up on the drive.

**A copy of a drive** says what the library had to match against, each of the
drive's own files as it is carried, each audio file as it is linked to the
library's copy and which file that was — or why it could not be linked, which
is usually a backup on another filesystem — and each one copied or noted
because the library has not got it. Then the time each phase took.

That is a few lines per file, so it sits at the log's most detailed level and
`BOOTH_LOG=info` turns it off without losing the results. In the command-line
tool the same detail is `-v`, on stderr, so redirecting the results still
captures only results.

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
- Cueing by the words needs a recogniser installed separately, and a stem kit
  rendered first — so the first track costs minutes twice over. What it hears is
  whatever Whisper hears: a heavily processed vocal, a language it was not told
  about, or a chopped-up sample comes back as noise, and a hook found in noise is
  a cue in the wrong place. The inspector shows how many lines were heard and
  what it decided the hook was, so a bad reading can be seen to be one; there is
  no way yet to correct it by hand.
- Key detection is right about 37% of the time on a real library, and confuses
  a key with its relative major or minor about 18% of the time. It is shown with
  its confidence for that reason.
- Cues and grids are never written back to the source files — they live in the
  collection and on the drive. Names can be, on request.
- A grid a player bent cannot be held in the collection, only the tempo it was
  written at, because a collection keeps a tempo and a downbeat rather than
  every beat time. That is why a track whose drive copy is kept is not written
  again.
- What a CDJ-3000X actually writes into the OneLibrary edit counters after an
  edit on the deck has not been published by anyone, so the file timestamps are
  doing most of the work. A deck that edited a track without touching its
  analysis files would go unnoticed.
- The reader for those analysis files was written from the same understanding of
  the format as the writer, so the two agreeing says nothing about whether
  either matches rekordbox. What it is for is reading back a file this program
  wrote and a player has since edited, which it is tested on; the check against
  an independent parser is the round trip in `rekordbox_export`.
- One deck, and no pitch, sync or mixing — it is for auditioning, not
  performing. The spec asks for two decks eventually.
- Playback resamples linearly, which is right for auditioning and is not what
  anyone would master through.
- Cue colours are assigned by slot rather than chosen.
- Ogg, Opus and WMA are recognised as unplayable but cannot be converted here:
  nothing in this build decodes them.
- Nothing resamples. A file above 96 kHz is reported and left alone.
