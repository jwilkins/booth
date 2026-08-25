# Booth — a functional specification for a rekordbox replacement

*Working title. Draft 1, August 2026.*

## 1. The thesis

Rekordbox is not loved. It is *load-bearing*. Every club in the world has CDJs behind
the booth, a CDJ will only play a drive that has been prepared in a particular way, and
rekordbox is the only program most DJs know that prepares one. The lock-in is not the
software; it is the **on-drive format**. Everything else — the browser, the analysis, the
subscription tiers, the cloud — rides along on that one dependency.

So the product is not "a nicer rekordbox". The product is:

> **A library you own, and a drive writer you trust, that makes a CDJ-3000 do things
> rekordbox cannot make it do.**

Two of those three are table stakes. The third is where this repository comes in. The
CDJ-3000 has no stem separation at all in standalone mode — rekordbox's Track Separation
lives in Performance mode, on a computer, and needs the computer to stay plugged in.
`musicai` already separates a track into vocals / melody / drums locally, offline, with
demucs, preserving sample alignment. Rendered ahead of time and written onto the drive as
first-class tracks with the parent's beatgrid and cues, that gives a **standalone
CDJ-3000 acapellas, instrumentals and drum tools with no computer in the booth** — a
capability the flagship's own software does not offer on that hardware, and which Engine
DJ shipped (pre-rendered, not real-time) to some acclaim.

That is the wedge. The rest of this document is what has to be true around it.

## 2. Scope

**In scope for v1**

- A local library: import, organise, tag, search, smart playlists.
- Analysis: tempo, beatgrid, key, structure/phrase, waveforms (including the CDJ-3000's
  3-band), loudness.
- Preparation: hot cues, memory cues, loops, colours, comments, cue templates.
- Stem rendering and the stem-aware export described in §8.
- Drive export and sync to CDJ-3000 / CDJ-3000X and the rest of the AlphaTheta line,
  with verification before the drive leaves the desk.
- Round-trip: read back what the players wrote (history, cues added in the booth).
- Import from rekordbox, Serato, Traktor, Engine DJ; export back out to them.
- A 2-deck local preview player — enough to audition and set cues, not a performance app.

**Out of scope for v1**

- A performance/DVS engine, video, lighting/DMX, streaming-service integration,
  a cloud service of any kind. Every one of these is a place rekordbox spends its
  engineering budget and its users' money, and none of them is why a DJ opens the app on
  a Tuesday afternoon.
- Pro DJ Link participation (see §12 — it is a phase-3 candidate, not v1).

**Non-goals, permanently**

- A subscription that withholds features already on the user's disk.
- Any design that makes the user's library unreadable without our software running.

## 3. What DJs actually complain about

Gathered from the AlphaTheta community forums, DJ trade press, the Mixxx issue tracker
and third-party library-tool documentation. Sources are listed in §14. Each complaint is
followed by the requirement it generates.

### 3.1 It is slow and it crashes

The forums carry a steady drip of it: 7.0.7 "doesn't work"; 7.2.10 has "too many bugs";
users report multi-minute start-ups, long stalls switching from Performance to Export
mode, and freezes while browsing. The standard advice on those threads is to uninstall
and go back to 6.8 — which tells you how much confidence the release train has earned.

→ **Startup and browsing are hard performance requirements, not aspirations** (UI-1,
UI-2). A library of 100,000 tracks must open in under two seconds and filter as you
type.

### 3.2 It corrupts the thing it exists to protect

The worst single incident is recent and instructive. CDJ-3000 firmware 3.30 (21 October
2025) switched the player to read the newer Device Library Plus database and *not* fall
back to the legacy one. DJs plugged in drives that had worked for years and got an empty
screen: music still on the stick, playlists gone. AlphaTheta suspended the firmware and
told venues to roll back to 3.20. Separately, 7.1.1 was reported to insert random time
skips that shift an already-correct beatgrid.

The lesson is not "AlphaTheta is careless". It is that **nobody verifies the drive
before it goes to the gig**. The software writes, and the DJ finds out in front of an
audience.

→ **The drive is verified by reading it back the way a player would** (SAFE-1..SAFE-5).
Export is not complete until a parser independent of the writer has walked the database
and confirmed every playlist, every track path, and every analysis file.

### 3.3 Two modes, one library, endless confusion

Export mode and Performance mode share most of their features and none of their mental
model. New users get stuck in the wrong one; experienced users don't discover that half
the prep tooling is only in the other. It is a UI seam that exists because of a licensing
history, not because of anything a DJ wants.

→ **One mode.** (UI-3.) Preparing a track and playing a track are views of the same
library, and every prep feature is available in both.

### 3.4 The subscription

Rekordbox now runs Free / Core / Creative / Professional, from $10 to $30 a month on
annual billing ($36 month-to-month), with Cloud as a further add-on. Version 7 moved
things behind paywalls that version 6 did not have. The recurring reviewer verdict is
that 7 is not worth it unless you are paying anyway. Cancellation and auto-renew draw
their own complaints.

→ **Perpetual licence, or open source, with no feature gated at runtime** (SAFE-6). The
library file format is documented and the reader is under a permissive licence so a
lapsed licence never costs anyone their prep work.

### 3.5 Cloud Library Sync is not trusted

The consensus advice from the trade press is: don't, not yet. It runs on a Dropbox
account AlphaTheta administers rather than the DJ's own; the move-vs-copy options can
delete music everywhere at once; a partial sync shows the whole library with most of it
greyed out and unplayable; and the behaviour changes between releases without
announcement.

→ **Sync is a file-level operation the user can inspect** (LIB-8, EXP-9). Any storage the
DJ already has — a NAS, their own Dropbox, a git-annex remote, a second drive — works,
because we sync files and a plain-text-adjacent database, not a proprietary service.

### 3.6 The library tools are powerful and unpleasant

My Tag, Intelligent Playlists, Related Tracks and phrase analysis are genuinely good
ideas; the browser wrapped around them is, in one reviewer's phrase, "powerful but not
always elegant". Intelligent Playlists cap at 1,000 tracks. Third-party plug-ins are not
supported at all, and there is no scripting surface. The most-quoted user review of
rekordbox on AlternativeTo is four words long: "clunky, no customization".

→ **Keyboard-first browsing, a real query language, no arbitrary caps, and a documented
extension point** (UI-4..UI-9, LIB-5, INTEROP-4).

### 3.7 Preparation is repetitive work the computer should do

Setting the same eight hot cues on the same eight structural moments of every track, for
a thousand tracks, is not craft. Neither is fixing a beatgrid that drifted because the
analyser assumed a constant tempo.

→ **Structure-aware cue templates and a beatgrid editor built for variable tempo**
(PREP-1..PREP-6, ANA-3).

### 3.8 Files that fail silently at the gig

32-bit float WAVs from a DAW, AIFF-C, WAV EXTENSIBLE headers, DRM'd AAC, sample rates
above 96 kHz, paths over 255 characters, NTFS drives — all of these look fine on the
laptop and fail on the player, several of them with no error message.

→ **A preflight that reads the actual file headers and either fixes the copy on the drive
or refuses to export it** (SAFE-2, SAFE-3). Prior art exists: this is one of the things
DJs already pay Lexicon for.

## 4. The constraint: how a CDJ actually reads a drive

Everything in §5 onwards is ordinary software. This section is the part that decides
whether the project is possible at all, so it comes first.

A prepared drive is a `/PIONEER/` (and now `/CONTENTS/`) tree containing a database of
tracks and playlists, plus one analysis file per track holding the beatgrid, cues and
waveforms. There are three generations of that database:

| Format | File | Storage | Documented? | Read by |
|---|---|---|---|---|
| **Device Library** (legacy) | `PIONEER/rekordbox/export.pdb` + `.DAT`/`.EXT`/`.2EX` analysis | DeviceSQL, page-based, unencrypted | **Yes** — reverse-engineered byte-for-byte, two independent parsers | Everything since CDJ-2000 (2009), **including the CDJ-3000 as it ships today** |
| **Device Library Plus** | `exportLibrary.db` | **Encrypted** SQLite | No | OPUS-QUAD and later |
| **OneLibrary** | Device Library Plus, rebranded and opened to partners | Encrypted SQLite | **No** — spec not public | CDJ-3000X, CDJ-1500X, XDJ-AZ, XDJ-AN, OPUS-QUAD, OMNIS-DUO; djay Pro and rekordbox write it, Traktor announced |

Two things about that table are worth stating flatly, because both are easy to
assume the other way round.

**The CDJ-3000X does not fall back to a legacy drive.** AlphaTheta's own
compatibility notice lists Device Library as "–" for the CDJ-3000X, and the
rekordbox FAQ says it directly: "The CDJ-3000X can browse tracks and playlists on
a USB storage device only if OneLibrary (formerly Device Library Plus) has been
exported to that device." What makes a modern rekordbox-written stick work on
both old and new players is that rekordbox 6.8.2 and later write *both*
databases to the same drive — not that the new player reads the old format.
Anecdotes that a 3000X played someone's old stick are almost certainly this:
the stick had been re-synced by a recent rekordbox and carries both.

**OneLibrary is standardised, not open.** It is a shared format agreed between
AlphaTheta, Algoriddim and Native Instruments, and AlphaTheta's own page says
only that they are "working with other brands". There is no published
specification, no SDK, and no developer programme on any AlphaTheta page found.
The Mixxx developers looked at exactly this question and concluded the route is
a direct conversation with AlphaTheta rather than a public document. The
rekordbox OneLibrary FAQ does point at a "rekordbox for Developers" support
section, which is the first thing to chase — but until something is published,
plan as though it is closed.

Three consequences follow, and they shape the whole roadmap.

**The legacy path is fully open, and it is the path to the installed base.** The
`export.pdb` format and the `ANLZ` analysis files are documented in public, in detail, by
Deep Symmetry's DJ Link Ecosystem Analysis; there are two independent parsers
(`crate-digger`, Kaitai-based, Java; `rekordcrate`, Rust, which implements `BinWrite` as
well as `BinRead`). The analysis files carry everything the CDJ-3000 needs: `PQTZ` beat
grids with per-beat tempo, `PCOB`/`PCO2` cues and loops with RGB colours and UTF-16
comments, `PWV4`/`PWV5` colour waveforms, `PWV6`/`PWV7` the CDJ-3000's 3-band waveforms,
and `PSSI` song-structure/phrase data (XOR-masked since rekordbox 6, and the mask is
known). **A drive that a CDJ-3000 will play can be written today, from Rust, with no
agreement from anyone.**

**The CDJ-3000X requires OneLibrary, and OneLibrary is closed.** The specification has
not been published; the Mixxx developers' conclusion after looking at it was that the
route is a direct conversation with AlphaTheta rather than a public spec. Lexicon ships
support for both databases on one drive, so third-party implementation is evidently
possible — by licence, by reverse engineering, or both. We should assume it takes a
partnership and plan the product so that it is valuable before that partnership exists.

**Writing both databases is not optional, and getting it wrong is the 3.30 failure.**
Rekordbox now writes both; firmware 3.30 read only the new one and DJs lost their
playlists. Our export therefore has an explicit, visible **compatibility target** — the
user picks the oldest player they might be handed — and the verifier checks the drive
against *that player's* rules, not against ours.

### 4.1 What this means for v1

- **v1 ships the legacy Device Library writer.** It targets CDJ-3000 (current firmware),
  CDJ-2000NXS2, XDJ-1000/RX, and every rented rig older than last year. That is most of
  the world's booths — but explicitly *not* the CDJ-3000X, which is the cost of starting
  here and has to be said out loud to anyone who buys into the project.
- **v1 offers an interoperability escape hatch for OneLibrary hardware**: write the
  legacy database, then hand off to rekordbox (which is free in Export mode) or djay to
  produce the OneLibrary database from it. Clunky, honest, and it works on day one.
- **A OneLibrary writer is a phase-2 deliverable contingent on a licence.** It is
  tracked as a risk in §12, not as a promise.

### 4.2 Player facts the exporter must respect

From AlphaTheta's own documentation and the CDJ-3000 specifications:

- Audio: MP3 and AAC at 16-bit / 44.1–48 kHz; WAV, AIFF, FLAC and ALAC at 16 or 24-bit up
  to 96 kHz. No 32-bit float, no AIFF-C, no DRM'd AAC, nothing above 96 kHz.
- Filesystem: FAT16, FAT32, HFS+ (exFAT support varies by model and firmware — the
  compatibility target decides). **Never NTFS.**
- Structure: 8 folder levels deep, 10,000 folders, 10,000 files per folder.
- Full path under 256 characters.
- Playlists: rekordbox's own Intelligent Playlists cap at 1,000 tracks; exported ordinary
  playlists are reported to hold up to 9,999. We cap at whatever the compatibility target
  is known to survive and say so at export time rather than silently truncating.
- SD, where present, is limited to 32 GB.

These are not trivia. Every one of them is a way for a drive to look correct on the
laptop and fail in the booth, and every one of them is checked in SAFE-2.

## 5. Principles

1. **The drive is the product.** Every feature is judged by whether it makes the thing
   the DJ carries to the club better or safer.
2. **Nothing is discovered at the gig.** If it can be checked at the desk, it is checked
   at the desk, automatically, and the failure is shown in plain language.
3. **The library is the DJ's, in a format they can read without us.** Documented schema,
   plain files, a documented export, and a working `--json` on everything.
4. **Analysis is a suggestion, editing is cheap.** Every automatic decision — grid, key,
   phrase, cue placement — is visible, correctable in one gesture, and remembers that it
   was corrected.
5. **Slow work runs in the background and survives being interrupted.** Stem separation
   is minutes per track. It queues, it resumes, it never blocks browsing.
6. **Do not become the thing.** No subscription tiering of features, no cloud that owns
   the library, no mode split.

## 6. The user interface

### 6.1 Shape

One window. Three columns and a dock, no modes, no tabs across the top that change what
the words mean.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ ⌕ bpm:124-128 key:8A -played:30d tag:peak            ⌘K                 ◐ 12 queued  │  ← command bar
├──────────────┬───────────────────────────────────────────────────┬───────────────────┤
│ COLLECTION   │  ARTIST            TITLE           BPM  KEY  ENERGY│  ▸ NOW            │
│  All tracks  │  ▸ Peverelist      Roll With The…  128  8A   ▓▓▓▓░ │  ┌──────────────┐ │
│  Unprepared  │    Batu            Marius          130  4A   ▓▓▓░░ │  │  cover       │ │
│  Recent      │    Pessimist       Hunter          172  11B  ▓▓▓▓▓ │  └──────────────┘ │
│              │    Bruce           Just Getting…   128  8A   ▓▓▓░░ │  Roll With The…   │
│ PLAYLISTS    │    …                                              │  Peverelist       │
│  ▾ Sat 14/9  │                                                   │  128.02  8A       │
│    ├ warm    │                                                   │  -8.4 LUFS  0.2dBTP│
│    ├ peak    ├───────────────────────────────────────────────────┤  ── stems ───────  │
│    └ close   │ ╔═══ waveform, 3-band ═══════════════════════════╗│  ✓ vocals         │
│  ▾ Digging   │ ║▁▂▄█▇▄▂▁▂▄█▇▆▄▂▁▃▅█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃║│  ✓ melody         │
│    Promos    │ ╚═╪═════╪═══════╪═════════╪═════════╪═══════════╝│  ✓ drums          │
│    To grid   │  A     B        C         D         E             │  ⋯ rendering 62%  │
│              │ [intro][ build ][  drop  ][ break ][   drop   ]   │  ── related ────  │
│ SMART        │  ▲ 1  16  32   48   64   80   96  112  128  144   │  Batu – Marius    │
│  Peak 128±3  │                                                   │  Bruce – Just G…  │
│  Needs grid  │  grid ✓ 128.02   key 8A ✓   phrase ✓   loud ✓     │                   │
├──────────────┴───────────────────────────────────────────────────┴───────────────────┤
│ DRIVES   ▣ SANDISK-64 (Sat 14/9 · 412 tracks · 51.2 GB) ─── 8 to add, 2 changed  SYNC │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

