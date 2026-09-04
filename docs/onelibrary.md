# OneLibrary, as far as it is known

What a CDJ-3000X reads off a USB drive, established from primary sources and
checked on 3 September 2026, with the hardware results this project has of its
own marked as such. It is research rather than a claim of player support, and
it says whose observation each finding is.

It exists because the rest of this repository used to say that OneLibrary's
schema "has never been published" and that "nobody has demonstrated" a
hand-written one on a player. Neither is true any more, and §11 lists what else
turned out to be wrong.

## The short version

- The **CDJ-3000X, CDJ-1500X, XDJ-AZ, XDJ-AN, OPUS-QUAD and OMNIS-DUO read one
  database off a drive**: `PIONEER/rekordbox/exportLibrary.db`, called
  OneLibrary and, until October 2025, Device Library Plus. They do not fall back
  to `export.pdb`. The CDJ-3000X manual says "This unit only supports
  OneLibrary"; the rekordbox FAQ, the USB export compatibility table and
  AlphaTheta's March 2026 notice say the same, and no firmware since has changed
  it (CDJ-3000X 1.40, July 2026, is current).
- **Every other player, the CDJ-3000 included, reads only `export.pdb`.** The
  CDJ-3000's one firmware that read OneLibrary (3.30) was withdrawn within weeks
  and the current one (3.22) says in its own notes that it "does not include
  OneLibrary support". rekordbox has written both databases to every exported
  drive since 6.8.1 (December 2023).
- **The file is an ordinary SQLite database encrypted with SQLCipher 4 at its
  default settings**, under one fixed passphrase that is the same for every
  drive in the world. The key is public, recoverable from the rekordbox binary,
  and published in half a dozen projects; this one carries it too, in
  `src/rekordbox/mod.rs`.
- **The schema is known in full.** Twenty-two tables, documented by at least six
  independent readers and three writers, with a real rekordbox export checked in
  as a test fixture in a rekordcrate pull request. §4 reproduces the DDL from
  that export.
- **The gap is hardware evidence, not documentation.** A drive this project
  wrote has been browsed on a CDJ-3000X — playlists, track list, key search
  (§7). Two other projects report the same for their own writers: dj-usb-tkit
  on a CDJ-3000X (firmware 1.31, 2 September 2026) and FableGear on a CDJ-3000
  with no `export.pdb` on the stick at all. What nobody has reported, here
  included, is **whether the player used the analysis files it was handed or
  measured its own** — the difference between a drive that browses and a drive
  that plays as prepared.
- **There is no specification, SDK, licence or developer programme.** The
  partners (Algoriddim, Native Instruments) got the format privately. The Mixxx
  project has decided not to write the file at all, on anti-circumvention
  grounds; §8 records that position. Whether this project shares it is a
  decision for the project, not for this note.

## 1. Names, and when each thing happened

Three names for two things. *Device Library* is the legacy `export.pdb` drive
(2009–). *Device Library Plus* is the encrypted SQLite drive AlphaTheta
introduced for the OPUS-QUAD in 2023. *OneLibrary* is Device Library Plus
renamed, when it was opened to Algoriddim and Native Instruments in October
2025; AlphaTheta's help centre says the two "are basically the same, and are
fully compatible with each other".

