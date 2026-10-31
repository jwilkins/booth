# To do

Work the repository says is left for later. None of it is marked `TODO` or
`FIXME`: the code flags it in prose, and the READMEs, the spec and the site keep
lists of their own. So each entry quotes the words that flag it, which find the
place again after the line numbers have moved.

Flags that are out of date come last, with the on-screen one first among them.

## Drives and backups

### Read play history from `export.pdb`

`booth/src/history.rs:19`, and the test at `:243`: *"a parser that does not
exist here yet"*. Also `booth/README.md:2106`, the spec's status table (HIST-1)
and the site's *What is next*.

History is read only from `exportLibrary.db`. A CDJ-3000 writes its own into
`export.pdb`, and reading rows back out of DeviceSQL is not written: `inspect`
walks the pages and counts rows without decoding one. A drive played only on
older hardware has a history Booth does not see, and says so rather than
reporting it empty.

### Merge a deck's edits back, and report the set

`docs/rekordbox-replacement-spec.md:877`, HIST-2 and HIST-3: *"are not done"*.

Cues and loops a deck saved are read back off a stick. Merging them into the
collection as a per-track diff is not done, and neither is the set report.

### Notice a deck edit that leaves the analysis files alone

`booth/README.md:2164`: *"A deck that edited a track without touching its
analysis files would go unnoticed"*.

Nobody has published what a CDJ-3000X writes into OneLibrary's edit counters,
so file timestamps do most of the work. Finding out means editing a track on a
deck and reading the counters before and after.

### Check folder depth and files per folder

`docs/rekordbox-replacement-spec.md:864`, SAFE-3: *"Folder depth and per-folder
file counts are not checked"*.

Free space, path length, files of 4 GiB or more on FAT32 and exFAT support are
all checked before a byte is written. These two limits are not.

### Artwork and `exportExt.pdb` on a OneLibrary drive

`docs/rekordbox-replacement-spec.md:862`: *"No `exportExt.pdb`, no artwork"*.

Whether any player needs `exportExt.pdb` is one of the open questions below.
Artwork is plain missing.

### Write the stem sum as a fourth file

`docs/rekordbox-replacement-spec.md:679`: *"it is not done yet"*.

A deck can play melody and drums together as an instrumental, but a CDJ only
has what was written for it. Writing the sum as its own file costs a third more
disk per kit.

### Formats nothing here converts

`booth/README.md:2178` and `:2180`.

Ogg, Opus and WMA are recognised as unplayable but cannot be converted, because
nothing in this build decodes them. A file above 96 kHz is reported and left
alone, because nothing resamples.

### Zip the copied half of a backup

`booth/src/backup.rs:32`: *"the intended next step once there is a month of
these to measure"*. Also `booth/README.md:2083` and the site's *What is next*.

A backup is a directory: `PIONEER/` copied byte for byte, the audio linked to
the library's copy. The plan is to zip `PIONEER/` alone into `PIONEER.zip` with
the links beside it, once there are real backups to measure what it saves. The
comment predicts `export.pdb` compresses well, the analysis files not much and
`exportLibrary.db` not at all, since it is encrypted. Measure that before
choosing.

## On a player

The drive format keeps its own lists of what is left: `docs/onelibrary.md` §10
(*"What is left, in order"*) and §12 (*"Still open"*), and
`docs/onelibrary-gist.md` §7 (*"Still unknown"*). What they still flag, and what
the code adds:

### Try a drive on a player older than the CDJ-3000

`booth/README.md:2119`: *"Nothing older than a CDJ-3000 has been seen to open
one"*. The site's *What is next* puts it first: *"A CDJ-2000NXS2, a stick, and a
morning."*

A CDJ-3000 on 3.22, a CDJ-3000X and a CDJ-1500X play drives Booth wrote. Every
older deck reads the same legacy database the CDJ-3000 does, which is the best
evidence there is short of trying one. `proto/musicai/tests/player_firmware.rs`
can boot CDJ-2000NXS2 firmware under QEMU. That runs nowhere but by hand, and a
real deck is the stronger answer.

### Test against a rekordbox-written control

`docs/onelibrary.md` §10, item 5: *"Done without the control"*.

Still to do: the rekordbox-written stick on the same players for comparison; a
cue set on the player, written back and read by rekordbox; and an OPUS-QUAD or
XDJ-AZ, to find out whether the all-in-ones behave like the 3000X.

### Find which of the CDJ-3000 changes were needed

`docs/onelibrary-gist.md` §7: *"take one away at a time, and nobody has"*.

The browse menu's categories and sort orders, the dated track rows, the sixteen
header bytes of the analysis files, the cue list order and the three-band byte
ranges all went in at once, and together they got a CDJ-3000 to play. Nobody
knows which of them it needed.

