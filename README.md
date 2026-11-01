# midy

`midy` turns binary MIDI files into deterministic, line-oriented text, and applies
text edits back to MIDI. Humans, scripts, and language models can all change a
song by writing lines like `TRANSPOSE semitones=2 track=1`.

It is built on [`midly`](https://github.com/kovaxis/midly).

## Quick start

```sh
midy tracks song.mid                         # what is in the file
midy read song.mid --track 1 --bars 1..4     # one part, as text
midy apply song.mid edits.txt --dry-run      # check edits, write nothing
midy apply song.mid edits.txt -o changed.mid # write the result
midy diff song.mid changed.mid               # see what changed
```

From a checkout, use `make run ARGS='read song.mid'`.

## Reading

`midy read` prints a timeline:

```text
MIDY_TIMELINE v1
HEADER format=parallel timing=metrical ticks_per_beat=480 tracks=2
SONG length_ticks=1920 length_beats=4.000
TRACK index=1 events=42 notes=12
META track=0 tick=0 kind=tempo us_per_quarter=500000 bpm=120.000
EVENT track=1 tick=0 kind=cc ch=0 controller=64 value=127
NOTE id=t1n0 track=1 ch=0 key=60 name=C4 start=0 dur=480 end=480 pos=1:1:0 end_pos=1:2:0 vel=96 off_vel=64
```

- `--format json` and `--format csv` give structured output.
- `--track`, `--ch`, `--key`, `--bars`, `--start` and `--end` select part of a song.
- `--compact` drops derived fields, and `--events basic|none` hides events. Use
  them to keep the output small enough for a language model.

## Editing

Write edit commands, one per line:

```text
ADD_NOTE track=1 ch=0 key=C4 at=2:1 dur=1/4 vel=90
SET_NOTE id=t1n0 key=62 dur=240
DELETE_NOTE id=t1n1
TRANSPOSE semitones=2 track=1
SHIFT by=1/8 bars=1..4
QUANTIZE grid=1/16 mode=both
VELOCITY add=8 track=1
SET_TEMPO bpm=140
SET_PROGRAM track=1 ch=0 program=33
ADD_EVENT track=1 tick=0 kind=cc ch=0 controller=64 value=127
DELETE_EVENTS kind=pitch_bend bars=1..4
ADD_TRACK name="Strings"
```

Then give them to `apply`:

```sh
midy apply song.mid edits.txt -o changed.mid
midy apply song.mid -e 'TRANSPOSE semitones=2' -o changed.mid
cat edits.txt | midy apply song.mid changed.mid
cat edits.txt | midy apply song.mid             # overwrites song.mid
```

You can also edit the output of `read` and apply that. In a text timeline, edited
`NOTE` lines act like `SET_NOTE`, and `--prune` deletes notes you removed. In JSON
or CSV, rows update notes by id, missing ids are deleted, and rows with a blank id
are added. A scoped read records its selection, so applying it never touches notes
outside that selection.

```sh
midy read song.mid --format csv > notes.csv
midy apply song.mid notes.csv -o changed.mid
```

`midy schema` prints the full edit grammar. `midy --man` prints the manual.

### Safety

- Unknown fields, bare values and unknown options are errors, with a suggestion.
- `DELETE_NOTES`, `MUTE`, `SOLO` and `DELETE_EVENTS` need a filter or `all=true`.
- `--dry-run` reports what each line matched without writing anything.
- `--strict` fails on warnings, such as a line that matches nothing.
- Pitch commands leave `ch=9` percussion alone unless you name it.
- If any line fails, nothing is written, and output files are replaced in one step.

## Other commands

```sh
midy suggest reduce-chords chords.mid --keep highest | midy apply chords.mid melody.mid
midy suggest bass chords.mid --octave 2 | midy apply chords.mid with-bass.mid
midy chords song.mid --bars 1..8
midy roll song.mid --track 1 --grid 1/16 --mode verbose
midy analyze song.mid --json
midy lint song.mid --exit-code
midy fix song.mid fixed.mid
midy humanize song.mid human.mid --timing 12 --velocity 8 --seed 1
midy dehumanize song.mid tight.mid --grid 1/16 --mode both
midy swing song.mid swung.mid --amount 66 --grid 1/8
midy chordize melody.mid chords.mid --quality maj7
midy arpeggiate chords.mid arp.mid --grid 1/16 --order updown
midy extract song.mid --track 1 melody.mid
midy mute song.mid --ch 9 no-drums.mid
midy solo song.mid --track 1 melody-only.mid
midy split song.mid --by channel --out-dir channels/
midy merge drums.mid bass.mid chords.mid -o arrangement.mid
midy diff before.mid after.mid --exit-code
midy render song.mid song.wav --soundfont piano.sf2
```

`render` needs [FluidSynth](https://www.fluidsynth.org/) and a SoundFont.

## Development

```sh
make build
make test
make verify
make release TYPE=patch
```

`tests/robustness.rs` also round-trips real files when `MIDY_CORPUS` names a
directory of `.mid` files, and `MIDY_FUZZ_ITERATIONS` sets how long it fuzzes.

## License

MIT. See [LICENSE](LICENSE).