| Date | What | Source |
|---|---|---|
| 2023-03-07 | rekordbox 6.6.11 adds Device Library Plus as an opt-in export, "OPUS-QUAD ONLY" | 6.6.11 release notes |
| 2023-12-08 | AlphaTheta's *Device Library Plus User's Guide*: DLP "since 2023", coexists with Device Library on one drive, the two managed separately, conversion one-way | the guide (PDF) |
| 2023-12-19 | rekordbox 6.8.1: "The rekordbox library is now always exports in both Device Library and Device Library Plus formats, for USB storage device use on all DJ equipment." | 6.8.1 release notes |
| 2024-03 | The static key is known to the community (pyrekordbox issue #125) | pyrekordbox |
| 2025-02-06 | rekordbox 7.0.9: an export to a legacy-only drive converts it to DLP first | 7.0.9 release notes |
| 2025-07-24 | rekordbox 7.1.4: edits and deletions are synchronised between the two on-drive libraries | release notes |
| 2025-08-22 | pyrekordbox gains `devicelib_plus` (read and write) | pyrekordbox git |
| 2025-09-09 | CDJ-3000X announced; the announcement says nothing about library formats | AlphaTheta |
| 2025-10-21 | OneLibrary launched with Algoriddim and NI; rekordbox 7.2.5 "Renamed Device Library Plus to OneLibrary"; djay 5.5 ships it; CDJ-3000 firmware 3.30 adds it and prefers it | release notes, change history |
| 2025-11 | 3.30 withdrawn after drives showed empty on CDJ-3000s; AlphaTheta tells venues to roll back to 3.20 | DJ Mag, Digital DJ Tips |
| 2026-01-07 | AlphaTheta: "the CDJ-3000 has reverted to using the Device Library format"; it is "reviewing the specifications of Device Library and OneLibrary" | pioneerdj.com notice |
| 2026-01-15 | CDJ-3000 firmware 3.22: "This update does not include OneLibrary support." 3.30 is gone from the change history | 3.22 change history |
| 2026-03-17 | rekordbox 7.2.12 withdrawn: its USB export wrote tracks that would not load on hardware; fixed in 7.2.13 | rekordbox notice |
| 2026-03-18 | AlphaTheta's USB notice: one format per player, 7.2.11 the minimum rekordbox, CDJ-3000X firmware "will support more flexible library loading" | alphatheta.com |
| 2026-07-02 / 07-09 | CDJ-1500X and XDJ-AN launched, OneLibrary-only; rekordbox 7.2.16 supports them | release notes |
| 2026-07-14 | CDJ-3000X firmware 1.40: dark mode, internet updates, Wi-Fi duplication. Nothing about libraries | 1.40 change history |
| 2026-07-29 | FableGear: a OneLibrary-only stick plays on a CDJ-3000 | FableGear PR #131 |
| 2026-08-29 | rekordcrate PR #269: read-only support with a real rekordbox export as fixture | rekordcrate |
| 2026-09-02 | dj-usb-tkit: hand-written dual-database sticks pass four scenarios on a CDJ-3000X, firmware 1.31 | dj-usb-tkit test matrix |

No follow-up to the "reviewing the specifications" statement had appeared on
alphatheta.com or rekordbox.com by 3 September 2026, and the promised "more
flexible library loading" is in no CDJ-3000X change history through 1.40.

## 2. What is on the drive

A drive rekordbox 7 writes, as measured by two third parties on real exports
(FableGear's ground-truth capture of a 412-track drive; rekordbox-explorer's
examinations). Official documentation never names a single file or folder.

```
/Contents/<Artist>/<Album>/<file>            the audio, as rekordbox lays it out
/PIONEER/rekordbox/export.pdb                Device Library (DeviceSQL)
/PIONEER/rekordbox/exportExt.pdb             its companion: My Tag and menu tables
/PIONEER/rekordbox/exportLibrary.db          OneLibrary (SQLCipher 4)
/PIONEER/rekordbox/exportLibrary.db-wal      SQLite write-ahead log — often most of the data
/PIONEER/rekordbox/exportLibrary.db-shm      SQLite shared-memory index
/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.DAT  analysis, one directory per track
/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.EXT
/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.2EX
/PIONEER/Artwork/...                         album art the database's image table points at
/PIONEER/MYSETTING.DAT, MYSETTING2.DAT, DEVSETTING.DAT, DJMMYSETTING.DAT
```

Three things about it that are easy to get wrong:

- **The write-ahead log is not optional to read.** rekordbox leaves most of a
  fresh export in `exportLibrary.db-wal`: a 118 KB main file beside a 1.1 MB log
  is normal, and opening the main file alone yields an almost empty library
  *with no error*. Every serious reader folds the log in; every writer that
  wants a player to see its work checkpoints it (fourfour runs
  `PRAGMA wal_checkpoint(TRUNCATE)` and deletes both sidecars).
- **The two databases are separate stores with separate ids.** Playlists and
  histories a player creates go into whichever library that player reads; a
  content id in one has no relation to a track id in the other; rekordbox 7.1.4
  and later keep edits in step, earlier versions did not. FableGear's capture
  had 413 tracks in `exportLibrary.db` and 57 in `export.pdb`, which is one
  export's history rather than a rule, but is a warning that "mirror what
  rekordbox writes" does not mean the two files agree.
- **A macOS-copied stick may carry `.PIONEER` instead of `PIONEER`**, and real
  players read it either way (clubtagger falls back to the dotted name).

## 3. The container

`exportLibrary.db` is SQLite 3 encrypted with SQLCipher at the version-4
defaults: 4096-byte pages with 80 reserved bytes each (16-byte IV, 64-byte
HMAC-SHA512), PBKDF2-HMAC-SHA512 with 256,000 iterations, AES-256-CBC, the
16-byte salt in the first 16 bytes of page 1 and no plaintext header. Three
from-specification implementations (rekordbox-explorer in JavaScript,
clubtagger in C, libdjinterop PR #196 in C++) decrypt real rekordbox exports
with exactly those values, and the real fixture in rekordcrate PR #269 opens
with nothing but `PRAGMA key`. Some writers add
`PRAGMA cipher_compatibility = 4`; it is harmless and unnecessary.

The key is a fixed 64-character ASCII **passphrase**, not a raw hex key, and it
is different from `master.db`'s. It is the same for every drive. In the
rekordbox binary it sits obfuscated — a Base85 blob, XORed against a short
fixed string, then zlib-inflated — and it has been recovered two independent
ways: by hooking `sqlite3_key` in rekordbox's bundled SQLite with Frida
(DjManager) and by reading the binary (fourfour, with radare2). It is
published in pyrekordbox, rbox, fourfour, the 0xdevalias gist and DjManager's
issue #300, and it is in `src/rekordbox/mod.rs` here — with `master.db`'s
beside it, which is published in as many places and just as fixed. Both can be
overridden (`ONELIBRARY_KEY`, `REKORDBOX_KEY`, `--onelibrary-key`, `--key`) and
both can be blanked at build time, because a key that has been changed once can
be changed again. §8 records the argument against carrying them.

## 4. The schema

Twenty-two tables, sequential integer ids throughout (where `master.db` uses
UUID strings), no foreign keys, no constraints beyond primary keys, four
indexes. The DDL below is a real rekordbox export's, from the fixture in
rekordcrate PR #269, and agrees with dj-usb-tkit's, FableGear's and
fourfour's writers and pyrekordbox's models . rbox's DDL lacks `djPlayCount`, and
pyrekordbox's `devicelib_plus` model still omits it too (its `Content` maps 45
of the 46 columns, jumping `isrc` → `isHotCueAutoLoadOn`); the real export and
dj-usb-tkit's writer agree on 46.

```sql
CREATE TABLE content(
    content_id integer primary key, title varchar, titleForSearch varchar,
    subtitle varchar, bpmx100 integer, length integer, trackNo integer,
    discNo integer, artist_id_artist integer, artist_id_remixer integer,
    artist_id_originalArtist integer, artist_id_composer integer,
    artist_id_lyricist integer, album_id integer, genre_id integer,
    label_id integer, key_id integer, color_id integer, image_id integer,
    djComment varchar, rating integer, releaseYear integer, releaseDate varchar,
    dateCreated varchar, dateAdded varchar, path varchar, fileName varchar,
    fileSize integer, fileType integer, bitrate integer, bitDepth integer,
    samplingRate integer, isrc varchar, djPlayCount integer,
    isHotCueAutoLoadOn integer, isKuvoDeliverStatusOn integer,
    kuvoDeliveryComment varchar, masterDbId integer, masterContentId integer,
    analysisDataFilePath varchar, analysedBits integer, contentLink integer,
    hasModified integer, cueUpdateCount integer, analysisDataUpdateCount integer,
    informationUpdateCount integer
);
CREATE TABLE genre(genre_id integer primary key, name varchar);
CREATE TABLE artist(artist_id integer primary key, name varchar, nameForSearch varchar);
CREATE TABLE album(album_id integer primary key, name varchar, artist_id integer,
    image_id integer, isComplation integer, nameForSearch varchar);   -- sic
CREATE TABLE label(label_id integer primary key, name varchar);
CREATE TABLE key(key_id integer primary key, name varchar);
CREATE TABLE color(color_id integer primary key, name varchar);
CREATE TABLE image(image_id integer primary key, path varchar);
CREATE TABLE playlist(playlist_id integer primary key, sequenceNo integer, name varchar,
    image_id integer, attribute integer, playlist_id_parent integer);
CREATE TABLE playlist_content(playlist_id integer, content_id integer, sequenceNo integer);
CREATE TABLE history(history_id integer primary key, sequenceNo integer, name varchar,
    attribute integer, history_id_parent integer);
CREATE TABLE history_content(history_id integer, content_id integer, sequenceNo integer);
CREATE TABLE hotCueBankList(hotCueBankList_id integer primary key, sequenceNo integer,
    name varchar, image_id integer, attribute integer, hotCueBankList_id_parent integer);
CREATE TABLE hotCueBankList_cue(hotCueBankList_id integer, cue_id integer, sequenceNo integer);
CREATE TABLE cue(
    cue_id integer primary key, content_id integer, kind integer,
    colorTableIndex integer, cueComment varchar, isActiveLoop integer,
    beatLoopNumerator integer, beatLoopDenominator integer, inUsec integer,
    outUsec integer, in150FramePerSec integer, out150FramePerSec integer,
    inMpegFrameNumber integer, outMpegFrameNumber integer, inMpegAbs integer,
    outMpegAbs integer, inDecodingStartFramePosition integer,
    outDecodingStartFramePosition integer, inFileOffsetInBlock integer,
    OutFileOffsetInBlock integer, inNumberOfSampleInBlock integer,     -- sic
    outNumberOfSampleInBlock integer
);
CREATE TABLE myTag(myTag_id integer primary key, sequenceNo integer, name varchar,
    attribute integer, myTag_id_parent integer);
CREATE TABLE myTag_content(myTag_id integer, content_id integer);
CREATE TABLE menuItem(menuItem_id integer primary key, kind integer, name varchar);
CREATE TABLE category(category_id integer primary key, menuItem_id integer,
    sequenceNo integer, isVisible integer);
CREATE TABLE sort(sort_id integer primary key, menuItem_id integer, sequenceNo integer,
    isVisible integer, isSelectedAsSubColumn integer);
CREATE TABLE property(deviceName varchar, dbVersion varchar, numberOfContents integer,
    createdDate varchar, backGroundColorType integer, myTagMasterDBID integer);
CREATE TABLE recommendedLike(content_id_1 integer, content_id_2 integer, rating integer,
    createdDate integer);
CREATE INDEX index_playlist_content_playlist_id on playlist_content(playlist_id);
CREATE INDEX index_myTag_content_myTag_id on myTag_content(myTag_id);
CREATE INDEX index_myTag_content_content_id on myTag_content(content_id);
CREATE INDEX index_hotCueBankList_cue_hotCueBankList_id on hotCueBankList_cue(hotCueBankList_id);
```

### 4.1 What rekordbox seeds, and a writer must copy

The player's browse screen is driven by static rows, not by firmware. Every
writer that has worked copied them verbatim from a real export.

| Table | Rows | What |
|---|---|---|
| `menuItem` | 27 | Browse categories. `kind` is a code (128 GENRE, 129 ARTIST, 130 ALBUM, 131 TRACK, 133 BPM … 140 DATE ADDED, 144 FOLDER, 151 DJ PLAY COUNT). `name` is the label wrapped in U+FFFA … U+FFFB, e.g. `￺GENRE￻`. |
| `category` | 22–23 | Which menu items show, in what order (about 10 visible). One rekordbox re-write was seen to grow it from 21 to 22 rows. |
| `sort` | 17 | Sort columns per menu item. |
| `color` | 8 | 1 Pink, 2 Red, 3 Orange, 4 Yellow, 5 Green, 6 Aqua, 7 Blue, 8 Purple. |
| `key` | 24 | 1–12 major C…B, 13–24 minor Cm…Bm, written as `D`, `Am`, `F#m`, `Abm`. |
| `myTag` | 28 | rekordbox's default My Tag tree (dj-usb-tkit seeds 28; the exact set is in its `usb_utils.rs`). |
| `property` | 1 | `dbVersion` is the text `'1000'` (DjManager's `'10000'` is a typo against three writers and the fixture). `deviceName` is the volume label, and rekordbox has been seen to blank it on re-export. `myTagMasterDBID` is 0 or a large integer rekordbox assigns (2168740311 seen); dj-usb-tkit seeds 969967066. `createdDate` is `YYYY-MM-DD`; `backGroundColorType` 0. |

### 4.2 Field semantics

- `bpmx100`: centi-BPM (12400 = 124.00). `length`: seconds. `rating`: 0–5,
  not the 0/51/…/255 encoding `export.pdb` uses.
- `path`: drive-relative with forward slashes, `/Contents/<Artist>/<Album>/<file>`.
  `fileName`: the last component. `fileSize` bytes; `bitrate` kbit/s;
  `bitDepth` 16 or 24; `samplingRate` Hz.
- `fileType`: the majority view (fourfour, FableGear, pyrekordbox's enum) is
  MP3 = 1, M4A/AAC = 4, FLAC = 5, WAV = 11, AIFF = 12. rbox's enum disagrees
  (MP3 = 0, ALAC = 4, M4A = 6). Nobody has published the codes read back from a
  real export beside the file they describe, so **check this against a real
  drive before writing it**.
- `analysisDataFilePath`: drive-relative path of the `.DAT`,
  `/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.DAT`; see §5 for the directory names.
- `analysedBits`: rbox's enum has 0, 105, 121 and 233 and fourfour writes 105
  for a fully analysed track; DjManager's document says 41. Conflicting;
  unverified against a real export by anyone who published it.
- `contentLink`: 788224 (0x000C0700) on every track DjManager examined,
  meaning unknown; fourfour writes 0. Single source for the constant.
- `masterDbId`, `masterContentId`: rekordbox's own library id and track id
  (3,000,546,302 and 107,700,687 seen after a rekordbox re-write). Third-party
  writers use 0.
- `hasModified`, `cueUpdateCount`, `analysisDataUpdateCount`,
  `informationUpdateCount`: bookkeeping for rekordbox's own sync; 0 from
  writers.
- Dates are text: `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS.SSS +HH:MM`.
- `playlist.attribute`: 0 playlist, 1 folder, 4 smart; `playlist_id_parent` 0
  at the root; `sequenceNo` is the order. `history`, `hotCueBankList` and
  `myTag` follow the same shape.
- `cue.kind`: 0 memory cue, otherwise the hot-cue number; `outUsec` is −1 when
  not a loop (DjManager).

### 4.3 What rekordbox leaves empty

**The `cue` table is empty on a rekordbox export.** rekordbox-explorer examined
a drive with three hot cues on one track and memory cues, a hot cue and a saved
loop on another, and found zero rows in `cue`. CueMirror found the same on a
*djay Pro* OneLibrary drive, before and after adding memory cues to it — a
second writer rather than a second rekordbox export, and memory cues only.
The cues a player shows come from the ANLZ files, exactly as on a Device
Library drive. Several writers insert `cue` rows anyway; no source shows a
player reading them. `recommendedLike` and `hotCueBankList` are likewise empty
unless the feature was used.

A djay-written drive carries two extra tables, `djay_content` and
`djay_migrations`, and rekordbox and the players tolerate them: the players do
not reject a database for holding tables they do not know.

## 5. The analysis files

Same `PIONEER/USBANLZ` tree, same three files per track, same sections as the
legacy format; OneLibrary adds nothing to the ANLZ files and takes nothing
away. Measured by FableGear over two real drives — 360 of 362 analysis sets on
one, all 354 track directories of a 412-track capture on the other:

| File | Sections, in order |
|---|---|
| `.DAT` | `PPTH`, `PVBR`, `PQTZ`, `PWAV`, `PWV2`, `PCOB` ×2 |
| `.EXT` | `PPTH`, `PWV3`, `PCOB` ×2, `PCO2` ×2, `PQT2`, `PWV5`, `PWV4`, `PSSI` |
| `.2EX` | `PPTH`, `PWV6`, `PWV7`, `PWVC` |

Two corrections to folklore. The phrase data (`PSSI`) is in the `.EXT`, not
the `.2EX`; the `.2EX` holds only the three-band waveforms and a 20-byte
`PWVC` summary.

**The order and the exact section list matter, and a player does not say when
they are wrong.** With the analysis files finally being found (§5.1), a
CDJ-3000X drew the monochrome preview from the `.DAT` and no colour at all.
The cause was in the `.EXT`: this project wrote a second `PQTZ` where rekordbox
writes `PQT2`, and everything after it — `PWV5`, `PWV4`, `PSSI`, which is to
say the colour waveforms and the phrases — went unread, while the cues that
sit *before* it came through. So a player reads these files in order and
stops making sense of one at the first section it did not expect there.

`PQT2` is an extended beat grid at two bytes a beat and that is the whole of
what is published about it, so the honest thing is to write nothing in its
place: a missing section is skipped, and a wrong one costs everything behind
it. And the `.2EX` is a CDJ-3000 file that predates OneLibrary,
though fourfour's hardware notes say the OneLibrary-only players require it
where a CDJ-3000 merely draws a worse waveform without it.

Two hardware facts from fourfour's CDJ-3000 work that any writer should
respect: the `PPTH` path must be null-terminated (two zero bytes, counted in
the length) or the player rejects the file and writes its own `ANLZ0001.DAT`
beside it; and a CDJ-3000 without an `.EXT` re-analyses on load.

### 5.1 The directory names — settled

**A player computes this name itself and looks nowhere else.** That was an
open question between the sources below until a CDJ-3000X (firmware 1.40) was
handed a drive this project wrote with the analysis directories named after
the track id: it browsed the library perfectly — playlists, titles, artists,
key search, BPM in the track list — and on loading a track showed **no
waveform, no beat grid, no phrases, no cues, and no BPM on the deck**, with no
pause to analyse. Everything the database holds, nothing the analysis files
hold, and no attempt to make up the difference. The files were on the drive and
`analysisDataFilePath` pointed straight at them.

So Deep Symmetry's reading — that a player follows the stored path — does not
describe the CDJ-3000X, and fourfour's does. It also explains why
`analysedBits` matters: told the track is analysed, the player does not
re-measure, and simply shows nothing when the analysis is not where it expects.

rekordbox
names each track's analysis directory `P{3 hex digits}/{8 hex digits}` — real
drives show `P00E/000281CE`, `P016/0000875E`, `P04A/000189B4`. fourfour
disassembled `CreateAnlzFileFolderPath()` from the rekordbox binary and reports
it as a hash of the drive-relative audio path:

```python
h = 0
for c in path:                       # UTF-16 code units of e.g. "/Contents/A/B/track.flac"
    h = (((h * 0x5BC9 + c) & 0xFFFFFFFF) * 0x93B5 + c) & 0xFFFFFFFF
r = h % 200003                       # the eight hex digits
p = ((r >> 0) & 1) | ((r >> 1) & 2) | ((r >> 4) & 4) | ((r >> 4) & 8) \
  | ((r >> 5) & 16) | ((r >> 8) & 32) | ((r >> 10) & 64)   # the three, so P000–P07F
```

The three-hex `P` reproduces for all three of fourfour's published test
vectors, and the eight-hex hash for two of them; the third gives
`P028/0000F098` where `P012/0000530C` is claimed. So treat the algorithm as
probable and check it against a real export before relying on it. Beyond the naming, the sources disagree on whether it matters:

- fourfour says the CDJ-3000 **recomputes** the directory from the audio path
  and ignores the path stored in the database. Confirmed on a CDJ-3000X above,
  except for the last part: it did not analyse afresh, it showed nothing.
- Deep Symmetry's analysis says players use the path string in the database.
  Not on this player.
- FableGear (`P{id // 2048:03}/{id:08X}`) derives the name from the track id,
  as this project did until the test above. Neither FableGear's CDJ-3000 run
  nor dj-usb-tkit's CDJ-3000X run recorded whether waveforms appeared, so
  neither result says anything about their naming either way — and the first
  observation of what actually happens suggests both are writing analysis
  files no player opens.

A consequence worth writing down: the eight hex digits are a hash modulo
200003, so a library of a few thousand tracks will put two tracks in one
directory. That is what the numbered files are for (`ANLZ0000`, `ANLZ0001`),
and it means the path inside each file has to name its own track exactly —
which is the likeliest reason a player validates `PPTH` at all, and why
fourfour found that a mismatched one gets the file rejected.

The safe course for a writer is to name the directories the way rekordbox does,
which satisfies both theories.

## 6. Which player reads what

From rekordbox's USB export compatibility table (read on 3 September 2026,
where each player has exactly one tick) and AlphaTheta's notice of 18 March
2026, which gives the same one-format-per-player matrix.

| Reads only OneLibrary | Reads only Device Library |
|---|---|
| CDJ-3000X, CDJ-1500X, OPUS-QUAD, OMNIS-DUO, XDJ-AZ, XDJ-AN | CDJ-3000, CDJ-TOUR1, CDJ-2000NXS2, CDJ-2000NXS, CDJ-900NXS, CDJ-850, CDJ-350, XDJ-1000MK2, XDJ-1000, XDJ-700, XDJ-XZ, XDJ-RX3, XDJ-RX2, XDJ-RX, XDJ-RR, XDJ-R1 |

The CDJ-3000X manual (page 9): "This unit only supports OneLibrary. When using
any library other than OneLibrary on the USB device, make sure to first convert
it to OneLibrary." Supported file systems: FAT16, FAT32, exFAT, HFS+; NTFS is
not. The OMNIS-DUO manual says the same of Device Library Plus.

Official sources are not consistent with each other: AlphaTheta's help-centre
article on OneLibrary hardware still lists the CDJ-3000 as compatible "as of
October 2025", the October 2025 launch listed "CDJ-3000 (requires firmware
version 3.30 or later)", and both predate the rollback. The table above is the
position after it.

### 6.1 Firmware, as it stands

| Player | Current | Library-related history |
|---|---|---|
| CDJ-3000X | 1.40 (2026-07-14) | 1.10 (2025-09-09) Serato HID; 1.20 (2025-10-21) djay HID; 1.31 (2026-01-08) Apple Music, a "Library not responding" fix; 1.40 dark mode, internet updates. No entry mentions a library format. |
| CDJ-1500X | 1.10 (2026-07-02) | Serato and djay compatibility. |
| CDJ-3000 | 3.22 (2026-01-15) | 3.30 (2025-10-21) "Support for OneLibrary … If Device Library and OneLibrary are both present on a USB storage device, OneLibrary will load by default" — withdrawn; 3.22 "does not include OneLibrary support". |
| OPUS-QUAD | 1.32 (2026-03-24) | Nothing about library formats since 1.10. |
| XDJ-AZ | 1.30 (2026-04-09) | Nothing about library formats. |
| OMNIS-DUO | 1.23 (2025-12-04) | Nothing about library formats. |

### 6.2 The 3.30 incident, in one paragraph

Firmware 3.30 made the CDJ-3000 read OneLibrary first and ignore the legacy
database when both were present, even when the OneLibrary one was stale or
empty — which it was on any drive last exported by a rekordbox older than
6.8.1, or synced piecemeal since. DJs got a drive with music on it and no
playlists. AlphaTheta suspended the firmware, told venues to roll back to 3.20,
and in January 2026 said the CDJ-3000 "has reverted to using the Device
Library format". The lesson for a writer is that **a drive carrying both
databases must carry the same library in both**, because which one a player
reads can change with a firmware update.

## 7. Who writes it, and what has been seen to play

| Writer | What it writes | Hardware result |
|---|---|---|
| rekordbox 6.8.1+ / 7 | Both databases, all three ANLZ files, artwork | The baseline everything else is measured against |
| djay Pro 5.5+ (partner; Windows/macOS write, iOS read-only) | `exportLibrary.db` with its two vendor tables, ANLZ including `.2EX`, artwork, `/Contents`; no `export.pdb` (wokhouse/onelib-converter exists to add one) | Reads on the OneLibrary players by design; rekordbox reads djay drives |
| Traktor Pro 4 / Play (partner) | Not shipped as of 4.5.1 (2026-07-09); still "coming months" on AlphaTheta's page. NI was acquired by inMusic in May 2026 | — |
| Lexicon (commercial) | Claims both; its author had no key in December 2023 | None published |
| VirtualDJ (not a partner) | Claims DLP from build 8528 (March 2025) | None published |
| **dj-usb-tkit** (Rust/Tauri, MIT) | Both databases from scratch (`usb_utils.rs`: DDL, 27 menu items, 22 categories, 17 sorts, 8 colours, 28 My Tags; `PRAGMA key` only; no `cue` rows, no `exportExt.pdb`, no `.2EX`) | **CDJ-3000X, firmware 1.31, 2026-09-02**: four scenarios pass — normal export, strict-parity repair, non-ASCII strings, and a fresh-initialised stick with more than 16 tracks ("accepted and playable"). CDJ-3000 3.20 also. Self-reported in the project's own test matrix; waveform and grid provenance not recorded. |
| **FableGear** (Python, sqlcipher3) | `exportLibrary.db` only, from DDL and rows copied from a real export; `.DAT`/`.EXT` writers, no `.2EX`; `cue` rows written | **CDJ-3000, 2026-07-29**: a OneLibrary-only stick "loaded and played … without issues". Firmware not recorded. Not tested on a CDJ-3000X. |
| **fourfour** pioneer-usb-writer (Rust) | Both databases, `.DAT`/`.EXT`, artwork; WAL then checkpoint; `analysedBits` 105, `contentLink` 0 | CDJ-3000, firmware 3.19: a dual-format drive works, and fourfour says that player reads OneLibrary but still needs the legacy `.EXT`. CDJ-3000X "from reference specs" only. |
| **this project** (`src/export/onelibrary.rs`) | Both databases from one collection, all three ANLZ files, no `exportExt.pdb`, no artwork; `cue` left empty | **CDJ-3000X, firmware 1.40, September 2026**: playlists, track list and key search read; no waveform, grid, phrases, cues or deck BPM, because the analysis directories were named after the track id (§5.1 — since fixed, and untested since). The same drive on a **CDJ-3000, firmware 2.05**: the library was not seen at all, which is the legacy `export.pdb` being refused rather than anything to do with OneLibrary |
| rbox (Rust, GPL-3.0) | Create and insert; no cue insert | None |
| pyrekordbox `devicelib_plus` (git only; 0.4.4 on PyPI predates it) | Read and write | None |
| rekordbox-explorer (JavaScript) | A playlist writer, deliberately unshipped: "we still have no CDJ to test against" | None |
| DjManager | A design (issue #300); no writer merged | None |
| Readers only | rekordcrate PR #269 (real fixture), libdjinterop PR #196, onelibrary-connect, clubtagger (over Pro DJ Link NFS from a CDJ-3000X), CueMirror, beat-link's OpusProvider (which needs the key supplied by the user and reads only `analysisDataFilePath` and `image`) | — |

Two things are notable by their absence. Nobody has documented a
**OneLibrary-only** drive on a **OneLibrary-only** player: every CDJ-3000X
result, this project's included, used a drive carrying both databases, and the
OneLibrary-only result was on a CDJ-3000, which officially does not read
OneLibrary at all — so either that unit was on 3.30, or the CDJ-3000 reads
`exportLibrary.db` when `export.pdb` is absent regardless of what AlphaTheta
says (fourfour and rekordbox-explorer both claim it does, on 3.15+ or 3.19+,
and no primary source settles it). Strictly, a 3000X browsing a dual-database
drive does not by itself prove it read the OneLibrary one — except that the
3000X is documented not to read the other, and a drive whose legacy database it
ignored would show nothing at all.

And **nobody has recorded whether a hand-written drive's grids and waveforms
were used rather than re-analysed**, which is the difference between a player
that *mounts* the drive and one that *plays* it as prepared. It is the cheapest
outstanding experiment on this page: load a track, and watch whether the
waveform is there at once or the player thinks about it first.

## 8. Is there a specification?

No. As of 3 September 2026 there is no OneLibrary specification, SDK, licence
text, partner application or developer programme on alphatheta.com (English or
Japanese), rekordbox.com or the help centre. The launch announcement defers
"USB Export specifications" to Algoriddim's and NI's own sites, which contain
none. "rekordbox for Developers" is a page about importing XML playlists into
the Bridge pane, linking a 2020 XML PDF and a contact form; it does not mention
OneLibrary and the OneLibrary FAQ does not point to it. Official documents
never name `exportLibrary.db`, a `PIONEER` folder, or any file on the drive.
AlphaTheta's own FAQ adds that "full compatibility between different software
applications isn't guaranteed".

What exists instead is the reverse engineering above, and it is unusually
complete for a format three years old. It is also contested. The August 2026
Mixxx proposal for a rekordbox USB exporter makes OneLibrary "out of scope —
for legal, not just effort, reasons": the file "is SQLCipher-encrypted (256-bit
AES); writing it means circumventing an access-control measure with an
extracted key, which is precisely the DMCA §1201 exposure the project is
structured to avoid", and OneLibrary "should be pursued only via an official
AlphaTheta spec/partnership, never with a recovered key". Deep Symmetry reads
the file only with a key the user supplies. rekordcrate's pull request
hard-codes the key. Those are three positions on one question, and this
project's current one — read with a key the user brings, write nothing — sits
between them.

## 9. What this project writes today, and the gap

`musicai export` and the app's sync write a Device Library drive —
`export.pdb`, the three ANLZ files per track, audio under
`/Contents/<Artist>/` — and a OneLibrary database beside it, built from the
same track list and playlist tree so the two cannot disagree about what is on
the drive. `src/export/onelibrary.rs` is the writer; it follows §4 exactly,
leaves `cue` empty as rekordbox does, and the export reads the file back off
the drive, keys it and counts it before calling itself done. A build with the
key blanked, and nothing in `ONELIBRARY_KEY`, writes the legacy drive alone and
says so.

Against §2 to §5 what is left is:

- **The waveform colours are a judgement call, not a measurement.** A player
  draws them from three bits each of red, green and blue; what rekordbox puts
  in those bits for a given piece of audio is not published, and nothing here
  has been compared against a real export column for column. The first attempt
  drew a real track almost entirely pale grey on a CDJ-3000X, which is at least
  a floor to improve on: shares measured against the loudest band rather than
  their own sum, averages rather than peaks so a kick does not whiten
  everything, and a crossover placed to keep a whole tune's worth of music in
  one band.
- **The analysis half has failed twice and been fixed twice.** A CDJ-3000X on
  firmware 1.40 first showed nothing at all off the analysis files, because
  they were in directories named after the track id rather than the hash of the
  audio path (§5.1). With that fixed it drew waveforms — but monochrome ones,
  from the `.DAT`, because a wrong section in the `.EXT` hid the colour
  waveforms behind it (§5). That is fixed too, and untested since. Phrases and
  the three-band waveform are the next things to look for, in that order.
- **A CDJ-3000 on firmware 2.05 does not see the library at all**, which is the
  legacy `export.pdb` being refused rather than anything about OneLibrary — the
  3000 never reads that database. This is the first time the legacy writer has
  been tried on hardware, and it failed. `export.pdb` parses under two
  independent parsers, so what is wrong is something a parser tolerates and a
  player does not; fourfour's notes list several candidates found the same way,
  and the page headers have since been rewritten to match all of them that are
  published. The remaining named suspect is that the history tables must not be
  empty, which every export this writes leaves empty and which cannot be
  satisfied without knowing what a valid history row looks like.
- No `exportExt.pdb`. rekordbox writes it; whether any player needs it is not
  established (dj-usb-tkit and FableGear omit it and passed).
- The `.2EX` now carries `PWV6`, `PWV7` and a `PWVC` whose three values are a
  guess at what a real one holds — a per-band average, which is the shape of
  the numbers a capture reported.
- The `.EXT` carries no extended beat grid at all, where rekordbox writes
  `PQT2`; see §5.
- No artwork, so `image` is empty and every `image_id` is 0.
- No My Tags beyond rekordbox's four empty groups, no history, no hot-cue
  banks: the collection has none of them to write.
- `musicai rekordbox schema <drive>` opens an `exportLibrary.db` with a
  supplied key and prints its tables, columns and row counts, which is how the
  DDL above can be checked against any drive to hand.

## 10. What is left, in order

0. **Compare what §9 writes against a real export**, table by table, with
   `musicai rekordbox schema` on both. The category and sort rows are the
   least attested part of the file, and a real export settles them.
1. **Read a real export first.** The fixture in rekordcrate PR #269 (417
   tracks) or any drive a rekordbox 7.2.13+ wrote, opened with
   `musicai rekordbox schema`, checks §4 and settles `fileType`,
   `analysedBits` and `contentLink` by looking at rows beside the files they
   describe.
2. **Write the database from the same collection the pdb is written from**:
   the DDL verbatim; seed rows copied from a real export (menuItem, category,
   sort, color, key, myTag, property); one `content` row per track with the
   same `/Contents` path and analysis path the pdb carries; artist, album,
   genre, key and image lookups; playlists and folders with `sequenceNo`.
   Leave `cue` empty, as rekordbox does. Checkpoint the WAL and delete both
   sidecars before the drive is unmounted.
3. **Keep the two databases telling one story.** Same tracks, same playlists,
   same paths, every sync — the 3.30 lesson. dj-usb-tkit derives its pdb
   playlist order from the OneLibrary side so they cannot drift; any scheme
   that rebuilds both from the collection on every write gets this for free.
4. **Name analysis directories the way rekordbox does**, after verifying the
   hash in §5.1 against a real export, and add `PWVC` to the `.2EX`.
5. **Test on hardware with a control.** A rekordbox-written stick first, on the
   same player, then the hand-written one; record the firmware; and record not
   just that the drive mounts but that the waveform, grid, hot cues, playlists
   and My Tags appear without the player re-analysing, and that a cue set on
   the player is written back and read by rekordbox. The CDJ-3000X (1.31+) and
   the CDJ-3000 (3.22) are the two players that matter, and an OPUS-QUAD or
   XDJ-AZ would answer whether the all-in-ones behave like the 3000X.
6. **Decide the key and licence question before shipping**, not after: the
   reader's policy (user-supplied key, never bundled) can extend to the writer
   unchanged, and the sync sheet should say what a drive was written for.

## 11. Claims checked and found wrong

Including this repository's own.

| Claim | Where | What is actually so |
|---|---|---|
| "rekordbox 6.8.2 and later write both databases" | this repo's spec | 6.8.1 (2023-12-19); 6.8.2's notes say nothing about libraries. |
| OneLibrary's "schema has never been published" / "is not published" | spec, READMEs, module docs | Not published by AlphaTheta; fully documented by third parties, with a real export as a public fixture. |
| "nobody has demonstrated an `exportLibrary.db` written from scratch that a CDJ-3000X will read" | spec, READMEs, module docs | dj-usb-tkit, CDJ-3000X 1.31, 2026-09-02 — with `export.pdb` alongside, and without recording analysis provenance. |
| Standard SQLCipher parameters "do not decrypt the database"; rekordbox's own `sqlite3.dll` must be loaded | DjManager issue #300 and protocol doc | Three from-spec implementations and the rekordcrate fixture open it with SQLCipher 4 defaults. |
| "CDJs do not use `exportLibrary.db` for audio playback — that relies on `export.pdb` and ANLZ" | DjManager protocol doc | The OneLibrary-only players have no `export.pdb` to rely on; FableGear's OneLibrary-only stick played. |
| `property.dbVersion` is `'10000'` | DjManager protocol doc | `'1000'` in three writers and the fixture. |
| `exportLibrary.db` "uses the same key and same schema" as `master.db` | MichaelChidiac/rekordbox-tools | Different key, different schema; that tool's primary mode writes `djmd*` tables and has no hardware result. |
| "Works on CDJ-3000" as evidence the encrypted file is right | fourfour's writer | The CDJ-3000 on current firmware reads `export.pdb`; fourfour writes both, so the pdb may be carrying the result. |
| The CDJ-3000 supports OneLibrary "as of October 2025" | AlphaTheta help centre | Superseded by the rollback and by AlphaTheta's own March 2026 table. |
| Phrase data lives in the `.2EX` | assorted | `PSSI` is in the `.EXT`; the `.2EX` is waveforms only. |
| "AlphaTheta has acquired Serato" | Lexicon blog | Clearance was declined by the NZ Commerce Commission in July 2024. |

## 12. Open questions

Answered since this was written, and left here so the answers are findable:
a CDJ-3000X **recomputes** the analysis directory and ignores
`analysisDataFilePath` (§5.1), and it does **not** silently re-analyse a track
whose analysis it cannot find — it shows nothing at all.

Still open:

- Does any player need `exportExt.pdb`, and what does a OneLibrary-only player
  do about My Tags without it — it has a `myTag` table of its own, which
  suggests not.
- Which players require the `.2EX`, and whether `PWVC` matters. This project
  writes `PWV7` then `PWV6` and no `PWVC`, where rekordbox writes all three in
  the other order; whether that is enough is the next thing a drive test would
  say.
- The real `fileType`, `analysedBits` and `contentLink` values, read from rows
  beside their files.
- Whether the CDJ-3000 on 3.22 reads `exportLibrary.db` when `export.pdb` is
  absent (FableGear's result says something did).
- **Why a CDJ-3000 on firmware 2.05 refuses a legacy `export.pdb` this project
  writes** — see §9. The candidates are all in fourfour's notes and none of
  them has been isolated.
- What `myTagMasterDBID` is for, and whether a player cares.
- What AlphaTheta's "more flexible library loading" will be, and when.

## 13. Dead ends

- GitHub issue and pull-request *comment threads* could not be read from this
  session (API rate-limited or blocked; page HTML renders without comments).
  Bodies and diffs were read via `patch-diff.githubusercontent.com` and raw
  file URLs. Unread: rekordcrate #172 and #142, pyrekordbox #125 and #140,
  mixxx #15556, libdjinterop #177, #191 and #194.
- `support.alphatheta.com` articles return 403 to this session; their content
  is taken from the search snippets and from the sweep that could read them.
- Deep Symmetry's Zulip has no indexed thread on `exportLibrary.db`; the DJ
  Link Ecosystem Analysis documents only `export.pdb` and `exportExt.pdb`.
- No forum or Reddit post describes a hand-made `exportLibrary.db` on any
  player; every hardware result above is from a project's own repository.
- The official OneLibrary export guide PDF is image-heavy and its text extract
  is partial; nothing in it names a file on the drive.

## 14. Sources

Official:
[rekordbox 6.6.11 notes](https://rekordbox.com/en/2023/03/rekordbox-v6611-release-information/) ·
[6.8.1 notes](https://rekordbox.com/en/2023/12/rekordbox-v681-release-information/) ·
[7.0.9 notes](https://rekordbox.com/en/2025/02/rekordbox-v709-release-information/) ·
[7.2.5 notes](https://rekordbox.com/en/2025/10/rekordbox-v725-release-information/) ·
[7.2.12 notice](https://rekordbox.com/en/2026/03/important-notice-regarding-rekordbox-v7212/) ·
[7.2.16 notes](https://rekordbox.com/en/2026/07/rekordbox-v7216-release-information/) ·
[release notes index](https://rekordbox.com/en/support/releasenote/) ·
[USB export compatibility table](https://rekordbox.com/en/support/usb-export/) ·
[OneLibrary FAQ](https://rekordbox.com/en/support/faq/onelibrary-7/) ·
[Device Library Plus FAQ](https://rekordbox.com/en/support/faq/devicelibraryplus-6/) ·
[Device Library Plus User's Guide (PDF)](https://cdn.rekordbox.com/files/20231208144230/rekordbox6.8.1_Device_Library_Plus_guide_EN.pdf) ·
[OneLibrary Compatible USB Device Export Guide (PDF)](https://cdn.rekordbox.com/files/20260318114024/OneLibrary-Compatible-USB-Device-Export_en.pdf) ·
[rekordbox for Developers](https://rekordbox.com/en/support/developer/) ·
[OneLibrary launch](https://rekordbox.com/en/2025/10/dj-brands-unite-to-launch-onelibrary/) ·
[alphatheta.com/onelibrary](https://alphatheta.com/en/onelibrary/) ·
[AlphaTheta USB notice, 18 March 2026](https://alphatheta.com/en/information/important-notice-for-customers-using-usb-devices-with-our-dj-equipment/) ·
[CDJ-3000 3.30 notice, 7 January 2026](https://www.pioneerdj.com/en/news/2026/cdj-3000-firmware-ver330-important-notice/) ·
[CDJ-3000 change history 3.22 (PDF)](https://downloads.support.alphatheta.com/firmwares/dj-players/CDJ-3000/CDJ-3000-Firmware-Change-History-Ver322-en.pdf) ·
[CDJ-3000X change history 1.31 (PDF)](https://downloads.support.alphatheta.com/firmwares/dj-players/CDJ-3000X/CDJ-3000X-Firmware-Change-History-Ver131-en.pdf) and 1.40 ·
[CDJ-3000X manual (PDF)](https://downloads.support.alphatheta.com/manuals/dj-players/CDJ-3000X/CDJ-3000X_DRI1956B_manual.pdf) ·
[OMNIS-DUO manual (PDF)](https://downloads.support.alphatheta.com/manuals/OMNIS_DUO_DRI1882A_manual.pdf) ·
[AlphaTheta help centre: OneLibrary hardware](https://support.alphatheta.com/en-US/articles/51298635657881) ·
[AlphaTheta and Serato statement](https://alphatheta.com/en/information/official-statement-from-alphatheta-and-serato-regarding-nzccs-ruling-on-acquisition/) ·
[djay: what is OneLibrary](https://help.algoriddim.com/topic/using-djay/what-is-onelibrary) ·
[inMusic acquires Native Instruments](https://www.inmusicbrands.com/press/inmusic-native/).

Reverse engineering and implementations:
[rekordcrate PR #269](https://github.com/Holzhaus/rekordcrate/pull/269) ·
[pyrekordbox Device Library Plus docs](https://pyrekordbox.readthedocs.io/en/latest/formats/devicelib_plus.html), [issue #125](https://github.com/dylanljones/pyrekordbox/issues/125), [PR #210](https://github.com/dylanljones/pyrekordbox/pull/210) ·
[dj-usb-tkit test matrix](https://github.com/haivala/dj-usb-tkit/blob/14c103abbda29c37c93340b5bd52b91ca9354d7b/docs/CDJ_TEST_MATRIX.md), [`usb_utils.rs`](https://github.com/haivala/dj-usb-tkit/blob/14c103abbda29c37c93340b5bd52b91ca9354d7b/backend/src/service/usb_utils.rs), [eDB notes](https://github.com/haivala/dj-usb-tkit/blob/14c103abbda29c37c93340b5bd52b91ca9354d7b/docs/eDB.md) ·
[FableGear hardware test](https://github.com/fabledharbinger0993/FableGear/blob/18407507780704e7c082baadc659ae0162600c97/docs/HARDWARE_TEST_ONELIBRARY.md), [PR #131](https://github.com/fabledharbinger0993/FableGear/pull/131), [`onelibrary_writer.py`](https://github.com/fabledharbinger0993/FableGear/blob/18407507780704e7c082baadc659ae0162600c97/fablegear_database/onelibrary_writer.py) ·
[fourfour PIONEER.md](https://github.com/morizkraemer/fourfour/blob/64091d39642bb071709201a9a74746629098fb0f/pioneer-usb-writer/reference-code/PIONEER.md) ·
[rekordbox-explorer database notes](https://github.com/CarlosFranzetti/rekordbox-explorer/blob/df7bb459450b94d8edf8e69eb913b21aa055c7a2/database.md) and `sqlcipher.js` ·
[libdjinterop PR #196](https://github.com/xsco/libdjinterop/pull/196) ·
[DjManager issue #300](https://github.com/Radexito/DjManager/issues/300) and [protocol doc](https://github.com/Radexito/DjManager/blob/44896ad2ca68672bc6b8002451620621feb71325/protocol_rekordbox.md) ·
[rbox](https://docs.rs/crate/rbox/latest) ·
[onelibrary-connect](https://github.com/chrisle/onelibrary-connect) ·
[clubtagger `onelibrary.c`](https://github.com/mischa85/clubtagger/blob/94a114c58fc0b074e4b534895e5fcec8ea60fcc4/prolink/onelibrary.c) ·
[CueMirror research notes](https://github.com/8rian6/CueMirror/blob/70766fb2a21b3564af46e5edf6edd8e3dd4e9cce/CueMirror/Research/MemoryCueBeforeAfterDiff.md) ·
[beat-link OpusProvider](https://github.com/Deep-Symmetry/beat-link/blob/main/src/main/java/org/deepsymmetry/beatlink/data/OpusProvider.java) ·
[wokhouse/onelib-converter](https://github.com/wokhouse/onelib-converter) ·
[0xdevalias's notes](https://gist.github.com/0xdevalias/b803476793b56f7c45e6361799168eb0) ·
[Mixxx issue #15556](https://github.com/mixxxdj/mixxx/issues/15556) and [proposals PR #20](https://github.com/mixxxdj/proposals/pull/20) ·
[Deep Symmetry: analysis files](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html).

Reporting:
[AlphaTheta suspends 3.30 (DJ Mag)](https://djmag.com/tech/alphatheta-suspends-cdj-3000-firmware-update-distribution-following-playlist-issues) ·
[AlphaTheta pulls CDJ-3000 firmware (Digital DJ Tips)](https://www.digitaldjtips.com/alphatheta-pulls-cdj-3000-firmware-after-playlist-issues/) ·
[Device Library Plus explained (Lexicon)](https://www.lexicondj.com/blog/everything-you-need-to-know-about-device-library-plus-and-more) ·
[VirtualDJ forum thread](https://virtualdj.com/forums/265825/General_Discussion/One_Library_universal_database.html) ·
[Traktor status thread](https://community.native-instruments.com/discussion/43/official-update-status-traktor-pro-4-current-version-4-5-1).