### Settle the fields copied without being understood

`docs/onelibrary.md` §10, items 0–1, and §12: *"read from rows beside their
files"*. `proto/musicai/src/export/onelibrary.rs:45` and `:513`: *"whose meaning
nobody has established"*, *"Nobody has published the codes"*.

`fileType`, `analysedBits` and `contentLink` are the values another writer used
on a drive that played, not values read from a real export. `analysedBits` can
be changed without a rebuild (`--analysed-bits`, `BOOTH_ANALYSED_BITS`) so that
it can be tried on a deck. The category and sort rows are the least attested
part of the file. Open one real export (the rekordcrate PR #269 fixture, or any
drive rekordbox 7.2.13+ wrote) with `booth-cli rekordbox schema` and compare it
with what Booth writes, table by table.

Less pressing, since each is written as rekordbox writes it and players accept
it, but none of them is understood:

- the seven bits of the analysis-directory hash that make its first component
  (`export/anlz.rs:66`, *"positions nobody has explained"*);
- `memory_count` in `PCOB`, written `0xffffffff` (`export/anlz.rs:205`);
- the big-endian 1000 after every cue's type (`export/anlz.rs:88`, `THOUSAND`);
- the header word before each analysis section's data (`export/anlz.rs:322`);
- the constant in a table's header page, and the counts the tracks and history
  tables carry there (`export/pdb.rs:575`);
- the second word of each row group (`rekordbox/pdb.rs:28`), and the five in
  the file header (`export/pdb.rs:429`);
- the browse menu's `special` flag, set on MATCHING alone (`export/menu.rs:79`).

### Match the older waveforms' heights to rekordbox's

`proto/musicai/src/export/waveform.rs:15`: *"one of the things to compare
against a rekordbox-produced reference once there is hardware to check
against"*.

The three-band pictures and the colour preview now follow baken's fit against
1,070 real analysis files. The rest (the monochrome preview, the scrolling
monochrome waveform and the colour detail) are still a per-track gain and a
shaping curve that nobody has compared with what rekordbox writes for the same
track. `docs/onelibrary.md` §9 and `booth/README.md:2135` say the same of the
colour waveform's colours: *"a judgement call, not a measurement"*. The hardware
is here now.

The same paragraph links to a [`reference_gain`] that does not exist. The gain
is `gain_for`, at `REFERENCE_QUANTILE`.

### The rest of the open questions

`docs/onelibrary.md` §12:

- Does any player need `exportExt.pdb`? A OneLibrary-only player has a `myTag`
  table of its own, which suggests not.
- Which players need the `.2EX`, and what a player does with the three gains in
  `PWVC`.
- Does a CDJ-3000 on 3.22 read `exportLibrary.db` when `export.pdb` is absent?
  FableGear reports one that did.
- What `myTagMasterDBID` is for.

## The collection and the window

### My Tags

`proto/musicai/src/export/onelibrary.rs:189`: *"the collection has no such thing
yet"*.

A drive gets rekordbox's four My Tag groups, empty, so a player's tag editor has
somewhere to hang its own. The collection keeps no tags, so none of Booth's
reach a drive.

### Type in a genre by hand

`booth/README.md:2154`: *"there is no way yet to type one in by hand"*.

A genre from the file's tag is read in and defended, and the model can suggest
one, but a DJ cannot set one.

### Choose cue colours

`booth/README.md:2177`: *"Cue colours are assigned by slot rather than chosen"*.
Also the spec's status table, PREP-1..5: *"bar user-chosen cue colours"*.

### Keep a grid a player bent

`booth/README.md:2160`.

The collection keeps a tempo and a downbeat, not every beat, so a grid a deck
bent cannot be held, only the tempo it was written at. That is why a track whose
drive copy is kept is not written again.

### Correct the rest of the analysis by hand

The site's *What is next*: *"What is left is the rest of the analysis"*.

The words, the beat grid and the phrase strip can be corrected by hand. Nothing
else the analysis decides can be.

### Keep the collection in SQLite

`docs/rekordbox-replacement-spec.md:869`, LIB-1: *"the swap is still owed"*.

The collection is a JSON document. LIB-1 asks for SQLite under a published
schema.

### Key detection

`booth/README.md:2155` and the spec's status table: *"right about 37% of the
time on a real library"*, and confusing a key with its relative major or minor
about 18% of the time. It is shown with its confidence for that reason.

### A second deck

`booth/README.md:2174`, the spec's status table (PREP-6) and the site's *What is
next*: *"The specification asks for one eventually"*.

## Stems

### Score a separation, and level the stems on the drive

