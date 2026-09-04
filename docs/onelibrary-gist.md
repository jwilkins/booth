# OneLibrary (`exportLibrary.db`): what's known, and what a CDJ-3000X did with it

Notes on the database a CDJ-3000X reads off a USB stick, written up because
the format is now documented well enough to write one — and because several
things that were open questions in the public research turned out to have
answers once a hand-written drive was put in front of real players.

The useful findings here are the **negative** ones. Two separate mistakes each
produced a drive that looked like it was working, and neither the player nor
any parser said a word about either.

Everything below is either cited to a primary source or is a hardware
observation made on **CDJ-3000X firmware 1.40** and **CDJ-3000 firmware 2.05**
in September 2026. Where a claim is inference, it says so.

---

## The short version

- The **CDJ-3000X, CDJ-1500X, XDJ-AZ, XDJ-AN, OPUS-QUAD and OMNIS-DUO** read
  `PIONEER/rekordbox/exportLibrary.db` and **do not fall back** to the legacy
  `export.pdb`. Every other player, the CDJ-3000 included, reads only
  `export.pdb`. No player reads both.
- A from-scratch `exportLibrary.db` **works**: a CDJ-3000X browsed playlists,
  the track list and key search off a stick written entirely by third-party
  code.
- **The player computes the analysis directory name itself** — from a hash of
  the audio file's path — and ignores `content.analysisDataFilePath`. Get the
  name wrong and you get a library that browses perfectly and has no waveform,
  no beat grid, no cues, no phrases and no BPM on the deck. It does *not*
  re-analyse to cover the gap. This is the single most expensive thing to get
  wrong, because it looks like success.
- **Sections in the analysis files are read in order, and one the player did
  not expect there costs you everything behind it.** A stray `PQTZ` in the
  `.EXT` where rekordbox writes `PQT2` hid the colour waveforms and the phrase
  data, while leaving the cues in front of it working — so the player drew a
  monochrome waveform out of the older `.DAT` section and looked, at a glance,
  fine.
- The schema is fully known (22 tables), the container is stock SQLCipher 4,
  and the key is a fixed passphrase identical on every drive.
- AlphaTheta publishes **no specification, SDK, or developer programme** for
  any of this.

---

## 1. Names and dates

| When | What |
|---|---|
| 2023-03-07 | rekordbox 6.6.11 adds "Device Library Plus", opt-in, OPUS-QUAD only |
| 2023-12-19 | 6.8.1: "The rekordbox library is now always exports in both Device Library and Device Library Plus formats" — note **6.8.1**, not 6.8.2 |
| 2025-08-22 | pyrekordbox gains a `devicelib_plus` module (read + write) |
| 2025-10-21 | Renamed **OneLibrary** in rekordbox 7.2.5; opened to Algoriddim and Native Instruments; CDJ-3000 firmware 3.30 adds it |
| 2025-11 | 3.30 withdrawn — it preferred OneLibrary even when that database was stale, and DJs got sticks with music and no playlists |
| 2026-01-15 | CDJ-3000 firmware 3.22: "This update does not include OneLibrary support" |
| 2026-03-18 | AlphaTheta notice: one format per player, rekordbox 7.2.11 the minimum; CDJ-3000X firmware "will support more flexible library loading" (still not shipped as of 1.40, July 2026) |

"Device Library Plus" and "OneLibrary" are the same format; AlphaTheta says so
outright.

## 2. On the drive

Official documentation never names a single file. This is from third-party
captures of real rekordbox 7 exports:

```
/Contents/<Artist>/<Album>/<file>            the audio
/PIONEER/rekordbox/export.pdb                Device Library (DeviceSQL)
/PIONEER/rekordbox/exportExt.pdb             its companion (My Tags, menus)
/PIONEER/rekordbox/exportLibrary.db          OneLibrary (SQLCipher)
/PIONEER/rekordbox/exportLibrary.db-wal      often holds most of the data
/PIONEER/rekordbox/exportLibrary.db-shm
/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.DAT  analysis, one dir per track
/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.EXT
/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.2EX
/PIONEER/Artwork/...
/PIONEER/MYSETTING.DAT, MYSETTING2.DAT, DEVSETTING.DAT, DJMMYSETTING.DAT
```

Three traps:

