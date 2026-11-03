# Formats no player opens

Two seconds of 1 kHz on the left and 1.5 kHz on the right, each at -24 dBFS
(ffmpeg's sine source is an eighth of full scale, and it is halved), encoded
with ffmpeg 6.1.1 and tagged the same way. The Vorbis and WMA files at
44.1 kHz, the Opus at 48 kHz, which is the only rate Opus decodes to:

```sh
ffmpeg -f lavfi -i "sine=frequency=1000:sample_rate=44100:duration=2" \
       -f lavfi -i "sine=frequency=1500:sample_rate=44100:duration=2" \
       -filter_complex "[0:a][1:a]amerge=inputs=2,volume=0.5[a]" -map "[a]" \
       -metadata title="Two Tones" -metadata artist="Booth Test" \
       -metadata album="Fixtures" -metadata date="2026" -metadata genre="Techno" \
       -metadata track="2/9" -c:a libvorbis -q:a 4 tones.ogg
```

and the same with `sample_rate=48000` and `-c:a libopus -b:a 96k tones.opus`,
and with `-c:a wmav2 -b:a 128k tones.wma`.

A different tone in each channel, so a decoder that swaps them or mixes them
down is caught, and synthetic, so there is nothing in them anybody owns.

`beat.*` are ten seconds of a kick on every beat at 128 BPM, a hat on every
half beat and a 320 Hz pad, in both channels, because a drive write finds a
grid and a pure tone has nothing to find one in. At low bitrates, to keep them
small:

```sh
E="0.5*exp(-30*mod(t,0.46875))*sin(2*PI*55*mod(t,0.46875))+0.12*exp(-60*mod(t,0.234375))*sin(2*PI*9000*mod(t,0.234375))+0.15*sin(2*PI*320*t)"
ffmpeg -f lavfi -i "aevalsrc='$E|$E':s=44100:d=10" -metadata title="Beat" \
       -metadata artist="Booth Test" -c:a libvorbis -q:a 0 beat.ogg
```

and with `s=48000` and `-c:a libopus -b:a 48k beat.opus`, and with
`-c:a wmav2 -b:a 64k beat.wma`.