Four things about that layout are deliberate.

**The command bar is the search.** One field, always focused on `⌘K`, taking a query
language rather than a set of dropdowns (§6.3). Everything the browser can filter by is
expressible as text, which means it is also scriptable and shareable.

**The prep view is under the list, not instead of it.** Rekordbox makes you leave the
browser to work on a track. Here, arrow-keying down the list moves the waveform; every
prep action is a keystroke away from the list you are digging in.

**The drive dock is always visible, and always tells you the delta.** "8 to add, 2
changed" is the single most important number in the app and it should never require
navigating to find. Sync is a diff, not an export.

**The inspector shows analysis confidence, not just values.** `grid ✓ 128.02` versus
`grid ⚠ 128.0 (drifting)` is the difference between a set that mixes and one that
doesn't.

### 6.2 The prep view

Selecting a track expands the strip into the editor. Nothing modal; the list stays.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ Peverelist — Roll With The Punches            128.02 BPM   8A   6:41   FLAC 24/44.1  │
├──────────────────────────────────────────────────────────────────────────────────────┤
│ ║▁▂▄█▇▄▂▁▂▄█▇▆▄▂▁▃▅█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃▁▂▄▆█▇▅▃║  │
│  ┊A      ┊B          ┊C              ┊D          ┊E                    ┊F            │
│ [ intro  ][  build   ][    drop      ][  break   ][      drop          ][  outro   ]  │
├──────────────────────────────────────────────────────────────────────────────────────┤
│ ▁▂▃▄▅▆▇█▇▆▅▄▃▂▁ ← zoomed, one bar per division, beats ticked, downbeats tall         │
│ ├───┼───┼───┼───┼───┼───┼───┼───┤   grid: anchored at 0:00.412, 128.02, 1 tempo region │
├──────────────────────────────────────────────────────────────────────────────────────┤
│ CUES     A intro     B first drop  C break     D second drop  E outro   F ·  G ·  H · │
│ LOOPS    1  8-bar @ C   2  4-bar @ E                                                  │
│ TAGS     peak · rolling · dub · 2019 · [+]                                            │
│ NOTES    works out of anything in 8A. long intro, start on B if tight.                │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

Keyboard, throughout: `1–8` jump to hot cues, `⇧1–8` set them, `G` nudges the grid,
`⌥←/→` moves the anchor by one sample-accurate step, `[`/`]` set a loop, `T` opens the
tag field, `S` queues stems, `E` adds to the export set. No dialog boxes.

### 6.3 The query language

The browser filters as you type. The grammar is small and total — anything in the schema
is queryable:

```
bpm:124-128            key:8A              key:~8A          (compatible keys)
tag:peak -tag:vocal    energy:>=4          added:<14d
played:never           played:>30d         plays:>3
missing:grid           missing:stems       missing:artwork
loudness:<-12          bitrate:<256        format:flac
path:~/Music/promos    playlist:"Sat 14/9" rating:>=4
in:drive:SANDISK-64    dupes:title+artist
```

Bare words are a fuzzy match over artist, title, album, label, comment and filename.
Terms are AND by default, `|` is OR, `-` negates, quotes group. **A saved query is a
smart playlist** — there is no second concept and no separate editor, and it has no
1,000-track ceiling.

### 6.4 Batch prep

Select a hundred tracks, press `⇧S`, and the queue does the analysis, the stems, the
loudness pass and the cue template in the background while you carry on digging.
Progress is a single line in the command bar (`◐ 12 queued`) that expands to a list.
Anything that fails goes into a **Needs attention** smart playlist with the reason
attached, rather than a modal that interrupts the session.

### 6.5 The sync sheet

The one screen that must never be wrong:

```
┌─ SYNC → SANDISK-64 ──────────────────────────────────────────────────────────────────┐
│ Compatibility target:  [ CDJ-3000 (fw 3.20+) ▾ ]   writes: Device Library + ANLZ      │
│                                                    ⚠ CDJ-3000X also needs OneLibrary  │
├──────────────────────────────────────────────────────────────────────────────────────┤
│  ADD       8 tracks           2.1 GB   ▸                                              │
│  UPDATE    2 tracks (cues)      —      ▸  Batu – Marius: hot cue C moved              │
│  REMOVE    0                                                                          │
│  STEMS    24 files            5.8 GB   ▸  vocals+drums for playlist "peak"            │
├──────────────────────────────────────────────────────────────────────────────────────┤
│  PREFLIGHT                                                                            │
│   ✓ 412 files decode          ✓ paths < 256 chars      ✓ FAT32, 41 GB free           │
│   ⚠ 1 file is 32-bit float WAV  → will be converted to 24-bit on the drive           │
│   ⚠ 1 file has no beatgrid      → export anyway / fix first                          │
│   ✓ database re-read after write: 412 tracks, 6 playlists, 412 analysis files         │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

The preflight runs *before* the copy and the verification runs *after* it, and the sheet
does not close claiming success until the second one passes.

## 7. Functional requirements

Each requirement is stated with what "done" means. Anything without a testable
acceptance criterion is not a requirement, it is an aspiration, and belongs in §13.

### 7.1 Library (LIB)

| # | Requirement | Done when |
|---|---|---|
| LIB-1 | The library is a single local SQLite database with a published schema, plus the audio files where the user already keeps them. | Schema is in `docs/schema.sql`; a third party can query the library with `sqlite3` and no other software. |
| LIB-2 | Tracks are identified by content, not by path. | Moving or renaming a file does not orphan its cues; re-import matches on the existing acoustic fingerprint (`src/tag/fingerprint.rs`) before falling back to path. |
| LIB-3 | Import a folder tree without analysing it. | 50,000 files land in the library in under 60 s on an SSD, marked `missing:grid`, and are browsable immediately. |
| LIB-4 | Arbitrary user tags with a flat namespace and autocomplete. | A track can carry any number of tags; `tag:` queries them; tags survive export to the players' My Tag equivalent where one exists. |
| LIB-5 | Smart playlists are saved queries (§6.3), with no track-count ceiling. | A saved query over 100,000 tracks returns in under 200 ms and exports in full. |
| LIB-6 | Duplicate detection by fingerprint, by tags, and by file. | `dupes:` surfaces groups; merging keeps the best-quality file and unions the cue sets. |
| LIB-7 | Metadata enrichment from AcoustID/MusicBrainz, opt-in and offline-capable. | Reuses `src/tag/` unchanged; `--on-existing keep` semantics are the default so hand-curated fields are never overwritten. |
| LIB-8 | The library is portable and syncable by ordinary means. | Closing the app leaves a consistent database file; copying the database plus the audio to another machine reproduces the library exactly, with no service involved. |
| LIB-9 | Every destructive operation is undoable for the session and journalled beyond it. | Deleting a playlist, merging duplicates or clearing cues can be reverted from a visible history. |

### 7.2 Analysis (ANA)

| # | Requirement | Done when |
|---|---|---|
| ANA-1 | Tempo and beatgrid, with **variable tempo as the default model**, not a special case. | The grid is a list of tempo regions (matching `PQTZ`'s per-beat tempo capability); a live recording that drifts 2 BPM over eight minutes grids without manual anchors. |
| ANA-2 | Downbeat detection, phrase-locked. | Bar 1 lands on the musical downbeat on ≥95% of a 200-track 4/4 electronic test set, measured against hand-gridded ground truth. |
| ANA-3 | Grid editing is direct and sample-accurate. | Drag a beat marker and the region re-fits; `⌥←/→` nudges by a sample; a manual edit pins that region against re-analysis forever. |
| ANA-4 | Key detection with a documented algorithm and a confidence figure. | Camelot and classical notation; ≥85% exact and ≥95% within a relative/neighbour key on a public labelled set; confidence shown in the inspector. |
| ANA-5 | Structure/phrase analysis producing intro / build / drop / break / outro with bar-accurate boundaries. | Exports as `PSSI` so the CDJ-3000 shows phrases on its waveform; boundaries are editable; disagreements are recorded for retraining. |
| ANA-6 | Waveforms in every format the target players read. | `PWAV`, `PWV3`, `PWV4`, `PWV5` and the CDJ-3000's 3-band `PWV6`/`PWV7` are all generated and byte-verified against a rekordbox-produced reference for the same audio. |
| ANA-7 | Loudness and true-peak per track and per playlist. | Reuses `src/loudness.rs` and `src/normalize/` — EBU R128 integrated loudness, LRA, dBTP, and ReplayGain 2.0 tags written without re-encoding. |
| ANA-8 | Analysis is incremental, parallel and interruptible. | Uses `rayon` as the existing pipeline does; killing the app mid-analysis loses at most the in-flight track; nothing half-written reaches the database. |
| ANA-9 | Every analysis result records which version of which analyser produced it. | Upgrading the beatgrid model offers to re-run only tracks below the new version, and never touches a hand-edited grid. |

### 7.3 Preparation (PREP)

| # | Requirement | Done when |
|---|---|---|
| PREP-1 | Eight hot cues, memory cues and saved loops, with colours and comments. | Round-trips through `PCO2` including RGB colour and UTF-16 comment, and reads back identically. |
| PREP-2 | **Cue templates**: a named set of cue placements defined against structure, applied in bulk. | "Cue A = first downbeat, B = first drop, C = 32 bars before the last drop, D = outro start" applies to a 500-track selection in one action, using ANA-5's boundaries. |
| PREP-3 | Cue placement is always quantised to the grid, with an explicit unquantised escape. | Cues land on a beat unless the user holds a modifier; a cue set before a grid edit follows the grid. |
| PREP-4 | Track colour, rating, energy (1–5) and free-text notes. | All four export to the fields the players display, and all four are queryable. |
| PREP-5 | Bulk edit over a selection. | Tag, colour, rate, re-key, re-grid, apply template, queue stems — all work on multi-select with a single undo entry. |
| PREP-6 | A 2-deck preview player with pitch, key-lock and looping. | Auditioning a mix point does not require exporting; cue points set while previewing are saved immediately. |

### 7.4 Export and sync (EXP)

| # | Requirement | Done when |
|---|---|---|
| EXP-1 | Write a Device Library drive: `export.pdb` plus per-track `.DAT`/`.EXT`/`.2EX`. | A CDJ-3000 plays the drive, shows the right waveform colours, phrases, hot cues, comments and playlist tree. Verified on hardware, not in theory. |
| EXP-2 | Sync is a delta. | Re-syncing an unchanged playlist copies zero bytes and rewrites only the database; a changed cue rewrites one analysis file. |
| EXP-3 | The **compatibility target** is explicit and drives every check. | Selecting "CDJ-2000NXS2" disables 3-band waveform generation and warns on FLAC over 48 kHz; selecting "CDJ-3000X" warns that OneLibrary is required (§4.1). |
| EXP-4 | Multiple drives, tracked independently, with their own contents and history. | The dock lists every known drive with its last sync time, whether it is plugged in or not. |
| EXP-5 | Export a subset by query. | `in:playlist:"Sat 14/9" \| tag:emergency` is a valid drive definition, re-evaluated at each sync. |
| EXP-6 | Filenames on the drive are normalised and path-length safe. | Deterministic transliteration, a documented naming scheme, and no path over the target's limit — with the mapping recorded so the drive can be read back. |
| EXP-7 | Export to rekordbox XML, Serato, Traktor and Engine DJ. | Cues, loops, grids, playlists and ratings survive a round trip; what cannot survive is listed before the export runs, not after. |
| EXP-8 | The drive carries a machine-readable manifest of what we wrote. | A JSON sidecar in our own namespace records source paths, hashes, analyser versions and the query that produced each playlist — so a drive can be reconstructed, diffed, or re-imported on another machine. |
| EXP-9 | Nothing on the drive is deleted without being named first. | Removals are listed in the sync sheet with their sizes and require an explicit confirmation; audio is never deleted from the user's own storage by a drive operation, ever. |

### 7.5 Safety and verification (SAFE)

| # | Requirement | Done when |
|---|---|---|
| SAFE-1 | **Read-back verification** after every write, by a parser that does not share code with the writer. | The verifier walks `export.pdb` and every analysis file and reports track count, playlist tree, and any track whose file is missing or whose grid failed to parse. A failure blocks the "done" state. |
| SAFE-2 | **Preflight** on file compatibility, reading real headers. | Detects 32-bit float WAV, WAV EXTENSIBLE oddities, AIFF-C, DRM'd AAC, >96 kHz, unsupported bit depths, and offers a fixed copy on the drive without touching the original. |
| SAFE-3 | Preflight on the medium. | Filesystem, free space, folder depth and file-count limits, and path length are all checked before a byte is copied. |
| SAFE-4 | Drive backup and restore, including our manifest and the analysis files. | One command produces an archive that restores a byte-identical drive. |
| SAFE-5 | Safe eject that is actually safe. | Sync flushes, verifies, then unmounts; interrupting the copy leaves a drive whose previous state is still complete and playable. |
| SAFE-6 | No feature is gated at runtime by a licence check. | The application runs, exports and reads the library with no network and no account. |
| SAFE-7 | Crash-only design for the library. | `kill -9` at any point during any operation leaves a database that opens cleanly on the next run. |

### 7.6 Round trip and history (HIST)

| # | Requirement | Done when |
|---|---|---|
| HIST-1 | Read back the players' history from the drive after a gig. | Sets appear as dated playlists with played timestamps; `played:` and `plays:` queries reflect them. |
| HIST-2 | Merge cues and grids edited on the player back into the library. | Changes made in the booth are shown as a diff and accepted or rejected per track — never silently overwritten in either direction. |
| HIST-3 | A set report. | Track list with times, keys, BPMs, and the transitions actually used, exportable as text/CSV for reporting and for royalties. |

### 7.7 Interoperability (INTEROP)

| # | Requirement | Done when |
|---|---|---|
| INTEROP-1 | Import an existing rekordbox 6/7 library directly. | Reads `master.db` (SQLCipher, key publicly known — `pyrekordbox` demonstrates it) and the XML export; brings across playlists, cues, grids, My Tags, colours, ratings and play counts. |
| INTEROP-2 | Import Serato, Traktor and Engine DJ libraries. | Same fidelity bar as INTEROP-1, with a pre-import report of what will not survive. |
| INTEROP-3 | Everything the GUI does, the CLI does. | Single binary, `--json` output on every command, exit codes that mean something. This is how the tool gets scripted into other people's workflows — and it is the extension point rekordbox has never had. |
| INTEROP-4 | A documented plug-in interface for analysers. | A third-party beatgrid or key detector can be dropped in and selected per track, because ours will not always be the best one. |

## 8. Stems on standalone CDJs — the differentiating feature

### 8.1 The gap

| Platform | Stems where? | Standalone hardware? |
|---|---|---|
| Serato, VirtualDJ, djay, Traktor | Real-time, in software | No — needs the laptop |
| rekordbox Track Separation | Real-time, in software, Performance mode | **No** — the CDJ-3000 gets stems only while a computer is running rekordbox |
| Engine DJ | **Pre-rendered during preparation** | **Yes** — SC/Prime players do stems with no computer |
| **This spec** | **Pre-rendered during preparation, by demucs, locally** | **Yes — on a CDJ-3000, which its own vendor's software cannot do** |

Engine DJ proved both the appetite and the approach: prepare the stems at home, play them
standalone. Rekordbox's own stems are consistently rated below djay, VirtualDJ and Serato
for vocal isolation, and it renders three stems where others render four or five. There
is room to be better on quality *and* to be the only option on the hardware that matters.

The whole rendering pipeline already exists in this repository: `musicai stems` drives
demucs, sums its four stems into three, writes them in the parent's format, and copies
the parent's tags with the stem name appended to the title. What follows is what has to
be added around it to make the output *play well in a booth*.

### 8.2 The core insight: the timeline is preserved

Demucs is a masking separator. Every stem it emits has **the same sample rate, the same
channel count and the same length as the input**, sample-for-sample — `StemSet` in
`src/stems/mod.rs` guarantees it, and `remix()` in the test suite proves the three parts
add back up to the original.

That single property is what makes this cheap. The beatgrid, the phrase boundaries and
every cue point of the parent track are **valid verbatim** on each stem. There is no
re-analysis, no re-gridding, no drift, and no risk that the acapella and the instrumental
disagree about where bar 1 is. Rendering a stem is a file operation; the musical
preparation is inherited.

### 8.3 Stem kits

A **stem kit** is a parent track plus a chosen set of rendered companions, prepared and
exported together as an atomic unit.

| Companion | Content | Typical use |
|---|---|---|
| `acapella` | vocals | Layering over another instrumental; mashups |
| `instrumental` | melody + drums (+ bass) | Playing under someone else's vocal; radio-unfriendly edits |
| `drums` | drums | Beat tools, drop transitions, filling a break |
| `bass` | bass, when the 4-stem model is used | Bass swaps between decks |
| `no-drums` | everything but drums | Long ambient blends |
| `no-vocals` | melody + drums (+ bass) | Same as instrumental; kept distinct for clarity |

Kits are defined per playlist, not per library: rendering every companion for 30,000
tracks is neither useful nor affordable in disk. The default is "acapella + instrumental
for anything in a playlist marked for a gig".

### 8.4 Requirements (STEM)

| # | Requirement | Done when |
|---|---|---|
| STEM-1 | Stems render offline, locally, in a background queue that survives quitting. | Queue state is in the library database; relaunching resumes; the UI never blocks. Reuses `src/stems/demucs.rs`. |
| STEM-2 | 4-stem separation (vocals / drums / bass / other), with the 3-stem sum kept as a preset. | `Stem` gains a `Bass` variant; the existing melody = bass + other summing becomes one of several documented recipes rather than the only one. |
| STEM-3 | **Stems inherit the parent's grid, phrases, cues, loops, key and colour, unmodified.** | A rendered acapella exports with byte-identical `PQTZ` and `PCO2` sections to its parent; verified in a test. |
| STEM-4 | **Stems are loudness-corrected by re-encoding, not by tagging.** | Players ignore ReplayGain, and a separated stem is both quieter than the mix on average and liable to peak above it. Each rendered stem gets EBU R128 gain applied to the samples with a −1.0 dBTP ceiling using `src/normalize/limiter.rs`, and the drive copy carries no ReplayGain tag at all. |
| STEM-5 | Clipping is impossible on the drive, not merely reported. | The existing "N samples clipped; the stem peaks above full scale" warning becomes an automatic attenuate-or-limit decision on the export copy, chosen by the same `--on-peak` policy the CLI already has. |
| STEM-6 | Every rendered stem is quality-scored, and bad ones are flagged rather than shipped silently. | `tests/stem_isolation.rs`'s method — transcribe the mix and the stems, check the sung words come back from the vocal stem and not the others — runs as a per-track score. Below threshold, the kit lands in **Needs attention** with the reason. |
| STEM-7 | Companions are named and tagged so a CDJ browse list stays usable. | Title becomes `Roll With The Punches (acapella)`; a `STEM` tag records the kind (already implemented in `src/tag/copy.rs`); companions carry the parent's colour and a distinct rating/colour convention; they are excluded from the main browser view by default and from any query unless `stem:` is named. |
| STEM-8 | On the drive, a kit is one playlist entry with its companions adjacent. | Companions sort immediately under their parent in the exported playlist, so the browse list on the player reads `track / (acapella) / (instrumental)` and a companion is two turns of the jog-adjacent encoder away, not a search. |
| STEM-9 | Kits appear in the players' **Related Tracks**, where the format supports it. | Needs verification against the exported database schema (§13); if Related Tracks cannot be populated by a third-party writer, STEM-8's adjacency is the fallback and the feature ships without it. |
| STEM-10 | Disk cost is shown before it is spent. | The sync sheet quotes stem bytes separately (see §6.5); the app recommends a companion format — FLAC for headroom, 320 kbps MP3 when the drive is tight — and separated stems compress substantially better than the mix they came from. |
| STEM-11 | Rendering uses the GPU when there is one. | `--demucs-device cuda`/`mps` is selected automatically with a CPU fallback; the queue reports realistic time-to-finish (minutes per track on CPU, seconds on a GPU). |
| STEM-12 | Nothing leaves the machine. | Separation is local; no upload, no account, no cloud service — unlike every "stem splitter" web product a DJ might otherwise use on unreleased material. |

### 8.5 What this feels like in the booth

Two CDJ-3000s, no laptop. Deck 1 plays the track. You want the vocal over the next
record: load `(acapella)` on deck 2 from the same playlist — it is the next line down —
hit the same hot cue letter, and it is phase-locked because both decks are reading the
same grid from the same analysis. A break that is too short becomes a long one by looping
the `(no-drums)` companion underneath. A track with a vocal you cannot play at this gig
has an `(instrumental)` sitting next to it, prepared weeks ago.

None of that needs new firmware, a licence, or a computer. It needs the drive to be
written correctly and the stems to be rendered before you left the house — which is
exactly what this codebase already does, plus a drive writer.

### 8.6 Where stems are *not* the answer

Real-time separation on the player is impossible without AlphaTheta implementing it, and
this spec does not pretend otherwise. If you did not prepare a companion, you do not have
one. That is a genuine step down from Serato or VirtualDJ on a laptop, and the honest
positioning is "stems on club hardware, prepared in advance" — the same trade Engine DJ
makes, on the hardware Engine DJ cannot touch.

## 9. Architecture

```
┌───────────────────────────────────────────────────────────────────────────┐
│  UI            egui/eframe window (existing gui/ crate, grown up)         │
│                + CLI, same commands, --json everywhere                    │
├───────────────────────────────────────────────────────────────────────────┤
│  Library       SQLite (documented schema) · queries · smart playlists     │
│                undo journal · drive registry · job queue                  │
├───────────────────────────────────────────────────────────────────────────┤
│  Analysis      beatgrid · key · phrase · waveforms (5 kinds) · loudness   │
│                ← existing: src/loudness.rs, src/normalize/, src/dsp/      │
├───────────────────────────────────────────────────────────────────────────┤
│  Stems         queue → demucs → sum recipe → normalize → tag → score     │
│                ← existing: src/stems/, src/audio/, src/tag/copy.rs        │
├───────────────────────────────────────────────────────────────────────────┤
│  Interop       rekordbox master.db (SQLCipher) · rekordbox XML ·          │
│                Serato · Traktor · Engine DJ                               │
├───────────────────────────────────────────────────────────────────────────┤
│  Drive         Device Library writer (export.pdb) · ANLZ writer           │
│                preflight · delta sync · independent verifier · manifest   │
│                [phase 2] OneLibrary writer                                │
└───────────────────────────────────────────────────────────────────────────┘
```

Build on what is here. `musicai` is already a Rust workspace with a CLI crate and a
separate `eframe` GUI crate, symphonia decoding, LAME/flacenc/hound encoding, chromaprint
fingerprinting, `ebur128` loudness, a look-ahead limiter, an STFT, a rayon-parallel batch
pipeline, and a job model in `gui/src/job.rs` that already runs long work off the UI
thread with cancellation. The parts that do not exist are the library, the analysis
(beatgrid, key, phrase, waveforms) and the drive writer.

Two dependency decisions worth stating now:

- **`rekordcrate` for `export.pdb`.** It is Rust, it models the tables the players need,
  and it implements `BinWrite` as well as `BinRead`. Where it does not cover a table we
  need, contribute upstream rather than fork — the format documentation is a community
  asset and this project should add to it.
- **Independent read-back for verification.** SAFE-1 requires the verifier not to share
  code with the writer, so the checker uses a second implementation — the Kaitai
  structures behind `crate-digger`, generated into Rust, or a deliberately separate
  minimal parser. A writer that validates its own output is a writer that validates its
  own bugs.

## 10. Parity, and where we go past it

| Capability | rekordbox | Serato | Engine DJ | **Booth** |
|---|---|---|---|---|
| Writes CDJ-3000 drives | ✅ | ❌ | ❌ | ✅ (v1) |
| Writes CDJ-3000X / OneLibrary drives | ✅ | ❌ | ❌ | ⚠ phase 2, licence-dependent |
| Stems on a standalone CDJ | ❌ | ❌ | n/a | **✅ (pre-rendered)** |
| Stem quality | rated below peers, 3 stems | strong | pre-rendered | **demucs, 4 stems, scored per track** |
| Variable-tempo gridding | weak | weak | weak | **first-class** |
| Query language / smart playlists | dropdown rules, 1,000-track cap | smart crates | limited | **text query, no cap, scriptable** |
| Drive verified before the gig | ❌ | n/a | ❌ | **✅ independent read-back** |
| File-compatibility preflight | ❌ | n/a | ❌ | **✅** |
| Reads back booth edits and history | partial | n/a | partial | **✅ as a reviewable diff** |
| Imports the other three | ❌ | ❌ | partial | **✅** |
| CLI / scriptable / plug-ins | ❌ | ❌ | ❌ | **✅** |
| Cost | $10–$36 / month | subscription | free with hardware | **perpetual / open** |
| Lighting, video, DVS, streaming | ✅ | ✅ | partial | **❌ — deliberately** |

The honest reading of that table: rekordbox wins on breadth of ecosystem features and on
OneLibrary hardware until phase 2 lands. We win on everything a DJ does between buying a
track and playing it.

## 11. Roadmap

**Phase 0 — prove the drive (6–8 weeks).** Write an `export.pdb` and ANLZ set for a
hand-made playlist of ten tracks; play it on a real CDJ-3000; verify waveforms, phrases,
cues and colours appear correctly. Nothing else matters until this works on hardware.
The analysis half is written — see §14 — and the database half is what is left.
*Exit criterion: a drive written by this project, played in a club, indistinguishable
from a rekordbox one.*

**Phase 1 — the library and the loop (3–4 months).** LIB-1..9, ANA-1..9, PREP-1..6,
EXP-1..9, SAFE-1..7, INTEROP-1. The complete prepare-and-export cycle for CDJ-3000-class
hardware, with the UI of §6. *Exit criterion: a working DJ prepares a real gig in it and
does not open rekordbox.*

**Phase 2 — stems (2–3 months, overlapping).** STEM-1..12. The queue, the kits, the
export adjacency, the quality scoring. Most of the DSP is already written; the work is
the queue, the loudness handling on export, and the browse ergonomics.
*Exit criterion: a DJ plays an acapella off a USB stick on a CDJ-3000 with no computer in
the booth.*

**Phase 3 — the rest of the ecosystem.** OneLibrary (licence permitting), INTEROP-2/4,
HIST-1..3 at full fidelity, Pro DJ Link participation for live library browse and history
capture (the protocol is documented by the same Deep Symmetry work that documents the
export format).

## 12. Risks

| Risk | Severity | Response |
|---|---|---|
| **OneLibrary stays closed**, and new hardware requires it. | High — it is the future installed base. | v1 targets the CDJ-3000 installed base, which is enormous and not going anywhere. Approach AlphaTheta early; Lexicon's support for both databases shows the door is not bolted. Keep the rekordbox hand-off escape hatch working. |
| **A firmware update changes the format** and drives stop working. | High. | This already happened to AlphaTheta's own users with 3.30. Mitigation is the compatibility target (EXP-3), the verifier (SAFE-1), a public matrix of firmware versions tested, and never being the only copy of anything. |
| **Legal pressure** over format interoperability. | Medium. | Reading and writing a file format for interoperability is well-trodden ground and the format documentation is already public and long-standing. Ship no AlphaTheta code, no circumvention of a technical protection measure on copyrighted works, and take advice before touching OneLibrary's encryption. |
| **A bad drive at a gig destroys trust instantly.** | High. | This is the entire justification for §7.5. The product's promise is reliability; one silent failure costs more than any feature gains. |
| **Beatgrid and key quality are hard**, and DJs are unforgiving. | Medium-high. | Ship measured accuracy figures against hand-labelled sets (ANA-2, ANA-4), make editing fast enough that imperfection is survivable, and allow third-party analysers (INTEROP-4). |
| **Stem rendering is slow and large.** | Medium. | Per-playlist opt-in, GPU support, honest time and size estimates before the queue starts (STEM-10, STEM-11). |
| **The audience is small and conservative.** | Medium. | Win on the drive first. A tool that only replaces the export step, and does it more safely, is already worth using — the library replaces rekordbox afterwards, not on day one. |

## 13. Open questions

1. **Can a third-party export populate Related Tracks and My Tag** on the CDJ-3000, or
   are those tables written but ignored without rekordbox's own metadata? Decides STEM-9
   and part of LIB-4. Answer by writing them and testing on hardware.
2. **Which `.2EX` fields does the CDJ-3000 actually require** for its 3-band waveform to
   render, and what does it do when they are absent or malformed? Decides ANA-6's
   acceptance test.
3. **exFAT support by model and firmware** — it matters for drives over 32 GB and the
   documentation is inconsistent. Needs a tested matrix, not a citation.
4. **How much does a rendered stem actually cost in bytes** across genres, in FLAC and
   MP3? Measure before quoting a figure in the UI.
5. **Should the preview player do real-time separation** for auditioning, or is that
   scope creep into a performance app?
6. **Is a OneLibrary partnership worth the constraints it would come with** — and would
   accepting one compromise principle 3?
7. **Where does the 3-stem/4-stem recipe boundary sit** for DJ use? `melody = bass +
   other`, as the CLI does today, is right for an instrumental and wrong for a bass swap.

## 14. Development status

Phase 0 is nearly done: a drive can be written, and the tracks on it are analysed rather
than described by hand. What exists:

**`src/export/`** — the drive.

- **`anlz.rs`** — the per-track analysis files: `PPTH` paths, `PQTZ` beat grids, `PCOB`
  and `PCO2` cue lists with colours, comments and loops, all seven waveform sections, and
  the masked `PSSI` phrase analysis, assembled into `.DAT`, `.EXT` and `.2EX`.
- **`waveform.rs`** — generating those waveforms from decoded audio in one pass, in the
  five packings the sections use, down to the CDJ-3000's three bytes of mid, high and low.
- **`pdb.rs`** — `export.pdb`, the DeviceSQL database: pages, heaps, the row index that
  builds backwards from the end of each page, the string encodings, and rows for tracks,
  artists, albums, genres, labels, keys, colours, the browse menu, and the playlist tree.
- **`mp3.rs`** — an MP3 frame walker that builds the `PVBR` variable-bitrate seek index,
  so a hot cue on a VBR file lands where it should. No decoding: it reads the frame headers.
- **`image.rs`** — the drive as a disk image rather than a folder: a master boot record
  with one FAT32 partition, which is the shape a player reads and the shape an emulator's
  USB slot takes.
- **`inspect()` in both** — readers written from the format documentation rather than
  from the writers, sharing no code with them: the beginnings of SAFE-1's verifier.

**`src/analysis/`** — what goes on it. Signal processing, not a model: seconds a track,
offline, and every decision visible.

- **`features.rs`** — one pass over the audio producing spectral flux, band energies,
  level and a vocal-presence estimate. A five-minute track measures out at about a
  megabyte rather than a spectrogram's few hundred.
- **`tempo.rs`** — ANA-1 and ANA-2. Autocorrelation with a perceptual tempo prior, then
  Ellis's dynamic program for the beat times, then the downbeat from where the kicks are.
  The tempo is read back off the tracked beats rather than off the autocorrelation lag,
  which is only accurate to about a beat per minute; where a constant tempo fits the
  beats, the grid becomes that constant tempo exactly.
- **`structure.rs`** — ANA-5. Foote's self-similarity novelty over bar-synchronous band
  features, snapped to four bars, labelled intro / build / drop / break / outro from how
  much is happening in each section.
- **`cues.rs`** — PREP-2, in its automatic form. A memory cue at the first downbeat and up
  to eight hot cues at the phrase boundaries and where a voice enters, named, coloured by
  kind, and quantised to the grid.
- **`key.rs`** — ANA-4. A chromagram read at each note's frequency (its own longer
  transform, so the bass resolves) and Sha'ath-profile correlation against all twenty-four
  keys, reported as Camelot and classical with a confidence. `Key::parse` reads the
  notations a library stores, both for measuring against one and, later, for importing one.

**Commands.** `musicai export` builds the drive and `musicai anlz` writes one track's
analysis; both listen to the audio, and both read back what they wrote before reporting
success. `--bpm` is an override rather than a requirement — and it overrides the *tempo*,
not the grid, so the beats stay tracked against the audio.

**Cross-checks** against `rekordcrate`, an independent implementation of both formats,
because our own reader agreeing with our own writer proves nothing.

**`booth/`** — §6's interface, as its own program rather than a mode of the batch tool.
One window: the query bar across the top, the collection down the left, the browser with
the prep editor beneath it rather than in place of it, and the dock along the bottom.

- **`query.rs`** — the language of §6.2. One parse produces both the terms that filter and
  the spans the bar paints, so what is highlighted is what is being matched. A term that
  does not parse matches nothing and is struck through, rather than being ignored — an
  ignored typo silently widens a search and looks like it worked. `bpm:128` finds a 128.02
  grid, and so does `bpm:124-128`: two forms of the same question must not disagree.
- **`library.rs`** — the collection. It describes what is on disk and never writes audio or
  changes a tag, which is what makes it safe to rebuild from a rescan. Saved atomically;
  a file that will not parse is an error rather than a fresh start, because replacing a
  real collection with an empty one is the worst thing a loader can do.
- **`sync.rs`** — the delta and the preflight. Whether a track needs rewriting is decided
  by a fingerprint over what the *player* will see, so a moved cue counts and a play count
  does not; a tempo that differs below the stored two decimals is not a change. The
  path-length check calls the writer's own `on_drive_path`, so the preflight cannot come to
  a different answer than the writer it is predicting.
- **`wave.rs`** — the three-band waveform, painted from the same `PWV6` bytes the drive
  will carry, peak-per-pixel so a kick stays visible, with the grid ticks and cue flags
  over it and the phrase strip beneath.

`booth/tests/end_to_end.rs` runs the whole spine on real audio — import, analyse, query,
plan, preflight, write — and reads the resulting `export.pdb` back with the independent
parser.

- **`config.rs`** — where music is kept, and what to do about music that is not kept there.
  A file played from a download folder or a borrowed stick is a file that can be gone on the
  night, so anything reached for from outside the library folder is copied in by default —
  copied, never moved, and never over the top of a different file that shares its name.

PREP-1 and PREP-2's manual halves are in: the inspector's names are editable, with an
explicit and separately-controlled write-back to the file's own tags (FLAC and MP3; a WAV
says it has nowhere to put them rather than appearing to work), free-form tags that stay in
the collection rather than reaching the drive, and cue editing against the waveform —
click to place the playhead, a button per slot, drag to move, snapped to the beat and the
memory cue to the bar. What the player would see goes through the drive delta; what it
would not — a cue's name, a tag — deliberately does not.

`booth/tests/keeping_a_copy.rs` covers the copy-in rule and the join between editing and
the delta.

What the interface does *not* yet do: a play history, importing an existing rekordbox
library, choosing cue colours, or playing anything — the playhead is a position to place
cues against, not a transport.

### What the first pass turned up

Comparing the documentation against a real rekordbox export and against a second parser
corrected several things, and left one open:

- **`PCP2` cue entries are longer than the documentation requires.** The format spec
  allows an entry to end after its colour; real rekordbox writes twenty more bytes.
- **`memory_count` is not a count.** A real export writes `0xffffffff` there even for an
  empty cue list.
- **Row heaps are four-byte aligned**, and `free_size` is what is left after the heap
  *and* the row index.
- **Two sections nobody has documented.** A real `.EXT` contains `PQT2` — an extended beat
  grid where `PQTZ` sits in the `.DAT` — and a real `.2EX` ends with a short `PWVC`.
- **Two parsers disagree about what a cue point is.** The documentation says a cue entry's
  type is 1 for a point and 2 for a loop, and says it twice; `rekordcrate` defines the
  point as 0 and cannot read a 1 at all. We follow the documentation. This has teeth now
  that every exported track carries cues: if the documentation is wrong, they will not
  appear on the player.

### What the analysis gets right, and where it does not

On a synthetic hundred-second arrangement at 126 BPM, with the drums entering at bar 8,
dropping out at 24 for a vocal breakdown, returning at 32, and a second vocal at 48, the
detector reports 126.05 BPM and cues at 0.02 s (intro), 15.23 s (drop), 45.71 s (break),
60.94 s (drop) and 91.43 s (vocal) — every boundary on the correct bar. That is a
synthetic track and a favourable one; the honest limits are:

- **Vocals are inferred, not separated.** Centred energy in the vocal band finds a sung
  line over a wide backing and also fires on a centred lead. The stems would answer it
  properly and cost minutes a track; §8's render queue is where that belongs.
- **Phrase labels are heuristic.** A breakdown that leads into a drop can be called
  either. The boundaries are the part to trust.
- **Heavy rubato is not handled.** The tracker follows one tempo estimate, so a live
  recording that drifts across a set is beyond it. Per-beat tempo is recorded, so a
  gentle drift survives; a rallentando will not.
- **No accuracy figures against a labelled set yet.** ANA-2 and ANA-4 ask for measured
  numbers on real music. `examples/eval.rs` is the harness for producing them — it reads a
  rekordbox collection export and scores the detected key and tempo against the library's
  own labels — but it needs the audio, which lives on the DJ's machine, not in the repo.
  What can be checked here already is that the harness reads a real 8,300-track v7.2.17
  export cleanly and that all 8,209 of its stored key labels parse.

### Getting to a player without a player

[cdj3k-emu](https://github.com/nsaintot/cdj3k-emu) boots real CDJ-3000 firmware under
QEMU and exposes a virtual USB slot that takes a raw `.img`. That is phase 0's exit
criterion reachable from a desk rather than a booth, and it is why `--image` exists: the
guest mounts partition 1 as FAT32 at `/media/usb/sdb1`, which is exactly what we now
write.

It is not a substitute for the real thing and it does not remove the hardware
requirement, for three reasons worth writing down:

- **Apple Silicon macOS only.** HVF, vmnet and CoreAudio. There is no Linux build, so it
  cannot run in CI, and the checks that gate a merge stay the parser-based ones.
- **It ships no firmware.** A CDJ-3000 `.UPD` and its decryption key have to come from
  the user. Neither this project nor that one can supply them.
- **It is an emulator.** Faithful enough to run EP122, explicitly not a forensic
  recreation. A drive it accepts is strong evidence; a drive a CDJ accepts is proof.

What it can settle that nothing else here can: whether the database is browsable, whether
the three-band waveform draws, whether phrases appear under it, and — the one genuinely
open question in the format — whether a cue point written as type 1 shows up at all.

### Next

1. A player, emulated or real. Everything here is checked against a parser, which is not
   the same as checked against a CDJ, and closing that gap is what is left of phase 0.
2. Accuracy run over a real library — the harness exists (`examples/eval.rs`); it needs
   the audio, which means running it on the machine the library lives on.

## 15. Sources

Complaints, format details and hardware facts referenced above:

- AlphaTheta / Pioneer DJ community forums — rekordbox 7 bug and stability threads:
  [7.0.7 doesn't work](https://community.pioneerdj.com/hc/en-us/community/posts/41343351607833-Rekordbox-7-0-7-doesn-t-work),
  [7.2.10 has too many bugs](https://community.pioneerdj.com/hc/en-us/community/posts/55336663722137-New-update-of-Rekordbox-7-2-10-has-too-many-bugs),
  [7.1.1 random time skips ruin the beat grid](https://community.pioneerdj.com/hc/en-us/community/posts/46261597128217-Rekordbox-7-1-1-Bug-random-time-skips-that-ruin-Beat-Grid),
  [rekordbox is a nightmare](https://forums.pioneerdj.com/hc/en-us/community/posts/203058519-Rekordbox-is-a-nightmare),
  [1,000-track limit on Intelligent Playlists](https://community.pioneerdj.com/hc/en-us/community/posts/22978972729369-Max-of-1000-tracks-per-playlist-Intelligent-Playlists),
  [switching from Performance to Export mode](https://forums.pioneerdj.com/hc/en-us/community/posts/207467526-Switching-from-Performance-to-Export-Mode).
- Firmware 3.30 / OneLibrary incident:
  [AlphaTheta suspends CDJ-3000 firmware update following playlist issues (DJ Mag)](https://djmag.com/tech/alphatheta-suspends-cdj-3000-firmware-update-distribution-following-playlist-issues),
  [CDJ-3000 firmware ver. 3.30 — important notice (Pioneer DJ)](https://www.pioneerdj.com/en/news/2026/cdj-3000-firmware-ver330-important-notice/),
  [What really happened (DJ LIFE)](https://djlifemag.com/2025/11/cdj-3000-firmware-3-30-issue-what-really-happened-how-to-save-your-library/),
  [AlphaTheta pulls CDJ-3000 firmware (Digital DJ Tips)](https://www.digitaldjtips.com/alphatheta-pulls-cdj-3000-firmware-after-playlist-issues/).
- Database formats:
  [Device Library Plus explained (Lexicon)](https://www.lexicondj.com/blog/everything-you-need-to-know-about-device-library-plus-and-more),
  [What is Device Library Plus? (AlphaTheta)](https://support.pioneerdj.com/hc/en-us/articles/16290620247321-What-is-Device-Library-Plus),
  [OneLibrary (AlphaTheta)](https://alphatheta.com/en/onelibrary/),
  [Can OneLibrary unite DJ tools? (CDM)](https://cdm.link/onelibrary-dj-interoperability/),
  [Add OneLibrary support (Mixxx issue #15556)](https://github.com/mixxxdj/mixxx/issues/15556),
  [OneLibrary isn't what you think it is](https://reallychrism.substack.com/p/why-im-not-cheering-onelibrary).
- Reverse-engineered format documentation and tooling:
  [DJ Link Ecosystem Analysis — analysis files](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html),
  [crate-digger](https://github.com/Deep-Symmetry/crate-digger),
  [rekordcrate](https://github.com/Holzhaus/rekordcrate),
  [pyrekordbox](https://pypi.org/project/pyrekordbox/).
- Cloud, pricing and paywalls:
  [Should DJs use rekordbox Cloud Library Sync? (Digital DJ Tips)](https://www.digitaldjtips.com/rekordbox-cloud-library-sync-advice/),
  [rekordbox plans and pricing](https://rekordbox.com/en/plan/),
  [Is rekordbox 7 worth it? (Audiomunk)](https://audiomunk.com/time-for-upgrading-is-rekordbox-7-really-worth-it/),
  [rekordbox 7 released — is it any good? (DJ.Studio)](https://dj.studio/blog/rekordbox-7-announced),
  [rekordbox on AlternativeTo](https://alternativeto.net/software/rekordbox/about/).
- Hardware and player limits:
  [CDJ-3000 supported formats and USB specs](https://boothready.app/players/cdj-3000),
  [Which file formats can I play? (AlphaTheta)](https://support.alphatheta.com/en-US/articles/4406128262681),
  [CDJ-3000X is here (DJ TechTools)](https://djtechtools.com/2025/09/09/the-cdj-3000x-is-here-an-iterative-upgrade-of-the-media-player-for-3000-2399/),
  [CDJ-3000X review (MusicTech)](https://musictech.com/reviews/dj/alphatheta-cdj-3000x-review/).
- Stems landscape:
  [Which DJ platform has the best sounding stems in 2025? (Digital DJ Tips)](https://www.digitaldjtips.com/best-sounding-stems-2025/),
  [Get rekordbox stems on any Pioneer DJ setup (We Are Crossfader)](https://wearecrossfader.co.uk/blog/rekordbox-stems/).
- Prior art in third-party library management:
  [Lexicon DJ](https://www.lexicondj.com/).

Two caveats on the above. Several forum claims (start-up times, specific crash reports)
are individual user reports on vendor forums rather than measured behaviour; they are
cited as evidence that a complaint is *common*, not that a number is exact. And the
support state of OneLibrary on the CDJ-3000 has changed twice in a year — §4 reflects the
position after the 3.30 rollback and must be re-checked before any of it is built on.
