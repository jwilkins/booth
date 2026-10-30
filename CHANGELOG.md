# Changelog

Every release Booth has tagged, newest first.

Dates are the day the tagged commit was made, by the clock of the machine that
made it. Two of them disagree with the website's release list by a day, where
that page took the date in UTC instead; the tags are what this follows.

The first and third releases were tagged `0.01` and `0.03` and renamed to
`0.0.1` and `0.0.3`, which is the naming every other release uses and the naming
used here. Both old tags are still in the repository, pointing at the same two
commits as their new names.

## 0.1.1 — 29 October 2026

The tempo's octave taken from what kind of record it is, hot cues that mean the
same thing on every record, the words timed against the record rather than a
separated voice, and drive writes a failing stick cannot leave half done.

There is no 0.1.0. The manifests carried that number from just after 0.0.9 with
no tag behind it, and the release that follows 0.0.9 is this one.

**What kind of record it is**

- The genre, as one of the 400 styles Discogs files records under — Goa trance
  rather than progressive, tech house rather than hard house — heard by the
  Music Technology Group's Discogs-EffNet model, run in pure Rust. It is a
  switch in Settings, **Hear the genre of each track analysed**, off by default,
  and about thirteen seconds a five-minute track; it shows as a Genre column,
  a line in the inspector with the model's confidence, and the genre on the
  drive, and `booth-cli genre` asks it from the command line.
- The model is theirs, under CC BY-NC-SA 4.0, and is neither shipped nor
  downloaded: Booth looks for it in the setting, then `BOOTH_GENRE_MODEL`, then
  `~/.cache/booth/models/`. Its front end was checked against Essentia stage by
  stage: given Essentia's own 16 kHz audio the 400 activations agree to 0.0001,
  and from an MP3 through Booth's own decoder to 0.061, which is what Essentia's
  own resampling qualities differ by on the same file.
- A genre tag is somebody's answer, so the model's genre goes in the way a
  lookup's names do: a file with none takes it, one that agrees is left alone,
  and one that disagrees is asked about. What you choose there is kept, and
  later analyses leave it alone. **Guess genre** on a row's menu listens to one
  track for its genre and changes nothing else.

**Tempo, the grid and the one**

- The tempo's octave goes by the genre. Hip hop at 86 is counted at 86 and drum
  and bass at 172 at 172, and the tempo alone cannot tell them apart. With the
  genre switch on, the genre is heard before the beats are tracked, and each of
  the model's five surest styles votes for the octave nearest the range its
  genre is played at, as sure as the model was of it. The ranges are a table of
  76 club genres in twelve families, from R&B at 70–100 to frenchcore at
  190–220, each with the names it goes by (`genre_tempos.json`). A genre has no
  say where both octaves are more than a third of an octave from its range, and
  an octave is taken at 0.3 or more and ahead of any other.
- Where the model is off or not sure, the file's genre tag does the same, so a
  collection with no model still gets it. A genre felt at half is placed where
  it is written, as madmom and Essentia count it — dubstep and trap at 140 —
  which a listener agreed with on a second listen of three dubstep records.
  Halftime, which the table did not have, is its own genre at 80–90.
- The genre that placed a tempo is kept with the track, so measuring the grid
  again or analysing with the switch off keeps the octave; the logs say when a
  tempo was moved and from what. A tempo given by hand wins.
- The tempo is chosen by the metrical levels around a candidate, not the most
  periodic lag alone, which on a rhythm in threes over the four is a lag in
  threes: VogelAudio's Circuit Breaker read 86.67 where madmom and Essentia
  both have 130, and now reads 130.
- Measured against madmom and Essentia on 75 CC records, counting the 62
  tempos the two agree on or a listener has counted: Booth has 52 without the
  genre and 58 with it. Seven records read at double or half came right; one
  went wrong, a record a listener counts at 92 that the model hears as hardcore,
  which the table plays at 150–200.
- The beats go on the kick. They followed onsets over the whole spectrum, which
  a hat fills and a kick barely touches, and a record with a quiet hat on every
  off-beat had all 67 beats on the hat; on 25 CC records the beats sat half a
  beat off for over a quarter of the way on twelve.
- A record made at one tempo gets one straight grid, through its breakdowns
  too: 22 of the 25 now, 20 of them at a whole number of BPM to within a
  hundredth, where main straightened three. A record whose tempo moves is
  tracked as it was.
- The one is read from the backbeat and the harmony where the kick cannot tell
  — a kick on every beat hits all four alike, and on 8 of 18 records the winner
  led by 1 to 2 per cent. On eleven records with a sure reference every bar line
  is now right, where it was 16 per cent of them.

**Hot cues and the drops**

- A to D mean the same thing on every record, so a hand that has learnt a pad
  reaches for the same moment on the next: **A** the hook sung outside a drop,
  **B** the main break, **C** the hook sung in a drop at least eight bars from
  D, **D** the main drop. Each is left empty on a record that has none of it.
  E to H are the rest in time order, filled by what a set needs next: a way in,
  a way out, a build, an instrumental stretch of eight bars or more to lay
  another vocal over (`Instrumental · 20 bars`), and a second line the room
  sings.