1. **The write-ahead log is not optional.** rekordbox leaves most of a fresh
   export in `exportLibrary.db-wal` — a 118 KB main file beside a 1.1 MB log is
   normal — and opening the main file alone yields an almost-empty library
   *with no error*. Readers must fold the log in; writers should checkpoint
   (`PRAGMA wal_checkpoint(TRUNCATE)`) and remove the sidecars.
2. **The two databases are independent stores.** Separate ids, separate
   playlists, separate histories. One capture had 413 tracks in
   `exportLibrary.db` and 57 in `export.pdb`.
3. A stick copied on macOS may carry `.PIONEER` rather than `PIONEER`, and
   players still read it.

## 3. The container

Stock **SQLCipher 4 defaults**: 4096-byte pages, 80 reserved bytes each
(16-byte IV + 64-byte HMAC-SHA512), PBKDF2-HMAC-SHA512 with 256,000 iterations,
AES-256-CBC, 16-byte salt in the first page, no plaintext header. `PRAGMA key`
alone opens a real rekordbox export — verified by at least four independent
implementations, one of which reimplements the crypto from the SQLCipher spec
with no SQLCipher dependency at all.

> DjManager's protocol notes claim standard SQLCipher parameters do *not* work
> and that rekordbox's own `sqlite3.dll` must be loaded. **That is wrong** —
> most likely a WAL problem misdiagnosed. Every other reader uses defaults.