`docs/rekordbox-replacement-spec.md:873`: *"Quality scoring (STEM-6) and the
drive's loudness pass (STEM-4, STEM-5) are not"*.

## Not started

`docs/rekordbox-replacement-spec.md:876` and `:878`.

- Serato, Traktor and Engine DJ (INTEROP-2).
- A plug-in interface (INTEROP-4).

## Releases and the site

### Sign and notarise the macOS app

`.github/workflows/macos-app.yml:63`: *"Set BOOTH_SIGN_IDENTITY and
BOOTH_NOTARY_PROFILE here once there is one"*. Also `scripts/package-macos.sh:126`:
*"Gatekeeper will refuse this on another Mac until it is"*.

The disk image goes on each release unsigned, so anyone downloading it has to
right-click and choose Open the first time. `package-macos.sh` already signs and
notarises when the two variables are set. What is missing is a Developer ID
certificate in the repository's secrets, and the workflow passing them in.

### Record the site's clips

`site/booth.js:8`: *"files nobody has recorded yet"*. Also
`site/media/README.md`, *The clips*.

`site/index.html` asks for seven clips, and `site/media/` has none of them:
`one-window`, `cues-from-words`, `lyrics-lookup`, `stems-ahead`, `memory-cues`,
`writing-a-drive`, `drive-copies`. The README says what each should show and how
to encode it, and dropping the `.mp4` in is the whole job. The memory-cue clip
asks for the cue on a CDJ-3000X's screen as well, which nothing else on the site
can stand in for.

## Flags that are already done

Words that still say something is missing after it was done. The follow-up is
to bring them up to date.

### The sync sheet warns about what has since worked

`booth/src/app.rs:13148` and `:13173`, the two warnings under *Target* in the
sync sheet.

This one is on screen. Writing both databases, the sheet says a CDJ-3000X
browses the drive but its *"waveforms and grids have not been seen on a player
since the naming they are found by was corrected"*. They have, on a CDJ-3000X and
a CDJ-1500X, with hot cues and phrases (`booth/README.md:2109`). For an older
target it says *"no player older than a CDJ-3000X has yet been seen to browse a
drive written here"*, and its comment says the CDJ-3000's refusal on 2.05 *"is
not known to have been fixed"*. A CDJ-3000 on 3.22 plays these drives, so the
line is now the CDJ-3000. No test pins either string.

### The spec's opening still says a CDJ-3000X will not play a drive

`docs/rekordbox-replacement-spec.md:60`, *What it will not do*: *"It does not
yet write a drive a CDJ-3000X is known to play"*. It ends *"If a 3000X is what
the club puts in front of you, this is not your tool yet"*.

Twenty lines further down, the same document says three players including the
3000X have played one, and its status table, *"current as of 0.1.1"*, agrees.

### `anlz` says `export.pdb` is not written

`proto/musicai/src/commands.rs:76`: *"a database (`export.pdb`) that this does
not write yet"*.

`booth-cli export` has written `export.pdb` for some time (`export/pdb.rs`,
`Database`). The comment still describes the `anlz` subcommand as half a drive
waiting for the other half.

### The OneLibrary reader says writing one is unproven and undecided

`proto/musicai/src/rekordbox/onelibrary.rs:18`: *"What is missing is
evidence"*, *"a decision this project has not made"*.

It says nothing shows a hand-written `exportLibrary.db` playing on a
OneLibrary-only player, and that writing one means a key decision not yet made,
*"So this module reads and describes"*. A CDJ-3000X has since played drives
Booth wrote and drawn their waveforms without measuring its own
(`export/onelibrary.rs`, *"What has and has not been seen on a player"*), and
the build ships `BUNDLED_ONELIBRARY_KEY` to write them.

### The "what is left" list in `docs/onelibrary.md` §10

Items 2–4 (write the database from the same collection as the pdb, keep the two
in step, name analysis directories by the hash and add `PWVC`) are what §9 says
Booth now does. Item 6 says *"Decide the key and licence question before
shipping"*, and the build now ships the key. If that was decided on purpose,
write the decision down and close the item. If not, it is still open, and the
gist's note on Mixxx ruling the file out on DMCA §1201 grounds is the reason to
decide it deliberately.

### Which value marks a point cue

`proto/musicai/tests/rekordbox_export.rs:502`: *"on the list of things to
settle against a player"*.

The format docs say a point cue is type 1, and `rekordcrate` says 0. Booth
writes 1, and the test was written to survive either answer. Hot cues have since
come up on a CDJ-3000X and a CDJ-3000 (`docs/onelibrary.md` §9 and §10), which
looks like the answer. If it is, say so in the doc comment.