- D is the first drop after the main break, not the loudest: a club record's
  drops are mastered about as hard as each other. B is the longest break, and of
  those within two bars of it the latest, and never one of two bars or less.
- No two hot cues sit within two bars of each other, because pressed, the
  earlier reaches the later before a hand finds the second button; the later
  becomes a marker and the button goes to whatever is next.
- A line the room sings has one button, at its best airing rather than its
  first, and a line inside a longer one ("praise you" in "I have to praise you
  like I should") gives its button to the longer.
- A word cue goes on the bar line before the line where the way in is clear,
  and never inside a word, and a line's start is found in the stem's breath
  before it rather than taken from a lyric timed against another pressing: on
  material with lyric times thrown off by up to 0.7 s, all 40 starts were found
  within 40 ms.
- The main drop and break are found by how much each bar plays — power and how
  far the low band climbs — rather than by the sections' names. With a kit
  rendered, the drum stem finds breaks the mix hides, such as a bassline plucked
  through a breakdown: eight bars of drums below a quarter of their range is a
  break.
- A loop a deck saved survives a stick being read back, and a rekordbox import
  keeps a cue's end; the waveform draws a loop's length along its flag.
- Each cue type is grabbed from its own third of the waveform, dragging a phrase
  boundary takes the section's memory cue with it, and a cue named for the
  words is renamed for the words it is dragged onto.

**The words**

- The recogniser reads the record itself, cut to where the vocal stem is
  singing, rather than the stem, whose separation scars it hears as syllables.
  On a spoken lyric over a synthetic band, eight lines: the stem put none
  within half a second, the whole record two, the record cut by the voice all
  eight.
- Words a lyrics server has only as text take their times from what the
  recogniser hears, lined up in order so a repeated hook's second airing cannot
  land before its first; each heard word now has its own time.
- After every change the words are written beside the track as `<name>.lrc`,
  the file every lyrics program looks for first. A lyric file Booth did not
  write is never written over.
- A lyric file is read for repeats the way a person wrote it: whole lines,
  then the pieces between punctuation, then runs of three words or more.
- The panels are **Lyrics**, **Cue Analysis** and **Lyric Frequencies**, each
  expandable; Cue Analysis opens with the buttons and what each is for, and
  **Copy** and **Copy with times** take the words out.
- The recogniser model worth having is large-v3-turbo; base.en is for trying
  the feature.

**Stems**

- A second separator, **RoFormer**, under **Separator** in Settings and
  `--backend roformer`: a band-split RoFormer for the voice, about three
  decibels cleaner than Hybrid Demucs, and demucs for the rest. It drives a
  locally installed `audio-separator` (Python 3.12 or 3.13, about 1 GB of
  weights), and wants a GPU or Apple Silicon: 29 minutes for 64 seconds on four
  CPU cores. Demucs stays the default.
- Stems fold away under their record, behind an arrow, with → and ← to open and
  close them.
- Long work no longer holds up the track in front of you: the deck's decode,
  a clicked row's picture and a re-measure run on a second worker, and the
  per-track buttons stay pressable and queue. A five-minute MP3 decoded in
  735 ms beside a demucs render, where it had waited about two minutes behind
  one.

**The drive**

- Every file written to a drive is landed whole — written beside its name,
  flushed and renamed — and read back off the device past the page cache, and
  the audio is compared with its source byte for byte. A drive that fails a
  write stops the export at the first failure, names the drive, and leaves both
  databases as the last write made them, so the stick still plays what it did.
- The machine is held awake while a write runs.
- A write that will not fit is refused before a byte is written, with both
  numbers; `--no-space-check` writes regardless. A file of 4 GiB or more is
  refused on a FAT32 stick before the write starts.
- `booth-cli audit <drive>`, and **What each player sees** on the sync sheet,
  read a drive and change nothing: where its two databases disagree, and for
  each generation of player the audio it cannot open and the analysis files
  it will not find. It works on drives other programs wrote.
- A write with no OneLibrary database removes a stale `exportLibrary.db` rather
  than leaving last week's for a CDJ-3000X to browse, and a write with one bad
  file keeps every good file's rows.
- A second write prepares only what changed: one track added to 400 in 0.52 s.
  The figures are on the site's Speed page, and `scripts/speed.sh` takes them
  on your own drive.

**Players and emulators**

- The three-band waveform has been seen drawn on a player from a drive this
  wrote.
- How to try a drive on a CDJ-2000NXS2's own firmware under QEMU, and on seven
  older players experimentally, with two ignored tests that boot a deck on a
  written drive. None of it has been run yet; it is not a hardware result.

**Fixed**

- The `._` files a Mac leaves beside every file on a FAT32 or exFAT drive no
  longer come in as a second, silent copy of every track. Imports and every
  command skip them, the window takes out any already in, and one in the
  library folder or on an external drive is deleted.
- A failed decode no longer leaves the deck loading for good.
- A lyric file with a run of lines stamped at one time no longer puts a cue on
  that time.
- The report under the cues no longer lists every marker as moved from Start.

**The project**

- Tempo measured against two trackers that are not Booth's, madmom and Essentia,
  kept in `scripts/reference/` and in ignored tests; seven records counted by
  ear, and three more agreed.
- A debug build writes `<file>.log` beside each track it analyses, with the
  numbers each decision was made on.
- Six bug reports against rekordbox and other writers turned into tests, which
  found three of the faults above.
- `scripts/make-bundle.sh` takes its version from the manifest when it is not
  given one.
- The website and the README brought up to 0.0.9 and then to this.

## 0.0.9 — 15 October 2026

A drive this writes plays on a CDJ-3000, and a track's loudness can be brought
to a level before it goes on one.

**The drive on a CDJ-3000**

Eight rounds against two players, each one a field a real export carries that
this did not, until a CDJ-3000 on firmware 3.22 browsed a drive and played
from it — from the slot and over Pro DJ Link — and a CDJ-3000X drew colour
waveforms, hot cues and phrases from the same stick. The account is
`docs/onelibrary-gist.md` §6; this is what changed on the drive.

- The legacy database, `export.pdb`, which the CDJ-3000 reads and the 3000X
  does not:
  - the one history row every real export carries, where the table was empty;
  - each data page described the way a real export describes it — the row
    count shifted down by three in the field this wrote a group count into,
    and the fixed tables in the page style of their own that real files give
    them;
  - a header page that points at its table's first data page, with the
    free-slot fill a real one has, where this wrote zeroes and a player had a
    header that led nowhere;
  - every table its own empty spare page, named in the file header's
    directory, where this named one page past the end of the file for all
    twenty tables — which is what had the whole database refused;
  - the browse menu's categories and sort orders, tables 17 and 18, written
    from the same definition as the OneLibrary `category` and `sort` tables
    where they were empty; a date, a bitrate and the id-like word real rows
    carry on every track row; the fill past the count on the tracks' and
    history's header pages; and sequence numbers 2 to 5 on the four fixed
    tables. With these the player saw the database and browsed it.
- The analysis files:
  - the `.2EX` in the order a real export uses, `PWV7` then `PWV6`, and the
    header word on `PWV4` that real files carry;
  - the word rekordbox puts after the column count of `PWAV` and `PWV2`, which
    this wrote with a nibble transposed;
  - an empty `PQT2` in the `.EXT` where rekordbox writes one;
  - the sixteen header bytes every real file carries; hot cues before memory
    cues; the three-band waveforms low, mid, high on the 0–127 scale a real
    file keeps — they were 0–255 and mid, high, low, and a CDJ-3000 hung
    loading every track — with the colour preview's six bytes related as real
    ones are and `PWVC` as the three band gains it is. With these the player
    loads and plays.
- A stem's three analysis files are looked at on every sync, not only its
  parent's, and a grid no player can draw — a file a deck had rewritten with
  beats before the start of the track — is written again without being asked.
  That was a CDJ-3000 crash four syncs had not cleared.
- `--no-onelibrary` on `booth-cli export`, for writing a drive with the legacy
  database alone when a player's trouble has to be pinned to one of the two.
- The window draws its three-band waveform from the drive's own encoding now,
  and redraws any picture cached in the old one.

**Loudness**

- A **Loudness** setting, off by default, with three answers: leave files as
  they are, write ReplayGain tags, or re-encode at the target. Booth had the
  levelling code and no way to ask for it.
- **re-encode** brings a file to -14 LUFS with a -1 dBTP ceiling and writes a
  new FLAC beside the original, carrying its tags across. The collection follows
  the new file and the old one stays where it was — the same shape a conversion
  takes, so a levelling that turns out wrong leaves behind the thing it was made
  from. This is the only one of the two a CDJ hears, because the level is then in
  the audio.
- **tags only** writes ReplayGain tags against the -18 LUFS reference and leaves
  the audio alone. No generation loss, nothing to undo, and no player AlphaTheta
  makes reads them. FLAC and MP3 only: a wav has no standard ReplayGain tag, and
  asked for one the levelling says so per file instead of writing something no
  reader would find.
- A **Level** button on a track and a **Level N** for everything showing,
  neither offered while the setting is off.
- A re-encoded track is measured again, because the loudness, the picture and
  the waveform all came off samples that have changed. A tagged track keeps all
  of it and takes only its new size, which is what the drive check compares
  against.

**The project**

- `CHANGELOG.md`, written from the tags.
- A note in `CLAUDE.md` on what a new tag needs: the three manifests and the
  changelog, neither of which happens on its own.
- rust-analyzer in the container a web session gets, as a second opinion
  beside clippy, which stays the gate.

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