The key is a fixed **64-character ASCII passphrase** (not a hex key, and a
different one from `master.db`'s). It is the same on every drive on earth,
sits obfuscated in the rekordbox binary as a Base85 blob XORed against a short
fixed string and zlib-compressed, and has been recovered independently by
hooking `sqlite3_key` with Frida and by disassembling the binary. It is
published in pyrekordbox, rbox, fourfour, dj-usb-tkit, DjManager #300,
rekordcrate PR #269 and [0xdevalias's
notes](https://gist.github.com/0xdevalias/b803476793b56f7c45e6361799168eb0) —
I haven't reproduced the literal string here, only because it adds nothing
those sources don't already give you.

## 4. The schema

22 tables, sequential integer ids (where `master.db` uses UUID strings), no
foreign keys, four indexes. This DDL is from a **real rekordbox export**
(the fixture in [rekordcrate PR
#269](https://github.com/Holzhaus/rekordcrate/pull/269)), and matches
dj-usb-tkit's and fourfour's writers. Note the two misspellings —
`isComplation` and `OutFileOffsetInBlock` — which you must reproduce.

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
    image_id integer, isComplation integer, nameForSearch varchar);
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
    OutFileOffsetInBlock integer, inNumberOfSampleInBlock integer,
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

### Seed rows a writer must supply

The browse screen is built from static rows, not from firmware.

| Table | Rows | Notes |
|---|---|---|
| `menuItem` | 27 | `kind` is a code; `name` is the label wrapped in `U+FFFA … U+FFFB`, e.g. `￺GENRE￻` |
| `category` | 22 | which menu items show, and their order (about 10 visible) |
| `sort` | 17 | sort columns |
| `color` | 8 | Pink, Red, Orange, Yellow, Green, Aqua, Blue, Purple |
| `key` | 24 | 1–12 major C…B, 13–24 minor Cm…Bm ("D", "Am", "F#m", "Abm") |
| `myTag` | 4 groups | rekordbox's defaults: Genre, Components, Situation, and one more |
| `property` | 1 | `dbVersion` is the **text** `'1000'` (DjManager's `'10000'` is a typo) |

**Useful shortcut:** the 27 `menuItem` rows are the *same list, in the same
order, with the same kind codes* as the legacy `export.pdb`'s Columns table
(0x80 GENRE, 0x81 ARTIST, 0x82 ALBUM, 0x83 TRACK, 0x85 BPM … 0x8C DATE ADDED,
0x90 FOLDER, 0x97 DJ PLAY COUNT, 0xA1 DEFAULT, 0xA2 ALPHABET, 0xAA MATCHING).
If you already write a pdb, generate both from one constant.

### Field notes

- `bpmx100` is centi-BPM (12400 = 124.00). `length` is seconds. `rating` is
  **0–5**, not the pdb's 0/51/…/255.
- `path` is drive-relative with forward slashes: `/Contents/Artist/Album/x.flac`.
- `fileType`: the majority view is MP3=1, M4A/AAC=4, FLAC=5, WAV=11, AIFF=12.
  rbox's enum disagrees. Nobody has published codes read back from a real
  export beside the files they describe — verify before trusting.
- `analysedBits`: fourfour writes 105 for a fully analysed track; DjManager
  reports 41 from real exports. **This field suppresses re-analysis** (see §6).
- `contentLink`: 788224 (0x000C0700) on every track in one examination;
  fourfour writes 0 and it worked. Meaning unknown.
- `playlist.attribute`: 0 = playlist, 1 = folder, 4 = smart; parent 0 = root.

### rekordbox leaves `cue` empty

Two independent examinations of real exports — one with three hot cues on one
track and memory cues, a hot cue and a saved loop on another — found **zero
rows** in `cue`. The cues a player shows come from the ANLZ files, exactly as
on a legacy drive. Several writers insert `cue` rows anyway; no evidence a
player reads them.

A djay-written drive carries two extra tables (`djay_content`,
`djay_migrations`) and players tolerate them, so the schema is additive.

## 5. The analysis files, and the directory name that matters

Same `PIONEER/USBANLZ` tree as the legacy format; OneLibrary adds nothing to
the ANLZ files. Measured across ~700 tracks of two real rekordbox exports:

| File | Sections, in order |
|---|---|
| `.DAT` | `PPTH`, `PVBR`, `PQTZ`, `PWAV`, `PWV2`, `PCOB`×2 |
| `.EXT` | `PPTH`, `PWV3`, `PCOB`×2, `PCO2`×2, `PQT2`, `PWV5`, `PWV4`, `PSSI` |
| `.2EX` | `PPTH`, `PWV6`, `PWV7`, `PWVC` |

Two corrections to things often repeated: **`PSSI` (phrase/lighting data) is in
the `.EXT`, not the `.2EX`**, and the `.2EX` holds only the three-band
waveforms plus a 20-byte `PWVC` summary.

**Order and section list matter, and getting them wrong is silent.** See §6:
writing a second `PQTZ` in the `.EXT` where rekordbox writes `PQT2` cost a
CDJ-3000X everything after that tag — the colour waveforms and the phrases —
while the cues *before* it came through fine. A player reads these files in
order and stops making sense of one at the first section it did not expect.
`PQT2` is "an extended beat grid, two bytes a beat" and that is all anyone has
published, so writing nothing there is safer than writing a guess: a missing
section is skipped, a wrong one costs everything behind it.

### The directory name

rekordbox names each track's analysis directory `P{3 hex}/{8 hex}` — real
drives show `P00E/000281CE`, `P04A/000189B4`. **The player recomputes this from
the audio path and ignores `analysisDataFilePath`.** fourfour got the algorithm
by disassembling `CreateAnlzFileFolderPath` out of the rekordbox binary:

```python
def analysis_dir(path):                  # path as the player sees it, e.g.
    h = 0                                # "/Contents/Artist/Track.flac"
    for ch in path:                      # UTF-16 code units
        c = ord(ch) & 0xFFFF
        h = ((h * 0x5BC9 + c) & 0xFFFFFFFF)
        h = ((h * 0x93B5 + c) & 0xFFFFFFFF)
    r = h % 200003
    p = ((r >> 0) & 1) | ((r >> 1) & 2) | ((r >> 4) & 4) | ((r >> 4) & 8) \
      | ((r >> 5) & 16) | ((r >> 8) & 32) | ((r >> 10) & 64)
    return f"/PIONEER/USBANLZ/P{p:03X}/{r:08X}"
```

Caveat: two of the three worked examples fourfour publishes reproduce exactly;
the third gives `P028/0000F098` where `P012/0000530C` is claimed. I'd treat the
algorithm as correct and that row as a typo, but check against a real export if
you can.

Consequence people miss: the second component is a hash **modulo 200003**, so a
library of a few thousand tracks will put two tracks in one directory. That is
what `ANLZ0001`, `ANLZ0002` are for — and it is presumably why the `PPTH`
section inside each file has to name its own track exactly, and why fourfour
found that a `PPTH` missing its UTF-16 null terminator gets the file rejected
and a fresh `ANLZ0001.DAT` written beside it.

## 6. The hardware results

Both on a stick written entirely by third-party code (no rekordbox involved),
carrying **both** databases, FAT32.

### CDJ-3000X, firmware 1.40 — reads the database, found no analysis

**Worked:** playlists, folders, the track list, titles, artists, key search,
and BPM *in the track list*.

**Did not appear on loading a track:** waveform, beat grid, phrases, hot cues,
memory cues, and BPM on the deck.

**And there was no pause.** The player did not analyse to fill the gap.

That combination is diagnostic, and it is worth spelling out because it is easy
to misread as a partial success. Everything visible came from
`exportLibrary.db`; everything missing lives in the ANLZ files. The files were
on the stick and `analysisDataFilePath` pointed straight at them — but the
directories were named after the track id rather than by the hash in §5, so the
player looked at a name that didn't exist and found nothing. It then showed
nothing rather than measuring its own, because `analysedBits` told it the track
was already analysed.

Practical upshot for anyone writing this format:

- `analysisDataFilePath` is, as far as the CDJ-3000X is concerned, decoration.
  Deep Symmetry's analysis says players use the stored path; that does not
  describe this player. fourfour's "the CDJ computes it itself" does.
- A drive that browses beautifully is **not** evidence your analysis files are
  right. Load a track and look at the waveform. Every third-party hardware
  report I could find — dj-usb-tkit's CDJ-3000X pass, FableGear's CDJ-3000 pass
  — explicitly did not record this, so neither of them establishes that their
  analysis files were ever opened either.

### CDJ-3000X again, after fixing the directory name — waveforms, but monochrome

Same player, same stick rewritten with the hash above. Waveforms appeared in
the browse preview column, which confirms the diagnosis. But they were
**monochrome** — the mono preview out of `PWAV` in the `.DAT` — with no colour
waveform, no phrases.

Cause: the `.EXT` had a second `PQTZ` where rekordbox writes `PQT2`, and
`PWV5`, `PWV4` and `PSSI` all sit *after* it in the file. Cues, which sit
before it, worked. So: sections after an unexpected one are not read, and the
player reports nothing about it — you get a plausible-looking waveform drawn
from the older, simpler section in the other file.

If you are writing these files, match a real export's section list and order
exactly, per file, and treat "I can see a waveform" as insufficient evidence.
Fixed by dropping the wrong tag (not by inventing a `PQT2`), reordering the
`.2EX` to `PWV6`, `PWV7`, `PWVC`, and writing `PVBR` in the `.DAT` even for
lossless files, as real exports do. Retest pending.

### And once colour appears, it is a picture rather than a spec

With the `.EXT` read through, the CDJ-3000X drew colour waveforms and hot-cue
markers — but pale ones. Nothing in the format says what colour a given piece
of audio should be: `PWV5` gives you three bits each of red, green and blue per
column and the rest is up to the writer.

Two mistakes are easy here, and both look like "the player is washing out my
colours":

- **Measuring each band's share against the sum of the three.** They then add
  up to one, so the loudest band can never reach the top of its three bits and
  every column comes out a different shade of grey. Measure against the
  *loudest* band instead, so it saturates and the others fall away from it.
- **Colouring from peak amplitude.** A kick drum's transient has energy in
  every band at once, so peak-coloured columns are white whatever the track is
  made of. Colour from an average over the column; keep the peak for the
  *height*, which is what makes a waveform look like the track.

Band corners are worth thinking of as picture-making rather than mixing
choices, too: a 2 kHz upper crossover splits the middle of most music across
two bands and draws it yellow-white, where putting it around 4 kHz keeps
basslines through vocals in one band and one colour.

### CDJ-3000, firmware 2.05 — did not see the library at all

The same stick, on a player that reads only the legacy `export.pdb`: no
library. Since the 3000 never reads `exportLibrary.db`, this says nothing about
OneLibrary — it means a from-scratch `export.pdb` was refused outright.

Worth knowing because that file **parses cleanly under two independent
parsers**. Whatever is wrong is something a parser tolerates and a player does
not. fourfour's notes list the fields they found by bisecting a CDJ-3000's
acceptance, and they're the best available checklist:

- the file header's `sequence` must exceed every data page's (their reported
  failure mode: works with 6 tracks, breaks at 11);
- the Columns table's data pages use a *different* header convention from every
  other table (`unknown5` = row count, `num_rows_large` = 0, group count + 1,
  sequence fixed at 3);
- the Tracks and History header pages carry `unknown7 = 1` and two constants at
  offsets 0x38/0x3C that no other table has;
- per-page: `unk3 = (rows % 8) * 0x20`, `unk4 = ceil(rows / 16)`, sequence =
  base + (rows − 1) × 5 with a per-table base;
- **the history tables must not be empty** — their fix was to embed pages
  copied out of a working rekordbox export.

I've implemented all of those except the last and it is still untested. If
someone with a CDJ-3000 and a byte-diff against a real stick wants to finish
this off, that's the open one.

## 7. Still unknown

- Does any player need `exportExt.pdb`?
- Which players actually require the `.2EX`, and does `PWVC` matter?
- Real `fileType` / `analysedBits` / `contentLink` values read from rows beside
  the files they describe.
- Does a CDJ-3000 on 3.22 read `exportLibrary.db` when `export.pdb` is absent?
  (FableGear reports a OneLibrary-only stick playing on a CDJ-3000, which
  officially cannot happen.)
- What `myTagMasterDBID` is for.
- Whether the empty-history-tables rule is really what refuses a pdb.

## 8. Claims worth not repeating

| Claim | Reality |
|---|---|
| rekordbox writes both databases from 6.8.2 | 6.8.1 (2023-12-19) |
| OneLibrary's schema has never been published | Not by AlphaTheta; fully documented by third parties, with a real export as a public fixture |
| Standard SQLCipher can't open it / you need rekordbox's `sqlite3.dll` | Stock SQLCipher 4 defaults work |
| `exportLibrary.db` uses the same key and schema as `master.db` | Different key, different schema |
| `property.dbVersion` is `'10000'` | `'1000'` |
| "Works on CDJ-3000" proves an `exportLibrary.db` is right | The CDJ-3000 reads `export.pdb`; if a writer emits both, the pdb may be doing the work |
| Phrase data is in the `.2EX` | It's `PSSI` in the `.EXT` |
| A waveform on screen means your analysis files are being read | It may be the mono `PWAV` from the `.DAT` while the whole `.EXT` after a bad section goes unread |
| A drive that browses proves the analysis files are good | See §6 |

## 9. Sources

Official: [rekordbox USB export
table](https://rekordbox.com/en/support/usb-export/) ·
[OneLibrary FAQ](https://rekordbox.com/en/support/faq/onelibrary-7/) ·
[6.8.1 notes](https://rekordbox.com/en/2023/12/rekordbox-v681-release-information/) ·
[7.2.5 notes](https://rekordbox.com/en/2025/10/rekordbox-v725-release-information/) ·
[AlphaTheta USB notice, Mar 2026](https://alphatheta.com/en/information/important-notice-for-customers-using-usb-devices-with-our-dj-equipment/) ·
[CDJ-3000 3.30 notice](https://www.pioneerdj.com/en/news/2026/cdj-3000-firmware-ver330-important-notice/)

Reverse engineering: [fourfour's
PIONEER.md](https://github.com/morizkraemer/fourfour) (the path hash and the
pdb bisection — the most useful single document out there) ·
[rekordcrate PR #269](https://github.com/Holzhaus/rekordcrate/pull/269) (real
export as a fixture) ·
[pyrekordbox `devicelib_plus`](https://github.com/dylanljones/pyrekordbox) ·
[dj-usb-tkit](https://github.com/haivala/dj-usb-tkit) (seed rows, CDJ test
matrix) · [FableGear](https://github.com/fabledharbinger0993/FableGear) ·
[rekordbox-explorer](https://github.com/CarlosFranzetti/rekordbox-explorer) ·
[libdjinterop PR #196](https://github.com/xsco/libdjinterop/pull/196) ·
[clubtagger](https://github.com/mischa85/clubtagger) ·
[0xdevalias's notes](https://gist.github.com/0xdevalias/b803476793b56f7c45e6361799168eb0) ·
[Deep Symmetry's DJ Link Ecosystem
Analysis](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/)
(the foundation everything else builds on)

A note on the legal question, since it comes up: the
[Mixxx project](https://github.com/mixxxdj/proposals/pull/20) has ruled writing
this file out of scope on DMCA §1201 grounds, because it means using an
extracted key. Other projects ship the key. Worth deciding deliberately.
