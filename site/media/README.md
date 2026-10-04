# The pictures and clips the site uses

## The pictures

Every `.png` here is taken by `scripts/site-media.sh`, which builds the window
with its layout-check hooks in, fills a scratch collection from
`booth/examples/demo_library.rs`, photographs it at twice the window's own size,
and cuts the crops. The collection is fixed, so two runs give two comparable
pictures, and none of it is anybody's music.

```sh
scripts/site-media.sh
```

Re-take them whenever the window has moved. A screenshot that is a release or
two out of date is the kind of wrong nobody notices from a diff.

| | |
|---|---|
| `booth.png` | The whole window, with a track selected. |
| `booth-sync.png` | The sync sheet: what goes on the drive and what comes off. |
| `booth-words.png` | The sheet asking whether a lyric a server found is this record's. |
| `booth-stems-ahead.png` | The sheet that prices a batch of separations before it starts. |
| `words-panel.png` | Crop: the inspector's *what it keeps saying* panel. |
| `waveform-phrases.png` | Crop: the waveform, its cue markers and the phrase bar. |
| `dock.png` | Crop: the drive dock along the bottom of the window. |

## The clips

The pages carry a still for every feature and swap it for a video the moment one
exists. Drop an `.mp4` in here under the name below and the page that wants it
picks it up on its next load — there is no HTML to edit, because `site/booth.js`
asks for the file and only builds a player when something answers.

| File | What it should show |
|---|---|
| `one-window.mp4` | Typing in the query bar and the list narrowing: `bpm:124-128`, then `key:~8A`, then `-played:30d`. Saving it as a playlist. About 20 seconds. |
| `cues-from-words.mp4` | A track with no words yet: press **Words**, let the pass run (cut the waiting), then the cue row filling and the *what it keeps saying* panel appearing. Click one of the times and let the deck jump there. |
| `lyrics-lookup.mp4` | The *Are these the words?* sheet arriving on a remix, both sets of words on screen, then **Edit…** and a line corrected by hand. |
| `stems-ahead.mp4` | Selecting a playlist, pressing **Words**, and the batch sheet naming the cost before anything starts. |
| `memory-cues.mp4` | The waveform zoomed in on a hook, the memory cue carrying its line, and — if you can film it — the same cue on a CDJ-3000X's screen. That last shot is the one nothing else on this site can stand in for. |
| `writing-a-drive.mp4` | Plugging a stick in, the dock picking it up, the sync sheet, and the write with its read-back check at the end. |
| `drive-copies.mp4` | A drive copied after a gig, and the player's own history turning into playlists in the sidebar. |

Keep them muted and short — these are a program being used, not a tutorial.
H.264 in MP4 plays everywhere; 1280 or 1600 wide is plenty, since the page never
shows one wider than about 1060 points. Something like:

```sh
ffmpeg -i capture.mov -vf "scale=1600:-2" -c:v libx264 -crf 24 \
       -preset slow -pix_fmt yuv420p -an -movflags +faststart \
       site/media/cues-from-words.mp4
```

`-an` drops the audio track: nothing here is meant to be listened to, and a
silent track is smaller and avoids a browser refusing to autoplay anything.
