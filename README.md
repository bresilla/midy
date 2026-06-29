# midy

`midy` is a Rust CLI shell for future MIDI workflows.

The project is built around [`midly`](https://github.com/kovaxis/midly), a
standard MIDI file parser/writer crate.

## ASCII timeline workflow

Read a MIDI file into a deterministic line-oriented ASCII format:

```sh
make run ARGS='read song.mid'
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
SET_NOTE id=t1n0 key=62 dur=240
DELETE_NOTE id=t1n1
TRANSPOSE semitones=2 track=1
QUANTIZE grid=120 mode=both
```

Apply it to write a new MIDI file:

```sh
make run ARGS='apply song.mid edits.txt -o changed.mid'
```

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

Supported edit commands:

- `ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96`
- `NOTE id=t0n0 ...` or `SET_NOTE id=t0n0 ...`
- `DELETE_NOTE id=t0n0`
- `DELETE_NOTES [track=0] [ch=0] [key=60] [start=0] [end=960]`
- `TRANSPOSE semitones=2 [track=0] [ch=0]`
- `SHIFT ticks=120 [track=0] [ch=0]`
- `SCALE_TIME factor=2/1`
- `SCALE_DURATION factor=1/2 [key=60]`
- `QUANTIZE grid=120 mode=both`

## Commands

```sh
make build
make run ARGS='--help'
make run ARGS='--man'
make run ARGS='--version'
make run ARGS='schema'
make run ARGS='read song.mid'
make run ARGS='apply song.mid edits.txt -o changed.mid'
cat edits.txt | cargo run --bin midy -- apply song.mid changed.mid
make test
make verify
make release TYPE=patch
```
