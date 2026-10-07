# Changelog

Every release Booth has tagged, newest first.

Dates are the day the tagged commit was made, by the clock of the machine that
made it. Two of them disagree with the website's release list by a day, where
that page took the date in UTC instead; the tags are what this follows.

Two releases carry a second tag under the older naming: `0.01` is the same
commit as `0.0.1`, and `0.03` the same as `0.0.3`. They are listed once, under
the name the rest of the releases use.

The crates all say `version = "0.1.0"` in their `Cargo.toml` and always have.
That number is not the release number and has never been bumped; the tags are
the release history.

## 0.0.8 — 7 October 2026

Correcting a beat grid by hand, and putting the words a track sings where they
can be read.

**The beat grid**

- The grid controls a player has had for twenty years: halve and double the
  tempo, nudge and shove the whole grid, slide it until a beat lands on the
  playhead, and say which beat is the one.
- The tempo shown where it can be typed over, with buttons for a tenth of a BPM
  either way — the unit a long mix drifts by.
- The readout says whether a track's tempo moves: `128.00 · dynamic · 512 beats`,
  or steady.
- **re-measure** listens again and takes the tempo and the beats, and nothing
  else — unlike re-analysing, which rewrites the cues, the phrases and the key.
- Beat marks: alt-click the waveform or press **mark**, then **fit** works a
  grid out from them. A plain tempo and downbeat wherever one explains the
  marks, and a grid that bends only where none does. Marks need not be next to
  each other or evenly spaced.
- **make steady** turns a grid kept beat by beat into a tempo and a downbeat.
- While the grid is being corrected the marks cross the waveform instead of
  ticking along the bottom of it, so a line is visibly on a kick or visibly
  beside it.
- A track start cue now lands on a bar line rather than on whatever beat was
  tracked first.

**The words**

- The whole lyric is shown in the inspector, each line with the moment it
  lands, and what cueing from the words decided is kept and shown under it —
  that account used to go to the log and nowhere else.
- Every line a track comes back to, scrolled, rather than the first four.
- A hot cue is read by the line it lands on: `Hot cue B — "hold me closer now"`.
- Right-clicking a cue button gives its name, its colour, and turning a hot cue
  into a memory cue or back.
- The lyrics lookup can be asked for a whole crate at once, and from the panel
  that shows the words.
- Phrases are numbered the way a player draws them instead of all being called
  two, and a section marker says how long the section runs.

**Drives and players**

- A **CDJ-1500X on firmware 1.10** mounts a drive this writes and plays from
  it, which makes two current players confirmed rather than one.
- A database row that would not fit a page is no longer written: a drive write
  failed on a 4049-byte row, three bytes inside the limit it was asked about
  and one past the real one.
- "Render stems again" renders them again.

**Fixed**

- Correcting a grid no longer throws away the zoom, and no longer re-decodes
  the whole file to redraw a picture the grid cannot change.
- Opening the grid controls no longer pushes the measurements line — and the
  only way to close them — off the bottom of the window.
- A grid anchored at the start of a track can be nudged earlier.
- The grid, the cue flags and the mapping from a click to a moment are drawn
  against the same view as the music.
- Naming or clearing one of several memory cues acts on that one. Every memory
  cue carries the same letter, so acting by letter acted on whichever sorted
  first — and clearing one cleared all of them.

**The project**

- A website at <https://jwilkins.github.io/booth/>.
- A written working agreement in `CLAUDE.md`: a branch per set of work, a pull
  request when the set is done, and the checks that run before it goes up.

## 0.0.7 — 4 October 2026

The words looked up rather than only listened for.

- A lyrics server is asked before anything expensive: taken outright where the
  evidence is strong, put up for a yes or no where it is plausible and not
  certain, and correctable by hand either way.
- A server saying a record is an instrumental saves the separation and the
  recogniser pass that would have ended in "nothing was sung".
- The recogniser's own doubt about its answer is kept and shown when it is
  poor.
- The repeats a recogniser never punctuated are found anyway.
- A track's words reach the player as its comment, hooks first — so what a long
  lyric loses to the row limit is its last verse and never the line that
  identifies the record.
- No database page is written with a row cut in half.

## 0.0.6 — 1 October 2026

Grids that hold, and the line the room sings.

- A record at one tempo is gridded at one tempo however untidily its beats were
  tracked, so eight beats from anywhere in it are eight beats from anywhere
  else. Scatter and drift are told apart by their shape rather than their size.
- What a track keeps saying is kept and shown: the lines it comes back to, how
  often each is sung, and where each one lands.
- Every verse marker carries the whole sung line, and says which line it is for.
- Shift asks for any kind of work a second time.
- A batch of separations says what it will cost before it spends it.
- Three tests stopped reading the machine they run on.

## 0.0.5 — 30 September 2026

Beat grids that hold still, and drives checked against the booth they are going
to.

- A metronomic track stopped alternating between two wrong tempos every beat.
- Lyric cues moved onto the stem rather than onto the recogniser's clock, which
  anchors its first segment at zero however long the intro runs.
- A drive is checked against the oldest player it has to work on, and anything
  that player could not open is carried as a 320 kbps MP3 while the library
  keeps its lossless file.

## 0.0.4 — 29 September 2026

Every marker named, and drives that behave.

- Cues placed from what is sung on a track, with the line a track keeps coming
  back to found first.
- Memory cues named, and each return of a line marked.
- A hot cue no longer hides under a memory cue, and a line is cued once rather
  than once for every time it comes round.
- Eleven colour schemes.
- A second drive can be added, reached and told apart from the first, and a
  drive stopped re-copying itself every five minutes.
- A grid that bends is kept, with its own downbeat.
- What the player changed is read back, and writing over it is asked about
  first.
- Tracks that will be slow to reach are called out at import.
- Every control says what it does, and a test fails on one that does not.

## 0.0.3 — 10 September 2026

The waveform made legible.

- Drawn against the track rather than against full scale, as an envelope rather
  than a comb, with the loudest band first so the kick is visible at all.
- The phrase strip became correctable by hand.
- Any section of the window can be sent to a window of its own.
- Switching between a track and its stems keeps your place.
- A track's stems go on the drive beside it, and survive a second write.

## 0.0.2 — 5 September 2026

One name each, and all three of them Booth's.

- The engine, the command and the app settled on the names they have now.
- What a file's own path says it is became a source of names for the records no
  database has heard of.
- Tagging asks first when a file is named nothing like its tags.
- A drive stopped being copied over and over.

## 0.0.1 — 4 September 2026

The first one: the point at which this was a program rather than a prototype.

- The one-window interface from the specification — query bar, collection,
  browser, prep editor, inspector and drive dock.
- Both on-drive databases written from one collection, with the analysis files
  every generation of player reads.
- **A CDJ-3000X reads a drive this writes.**
- Tempo, beat grid, key, phrases, cues and colour waveforms, all measured
  locally.
- Loudness normalisation and stem separation, with stems carried to the drive
  as tracks sorted under their parent.
- Tracks identified by sound through AcoustID and MusicBrainz.
- rekordbox's own encrypted libraries read, so a collection can be brought
  across.
- Duplicates found by hashing, with what each copy knows merged into the one
  kept.
- A copy of every drive written or plugged in, and the player's own history
  read back into playlists.
- The macOS app, the licence, and CI on every push.
