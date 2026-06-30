# midy

`midy` is a Rust CLI shell for future MIDI workflows.

The project is built around [`midly`](https://github.com/kovaxis/midly), a
standard MIDI file parser/writer crate.

## ASCII timeline workflow

Read a MIDI file into a deterministic line-oriented ASCII format:

```sh
make run ARGS='read song.mid'
make run ARGS='read song.mid --format json'
make run ARGS='read song.mid --format csv'
```

The output starts like:

```text
MIDY_TIMELINE v1
HEADER format=parallel timing=metrical ticks_per_beat=480 tracks=2
SONG length_ticks=1920 length_beats=4.000
TRACK index=1 events=42 notes=12
NOTE id=t1n0 track=1 ch=0 key=60 name=C4 start=0 dur=480 end=480 pos=1:1:0 end_pos=1:2:0 vel=96 off_vel=64
```

Create an edit file:

```text
ADD_NOTE track=1 ch=0 key=64 start=480 dur=480 vel=90
ADD_NOTE track=1 ch=0 key=C4 at=2:1 dur=1/4 vel=90
SET_NOTE id=t1n0 key=62 dur=240
DELETE_NOTE id=t1n1
TRANSPOSE semitones=2 track=1
SHIFT by=1/8 bars=1..4
QUANTIZE grid=1/16 mode=both
VELOCITY add=8 track=1
CRESCENDO start_vel=40 end_vel=110 start=0 end=1920
```

Apply it to write a new MIDI file:

```sh
make run ARGS='apply song.mid edits.txt -o changed.mid'
```

You can also edit structured output from `read --format json` or
`read --format csv` and apply it back:

```sh
make run ARGS='read song.mid --format csv' > notes.csv
make run ARGS='apply song.mid notes.csv -o changed.mid'
```

For JSON/CSV apply, matching `id` rows update existing notes, missing ids are
deleted, and blank/new ids are added as new notes.

Or pipe edits through stdin:

```sh
cat edits.txt | cargo run --bin midy -- apply song.mid changed.mid
```

If you omit the output MIDI path while piping, `midy` overwrites the input after
the edit text and MIDI file parse successfully:

```sh
cat edits.txt | cargo run --bin midy -- apply song.mid
```

`write` is an alias for `apply`.

`apply` can also consume a modified timeline: read-only lines such as `HEADER`,
`SONG`, `TRACK`, `META`, and `EVENT` are ignored, while edited `NOTE ...` lines
act like `SET_NOTE ...` commands.

Print the full edit grammar:

```sh
make run ARGS='schema'
```

Print the long manual:

```sh
make run ARGS='--man'
```

Useful analysis/suggestion commands:

```sh
make run ARGS='suggest reduce-chords chords.mid --keep highest'
make run ARGS='suggest bass chords.mid --out-track 2 --out-ch 0 --octave 2'
make run ARGS='chords song.mid --bars 1..8'
make run ARGS='roll song.mid --track 1 --grid 1/16 --mode verbose'
make run ARGS='analyze song.mid'
make run ARGS='analyze song.mid --json'
make run ARGS='tracks song.mid'
make run ARGS='lint song.mid'
make run ARGS='fix song.mid fixed.mid'
make run ARGS='humanize song.mid human.mid --timing 12 --velocity 8 --seed 1'
make run ARGS='dehumanize song.mid tight.mid --grid 1/16 --mode both'
make run ARGS='swing song.mid swung.mid --amount 55 --grid 1/8'
make run ARGS='chordize melody.mid chords.mid --quality maj'
make run ARGS='arpeggiate chords.mid arp.mid --grid 1/16 --order up'
make run ARGS='extract song.mid --track 1 melody.mid'
make run ARGS='mute song.mid --ch 9 no-drums.mid'
make run ARGS='solo song.mid --track 1 melody-only.mid'
make run ARGS='split song.mid --by channel --out-dir channels/'
make run ARGS='merge drums.mid bass.mid chords.mid -o arrangement.mid'
make run ARGS='diff before.mid after.mid'
make run ARGS='render song.mid song.wav --soundfont piano.sf2'
```

Chord reduction is pipe-friendly:

```sh
cargo run --bin midy -- suggest reduce-chords chords.mid --keep highest \
  | cargo run --bin midy -- apply chords.mid melody.mid
```

Bass generation is also pipe-friendly:

```sh
cargo run --bin midy -- suggest bass chords.mid --out-track 2 --octave 2 \
  | cargo run --bin midy -- apply chords.mid with-bass.mid
```

Chordization and arpeggiation can be chained from a melody:

```sh
make run ARGS='chordize melody.mid chords.mid --quality maj7'
make run ARGS='arpeggiate chords.mid arp.mid --grid 1/16 --order updown'
```

Audio preview is optional and shells out to FluidSynth:

```sh
make run ARGS='render melody.mid melody.wav --soundfont piano.sf2'
make run ARGS='render melody.mid melody.flac --soundfont gm.sf2 --sample-rate 48000'
```

Supported edit commands:

- `ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96`
- `ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=1/4 vel=96`
- `NOTE id=t0n0 ...` or `SET_NOTE id=t0n0 ...`
- `DELETE_NOTE id=t0n0`
- `DELETE_NOTES [track=0] [ch=0] [key=C4] [bars=1..4]`
- `TRANSPOSE semitones=2 [track=0] [ch=0]`
- `SHIFT ticks=120 [track=0] [ch=0]` or `SHIFT by=1/8 bars=1..4`
- `SCALE_TIME factor=2/1`
- `SCALE_DURATION factor=1/2 [key=60]`
- `QUANTIZE grid=120 mode=both` or `QUANTIZE grid=1/16 mode=both`
- `HUMANIZE timing=12 velocity=8 seed=1`
- `DEHUMANIZE grid=1/16 mode=both`
- `SWING amount=55 grid=1/8`
- `VELOCITY scale=0.8`, `VELOCITY add=10`, `VELOCITY set=96`
- `VELOCITY compress=0.5 center=80`
- `CRESCENDO start_vel=40 end_vel=110 start=0 end=1920`
- `CHORDIZE quality=maj`, `CHORDIZE quality=min7`, `CHORDIZE intervals=0,4,7`
- `ARPEGGIATE grid=1/16 order=up|down|updown`
- `BLOCK_CHORD grid=1/8`
- `INVERT_CHORDS inversion=1`
- `DOUBLE octave=-1 [key=60]`
- `VOICE_LEAD max_jump=7`
- `MUTE [track=2] [ch=9]`
- `SOLO [track=1] [ch=0]`
- `MOVE_TRACK from=2 to=1 [ch=0]`
- `SET_CHANNEL track=1 ch=0 [from_ch=1]`

For metrical MIDI files, edit commands accept musical aliases: note names
(`key=C4`, `key=F#3`, `key=Bb2`), positions (`at=BAR:BEAT[:TICK]`,
`pos=BAR:BEAT[:TICK]`), bar filters (`bar=2`, `bars=1..4`), and musical
durations/grids (`dur=beat`, `dur=bar`, `dur=1/8`, `grid=1/16`, `by=-beat`).
Bar and beat positions follow the file's MIDI time-signature map.

## Commands

```sh
make build
make run ARGS='--help'
make run ARGS='--man'
make run ARGS='--version'
make run ARGS='schema'
make run ARGS='read song.mid'
make run ARGS='read song.mid --format json'
make run ARGS='read song.mid --format csv'
make run ARGS='suggest reduce-chords chords.mid --keep highest'
make run ARGS='suggest bass chords.mid --out-track 2 --out-ch 0 --octave 2'
make run ARGS='chords song.mid'
make run ARGS='roll song.mid --grid 1/16 --mode verbose'
make run ARGS='analyze song.mid'
make run ARGS='tracks song.mid'
make run ARGS='lint song.mid'
make run ARGS='fix song.mid fixed.mid'
make run ARGS='humanize song.mid human.mid --timing 12 --velocity 8 --seed 1'
make run ARGS='dehumanize song.mid tight.mid --grid 1/16 --mode both'
make run ARGS='swing song.mid swung.mid --amount 55 --grid 1/8'
make run ARGS='chordize melody.mid chords.mid --quality min7'
make run ARGS='arpeggiate chords.mid arp.mid --grid 1/16 --order updown'
make run ARGS='extract song.mid --track 1 melody.mid'
make run ARGS='mute song.mid --ch 9 no-drums.mid'
make run ARGS='solo song.mid --track 1 melody-only.mid'
make run ARGS='split song.mid --by channel --out-dir channels/'
make run ARGS='merge drums.mid bass.mid chords.mid -o arrangement.mid'
make run ARGS='diff before.mid after.mid'
make run ARGS='render song.mid song.wav --soundfont piano.sf2'
make run ARGS='apply song.mid edits.txt -o changed.mid'
cat edits.txt | cargo run --bin midy -- apply song.mid changed.mid
make test
make verify
make release TYPE=patch
```
