use std::{io::Read, path::PathBuf};

use crate::{Error, Result, midi, name};

const HELP: &str = "\
midy

Usage:
  midy read <input.mid> [--format midy|json|csv]
  midy apply <input.mid> <edits.txt> -o <output.mid>
  midy apply <input.mid> <output.mid>        # read edits from stdin
  midy apply <input.mid>                     # read edits from stdin and overwrite
  midy suggest reduce-chords <input.mid> [--keep highest|lowest|root|nth=N]
  midy suggest bass <input.mid> [--out-track N] [--out-ch N]
  midy chords <input.mid>
  midy roll <input.mid> [--mode compact|verbose]
  midy analyze <input.mid>
  midy tracks <input.mid>
  midy lint <input.mid>
  midy fix <input.mid> <output.mid> [--stuck-mode remove|close] [--remove-empty-tracks]
  midy humanize <input.mid> <output.mid> [--timing N] [--velocity N] [--seed N]
  midy dehumanize <input.mid> <output.mid> [--grid GRID] [--mode start|duration|both]
  midy swing <input.mid> <output.mid> [--amount 50..100] [--grid GRID]
  midy chordize <input.mid> <output.mid> [--quality maj|min|...] [--intervals 0,4,7]
  midy arpeggiate <input.mid> <output.mid> [--grid GRID] [--order up|down|updown]
  midy extract <input.mid> (--track N|--ch N) <output.mid>
  midy mute <input.mid> (--track N|--ch N) <output.mid>
  midy solo <input.mid> (--track N|--ch N) <output.mid>
  midy split <input.mid> --by track|channel --out-dir <dir>
  midy merge <input-a.mid> <input-b.mid> [...input.mid] -o <output.mid>
  midy diff <before.mid> <after.mid>
  midy render <input.mid> <output.wav|output.flac> --soundfont <file.sf2>
  midy schema
  midy --man
  midy [OPTIONS]

Options:
  -h, --help       Show this help message
      --man        Show the detailed manual
  -V, --version    Show version information

Commands:
  read     Print a deterministic ASCII timeline for a MIDI file
  apply    Apply ASCII edit commands and write a new MIDI file
  write    Alias for apply
  suggest  Print generated edit commands
  chords   Print detected chord slices
  roll     Print an ASCII piano-roll view
  analyze  Print a high-level MIDI analysis
  tracks   Print track/channel summaries
  lint     Print MIDI quality warnings
  fix      Repair common note-level MIDI problems
  humanize Randomize note starts/velocities reproducibly
  dehumanize Quantize note starts/durations
  swing    Delay off-grid subdivisions
  chordize Add chord tones above matching notes
  arpeggiate Turn simultaneous chord stacks into arpeggios
  extract  Write a MIDI containing only selected track/channel notes
  mute     Write a MIDI with selected track/channel notes removed
  solo     Alias-style operation: keep only selected track/channel notes
  split    Write one MIDI per source track or channel
  merge    Merge multiple MIDI files into one parallel-track MIDI
  diff     Print note-level edit commands from before to after
  render   Render MIDI to audio by shelling out to FluidSynth
  schema   Print the ASCII edit format accepted by apply/write
";

const MANUAL: &str = "\
MIDY MANUAL
===========

Name
----
midy - convert MIDI files into editable ASCII timelines, then apply ASCII edits
back into MIDI files.

Purpose
-------
midy is designed as a bridge between binary MIDI data and text-based tools.
The main idea is:

  1. Read a .mid file.
  2. Print a stable, line-oriented ASCII timeline.
  3. Send that text to a human, script, or language model.
  4. Let that text system describe changes as edit lines.
  5. Apply those edits and write a new .mid file.

The ASCII format is intentionally plain text. It is not meant to be pretty
sheet music. It is meant to be explicit, deterministic, grep-friendly, and
easy for another machine to understand and rewrite.

Quick start
-----------
Print a MIDI file as ASCII:

  midy read song.mid

Save that ASCII to a file:

  midy read song.mid > song.midy.txt

Create a separate edit file:

  ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96
  SET_NOTE id=t0n0 key=62 dur=240
  DELETE_NOTE id=t0n1
  TRANSPOSE semitones=2 track=0

Apply the edits and write a new MIDI file:

  midy apply song.mid edits.txt -o changed.mid

Or pipe edits through stdin and name only the input and output MIDI files:

  cat edits.txt | midy apply song.mid changed.mid

If you pipe edits and provide only the input MIDI file, midy overwrites that
input file after successfully parsing and rewriting the MIDI in memory:

  cat edits.txt | midy apply song.mid

The write command is an alias for apply:

  midy write song.mid edits.txt -o changed.mid

Commands
--------
midy --help
  Prints the short command summary.

midy --man
  Prints this detailed manual.

midy --version
  Prints the program version.

midy schema
  Prints the concise machine-readable edit grammar.

midy read <input.mid> [--format midy|json|csv]
  Parses a MIDI file and prints the timeline to stdout. The default format is
  midy, the stable line-oriented ASCII timeline. Use json for structured
  scripts, or csv for note rows that are easy to inspect in spreadsheets.

  Examples:

    midy read song.mid
    midy read song.mid --format json
    midy read song.mid --format csv

midy apply <input.mid> <edits.txt> -o <output.mid>
  Parses the original MIDI file, reads edits from edits.txt, applies them, and
  writes output.mid. If the edit file extension is .json or .csv, midy treats
  it as edited output from `midy read --format json|csv`: matching ids are
  updated, missing ids are deleted, and rows with blank/new ids are added.

midy apply <input.mid> <output.mid>
  Reads edit commands from stdin and writes the edited MIDI to output.mid.
  Example: cat edits.txt | midy apply input.mid output.mid

midy apply <input.mid>
  Reads edit commands from stdin and overwrites input.mid. The overwrite happens
  only after the input MIDI and edit text have both parsed successfully.

midy suggest reduce-chords <input.mid>
  Reads a MIDI file, detects simultaneous chord stacks, and prints DELETE_NOTE
  edit commands that reduce each stack to one note. This is useful when you want
  to turn block chords into a melody or bassline before piping into apply.

  Examples:

    midy suggest reduce-chords chords.mid --keep highest
    midy suggest reduce-chords chords.mid --keep lowest --track 1
    midy suggest reduce-chords chords.mid --keep root --window 8
    midy suggest reduce-chords chords.mid --keep nth=2

  Pipe example:

    midy suggest reduce-chords chords.mid --keep highest | midy apply chords.mid melody.mid

midy suggest bass <input.mid>
  Reads detected chord stacks and prints ADD_NOTE commands for simple bass notes
  on each chord root. Use --track/--ch/--bars to filter the source chords, and
  --out-track/--out-ch to choose where generated bass notes should be written.

  Examples:

    midy suggest bass chords.mid --out-track 2 --out-ch 0 --octave 2
    midy suggest bass chords.mid --bars 1..8 --dur 1/4 --vel 90
    midy suggest bass chords.mid | midy apply chords.mid with-bass.mid

midy chords <input.mid>
  Prints detected chord slices with tick, optional bar position, note names,
  MIDI keys, chord name, inversion, bass note, and confidence.

  Examples:

    midy chords song.mid
    midy chords song.mid --track 1
    midy chords song.mid --bars 1..8

midy roll <input.mid>
  Prints a compact ASCII piano roll. Use this when humans or LLMs need a visual
  representation of melody contour and chord stacks.

  Examples:

    midy roll song.mid
    midy roll song.mid --track 1 --grid 1/16
    midy roll song.mid --ch 0 --start 0 --end 1920
    midy roll song.mid --mode verbose --bars 1..2

midy analyze <input.mid>
  Prints a high-level analysis: header, tempo summary, time signature summary,
  estimated key center, track role guesses, ranges, densities, and detected
  chord progression.

  Use --json for a compact script-friendly JSON summary:

    midy analyze song.mid --json

midy tracks <input.mid>
  Prints one line per track with role guess, channel set, note count, range,
  average velocity, average duration, polyphony, density, and track names.

midy lint <input.mid>
  Prints MIDI quality warnings that are useful before asking an LLM to edit a
  file: duplicate notes, overlapping same-pitch notes, zero-duration notes,
  empty tracks, and stuck note-ons without matching note-offs.

  Example:

    midy lint song.mid

midy fix <input.mid> <output.mid>
  Writes a repaired MIDI. The current fixer removes exact duplicate notes,
  deletes zero-duration notes, trims overlapping same-pitch notes, and removes
  stuck note-on events that have no matching note-off. Use --stuck-mode close
  to add a missing note-off instead, with --stuck-duration N controlling the
  generated duration. Use --remove-empty-tracks to drop empty tracks after
  repair.

  Examples:

    midy fix song.mid fixed.mid
    midy fix song.mid fixed.mid --stuck-mode close --stuck-duration 240
    midy fix song.mid fixed.mid --remove-empty-tracks

midy humanize <input.mid> <output.mid> [--timing N] [--velocity N] [--seed N]
  Applies the HUMANIZE edit command and writes a new MIDI. timing randomizes
  starts by +/- N ticks; velocity randomizes note-on velocity by +/- N. The seed
  makes output reproducible.

  Examples:

    midy humanize song.mid human.mid --timing 12 --velocity 8 --seed 1
    midy humanize song.mid human.mid --track 1 --bars 1..4

midy dehumanize <input.mid> <output.mid> [--grid GRID] [--mode start|duration|both]
  Applies the DEHUMANIZE edit command. This quantizes matching note starts by
  default; mode=both also quantizes durations. GRID may be ticks or a metrical
  fraction such as 1/16.

  Examples:

    midy dehumanize song.mid tight.mid --grid 1/16
    midy dehumanize song.mid tight.mid --grid 120 --mode both

midy swing <input.mid> <output.mid> [--amount 50..100] [--grid GRID]
  Applies the SWING edit command. amount=50 leaves timing straight; larger
  values delay odd grid subdivisions. GRID may be ticks or a metrical fraction.

  Examples:

    midy swing song.mid swung.mid --amount 55 --grid 1/8
    midy swing song.mid swung.mid --amount 62 --grid 120 --track 1

midy chordize <input.mid> <output.mid> [--quality QUALITY]
  Applies the CHORDIZE edit command. This turns matching single-note material
  into block chords by adding chord tones above each selected note. Use a named
  quality such as maj, min, dim, sus4, maj7, min7, or dom7; or pass explicit
  semitone intervals with --intervals 0,4,7.

  Examples:

    midy chordize melody.mid chords.mid --quality maj
    midy chordize melody.mid chords.mid --quality min7 --track 1 --bars 1..8
    midy chordize melody.mid chords.mid --intervals 0,3,7,10

midy arpeggiate <input.mid> <output.mid> [--grid GRID] [--order up|down|updown]
  Applies the ARPEGGIATE edit command. This takes simultaneous chord stacks and
  offsets their notes onto a grid, making a simple arpeggio.

  Examples:

    midy arpeggiate chords.mid arp.mid --grid 1/16 --order up
    midy arpeggiate chords.mid arp.mid --grid 120 --order down --track 1

midy extract <input.mid> (--track N|--ch N) <output.mid>
  Writes a new MIDI that keeps only MIDI channel events matching the selected
  source track and/or MIDI channel. Non-MIDI and meta events are preserved so
  tempo maps, track names, and end-of-track markers survive the operation.

  Examples:

    midy extract song.mid --track 1 melody.mid
    midy extract song.mid --ch 9 drums.mid
    midy extract song.mid --track 2 --ch 0 track2-ch0.mid

midy mute <input.mid> (--track N|--ch N) <output.mid>
  Writes a new MIDI with selected MIDI channel events removed. Use this when
  you want the full arrangement minus one track/channel.

  Examples:

    midy mute song.mid --track 2 no-chords.mid
    midy mute song.mid --ch 9 no-drums.mid

midy solo <input.mid> (--track N|--ch N) <output.mid>
  Keeps only the selected MIDI channel events and preserves non-MIDI/meta
  events. This is intentionally similar to extract, but named for the common
  DAW workflow of soloing one part.

  Examples:

    midy solo song.mid --track 1 melody-only.mid
    midy solo song.mid --ch 0 channel-zero.mid

midy split <input.mid> --by track|channel --out-dir <dir>
  Writes multiple MIDI files into the output directory. Splitting by track
  creates files named track-N.mid for note-bearing tracks. Splitting by channel
  creates files named ch-N.mid for channels found in note events.

  Examples:

    midy split song.mid --by track --out-dir stems/
    midy split song.mid --by channel --out-dir channels/

midy merge <input-a.mid> <input-b.mid> [...input.mid] -o <output.mid>
  Merges two or more MIDI files into one parallel-track MIDI. All inputs must
  use the same MIDI timing division. Tracks are appended in input order and
  events inside each source track are preserved.

  Example:

    midy merge drums.mid bass.mid chords.mid -o arrangement.mid

midy diff <before.mid> <after.mid>
  Prints note-level edit commands that transform before.mid into after.mid.
  Unchanged notes are omitted; changed notes are represented as DELETE_NOTE
  plus ADD_NOTE so the output can be piped directly into midy apply.

  Examples:

    midy diff before.mid after.mid
    midy diff before.mid after.mid | midy apply before.mid recreated.mid

midy render <input.mid> <output.wav|output.flac> --soundfont <file.sf2>
  Renders MIDI to an audio file by shelling out to FluidSynth. Rendering is an
  optional preview workflow: midy itself remains a lightweight MIDI editor, and
  the external fluidsynth binary plus a SoundFont provide the audio engine.

  Options:

    --soundfont FILE
      Required .sf2/.sf3 SoundFont path.

    --synth PATH
      FluidSynth executable to run. Defaults to fluidsynth.

    --sample-rate N
      Audio sample rate. Defaults to 44100.

  Examples:

    midy render song.mid song.wav --soundfont piano.sf2
    midy render song.mid song.flac --soundfont gm.sf2 --sample-rate 48000

Timeline output
---------------
The first line identifies the format:

  MIDY_TIMELINE v1

The HEADER line describes the MIDI container:

  HEADER format=parallel timing=metrical ticks_per_beat=480 tracks=2

Important fields:

  format
    MIDI file layout: single, parallel, or sequential.

  timing=metrical ticks_per_beat=N
    The common musical timing mode. Tick positions in NOTE lines are integer
    MIDI ticks. If ticks_per_beat is 480, one quarter note is 480 ticks.

  tracks=N
    Number of MIDI tracks in the file.

The SONG line gives total length:

  SONG length_ticks=1920 length_beats=4.000

Track summaries look like:

  TRACK index=1 events=42 notes=12

Meta lines show important non-note MIDI events:

  META track=0 tick=0 kind=tempo us_per_quarter=500000 bpm=120.000
  META track=0 tick=0 kind=time_signature numerator=4 denominator=4 ...
  META track=0 tick=1920 kind=end_of_track

Program changes and system events are shown as EVENT lines:

  EVENT track=1 tick=0 kind=program_change ch=0 program=4

Notes are the main editable lines:

  NOTE id=t1n0 track=1 ch=0 key=60 name=C4 start=0 dur=480 end=480 pos=1:1:0 end_pos=1:2:0 vel=96 off_vel=64

NOTE fields
-----------
id=t1n0
  Stable note identifier for this read output. t1n0 means track 1, note 0.
  Use this id with SET_NOTE, DELETE_NOTE, or by editing the NOTE line itself.

track=1
  MIDI track index, zero-based.

ch=0
  MIDI channel, zero-based. Valid range is 0..15.

key=60
  MIDI note number. Valid range is 0..127. Middle C is commonly key 60. Edit
  commands also accept note names such as key=C4, key=F#3, or key=Bb2.

name=C4
  Human-friendly note name derived from key. This is informational; midy uses
  key=N as the authority.

start=0
  Start time in MIDI ticks.

dur=480
  Duration in MIDI ticks.

end=480
  End time in ticks. For edited NOTE lines, midy uses start and dur.

pos=1:1:0
  Human-friendly bar:beat:tick position when the MIDI file has metrical timing.
  This is informational in read output. In edit commands you may use pos= or
  at= as a start-time alias when the source MIDI uses metrical timing.

vel=96
  Note-on velocity. Valid range is 0..127.

off_vel=64
  Note-off velocity. Valid range is 0..127.

Editing workflow styles
-----------------------
You can edit MIDI in two ways.

1. Command file style:

  ADD_NOTE track=0 ch=0 key=64 start=480 dur=480 vel=90
  ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=1/4 vel=90
  SET_NOTE id=t0n0 key=62 dur=240
  SHIFT by=1/8 bars=1..4
  DELETE_NOTE id=t0n1

2. Modified timeline style:

  midy read song.mid > song.midy.txt

Then edit NOTE lines directly, for example changing:

  NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=0 dur=480 ...

to:

  NOTE id=t0n0 track=0 ch=0 key=62 name=D4 start=0 dur=240 ...

Then apply the edited timeline:

  midy apply song.mid song.midy.txt -o changed.mid

Or use a pipe instead of passing the timeline as an edit file:

  cat song.midy.txt | midy apply song.mid changed.mid

When applying a full timeline file, midy ignores read-only lines such as
HEADER, SONG, TRACK, META, and EVENT. Edited NOTE lines act like SET_NOTE.

Edit commands
-------------
ADD_NOTE
  Adds a new note.

  ADD_NOTE track=0 ch=0 key=60 start=0 dur=480 vel=96
  ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=1/4 vel=96

  Optional:

  off_vel=64

SET_NOTE
  Changes an existing note by id. Only fields you provide are changed.

  SET_NOTE id=t0n0 key=62
  SET_NOTE id=t0n0 key=F#4 dur=1/8
  SET_NOTE id=t0n0 start=240 dur=120 vel=80
  SET_NOTE id=t0n0 track=1 ch=2 key=72 off_vel=0

NOTE
  A NOTE line with an id is accepted as an editable SET_NOTE line. This is what
  allows modified read output to be applied directly.

DELETE_NOTE
  Deletes one note by id.

  DELETE_NOTE id=t0n0

DELETE_NOTES
  Deletes all matching notes. With no filter it deletes all notes, so use this
  carefully.

  DELETE_NOTES track=0
  DELETE_NOTES track=0 ch=0 key=60
  DELETE_NOTES start=960 end=1920
  DELETE_NOTES bars=1..4 key=C4

TRANSPOSE
  Moves matching note keys by semitones.

  TRANSPOSE semitones=2
  TRANSPOSE semitones=-12 track=1
  TRANSPOSE semitones=7 ch=0 start=0 end=1920

SHIFT
  Moves matching note start times by ticks or by a metrical musical duration.

  SHIFT ticks=120
  SHIFT ticks=-240 track=0 start=960 end=1920
  SHIFT by=1/8 bars=1..4
  SHIFT by=-beat bar=2

SCALE_TIME
  Scales matching note start times and durations. This stretches or compresses
  the timeline.

  SCALE_TIME factor=2/1
  SCALE_TIME factor=1/2
  SCALE_TIME factor=0.5

SCALE_DURATION
  Scales matching note durations without moving their starts.

  SCALE_DURATION factor=1/2
  SCALE_DURATION factor=2 key=60
  SCALE_DURATION factor=1/2 bars=1..8

QUANTIZE
  Snaps starts, durations, or both to a tick grid. Metrical MIDI files also
  accept fraction grids such as 1/16 and 1/8t.

  QUANTIZE grid=120
  QUANTIZE grid=1/16
  QUANTIZE grid=120 mode=start
  QUANTIZE grid=120 mode=duration
  QUANTIZE grid=120 mode=both

HUMANIZE
  Randomizes matching note starts and note-on velocities. Use seed for
  reproducible output.

  HUMANIZE timing=12 velocity=8 seed=1
  HUMANIZE timing=6 velocity=4 seed=7 track=1 bars=1..4

DEHUMANIZE
  Quantizes matching note starts by default. Use mode=both to quantize starts
  and durations.

  DEHUMANIZE grid=1/16
  DEHUMANIZE grid=120 mode=both

SWING
  Delays odd grid subdivisions. amount=50 is straight timing; values above 50
  add delayed swing.

  SWING amount=55 grid=1/8
  SWING amount=62 grid=120 track=1

VELOCITY
  Changes note-on velocities for matching notes.

  VELOCITY scale=0.8
  VELOCITY add=10 track=1
  VELOCITY set=96 key=C4
  VELOCITY compress=0.5 center=80

CRESCENDO
  Ramps note-on velocities across a selected tick range. With start/end
  filters, start_vel is used at start and end_vel is used near end.

  CRESCENDO start_vel=40 end_vel=110 start=0 end=1920
  CRESCENDO start_vel=90 end_vel=50 bars=5..8 track=1

CHORDIZE
  Adds chord tones above matching notes. Use quality for common chord shapes or
  intervals for exact semitone offsets from each source note. The original note
  is kept; interval 0 is accepted but does not create a duplicate note.

  CHORDIZE quality=maj
  CHORDIZE quality=min7 track=1 bars=1..4
  CHORDIZE intervals=0,4,7,12 key=C4

ARPEGGIATE
  Turns simultaneous chord stacks into an arpeggio by moving notes onto a grid.
  order may be up, down, or updown.

  ARPEGGIATE grid=1/16 order=up
  ARPEGGIATE grid=120 order=down track=1

BLOCK_CHORD
  Moves matching notes down to the start of their grid window. Use a grid large
  enough to collect the arpeggio notes you want to block together.

  BLOCK_CHORD grid=1/8
  BLOCK_CHORD grid=1/4 track=2

INVERT_CHORDS
  Inverts simultaneous chord stacks by moving low notes up octaves, or high
  notes down octaves for negative inversions.

  INVERT_CHORDS inversion=1
  INVERT_CHORDS inversion=-1 track=1

DOUBLE
  Adds octave-doubled copies of matching notes.

  DOUBLE octave=-1
  DOUBLE octave=1 key=C4

VOICE_LEAD
  Moves later chord notes by octaves toward the previous chord voices.

  VOICE_LEAD max_jump=7
  VOICE_LEAD max_jump=12 track=1

MUTE and SOLO
  Delete matching notes, or delete everything except matching notes. These are
  edit-command equivalents for common track/channel isolation workflows.

  MUTE track=2
  MUTE ch=9
  SOLO track=1
  SOLO track=1 ch=0

MOVE_TRACK
  Moves matching notes from one existing source track to another existing track.
  The from= track is required and can be combined with channel/key/range
  filters.

  MOVE_TRACK from=2 to=1
  MOVE_TRACK from=2 to=1 ch=0

SET_CHANNEL
  Changes matching notes to a new MIDI channel. ch= is the destination channel;
  use from_ch= when you also need to filter by the original channel.

  SET_CHANNEL track=1 ch=0
  SET_CHANNEL track=1 ch=0 from_ch=2

Filters
-------
Most whole-file commands accept optional filters:

  track=0
    Match only notes on track 0.

  ch=0 or channel=0
    Match only notes on MIDI channel 0.

  key=60
    Match only MIDI key 60. Edit commands also accept note names like key=C4.

  start=480
    Match notes whose start tick is >= 480.

  end=960
    Match notes whose start tick is < 960. The end filter is exclusive.

  bars=1..4
    Select a 1-based musical bar range using the MIDI time-signature map. In
    edit commands, bar=1 and bars=1..4 are aliases for start/end tick filters
    when the source MIDI uses metrical timing.

  at=2:1 or pos=2:1:0
    For ADD_NOTE and SET_NOTE, set start using 1-based bar:beat:tick notation.
    The allowed beat count follows the active time signature at that bar.

  dur=1/4, dur=beat, dur=bar
    For ADD_NOTE and SET_NOTE, set duration using musical values.

Examples:

  TRANSPOSE semitones=2 track=1 ch=0
  QUANTIZE grid=120 track=1 start=0 end=1920
  QUANTIZE grid=1/16 bars=1..4
  HUMANIZE timing=12 velocity=8 seed=1 track=1
  SWING amount=55 grid=1/8 bars=1..4
  ARPEGGIATE grid=1/16 order=up track=1
  DOUBLE octave=-1 key=C4
  DELETE_NOTES track=2 key=36
  SCALE_DURATION factor=1/2 ch=9
  VELOCITY add=8 track=1 bars=1..4
  CRESCENDO start_vel=40 end_vel=110 start=0 end=1920

Safety and limits
-----------------
midy currently edits note events. It preserves other MIDI and meta events while
rebuilding note-on and note-off event timing.

If you use the edit-file form, pass -o to choose the output path. If you pipe
edits through stdin, the second positional path is the output MIDI path. If you
pipe edits and omit the output path, midy overwrites the input file after a
successful parse/rewrite.

All times are integer MIDI ticks. midy does not guess musical intent beyond the
explicit commands you provide.

Validation rules:

  - track must exist
  - ch/channel must be 0..15
  - key, vel, and off_vel must be 0..127
  - dur must be greater than zero
  - negative SHIFT cannot move a note before tick 0
  - transpose cannot move keys outside 0..127
  - factor must be greater than zero
  - quantize grid must be greater than zero

Suggested machine workflow
--------------------------
For an ASCII-understanding machine, use this loop:

  1. Run: midy read input.mid
  2. Give the output plus the user's musical instruction to the machine.
  3. Ask the machine to output only valid midy edit lines.
  4. Run: cat edits.txt | midy apply input.mid output.mid
  5. Run: midy read output.mid to inspect the result.

For maximum reliability, tell the machine to prefer explicit edit commands
instead of prose. Good machine output:

  TRANSPOSE semitones=2 track=0
  QUANTIZE grid=120 mode=both
  SET_NOTE id=t0n4 dur=240
  VELOCITY add=8 track=0

Bad machine output:

  Make the melody brighter and tighten the timing.

That sentence is understandable to a human, but midy intentionally applies only
explicit line-based commands.
";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Help,
    Man,
    Version,
    Schema,
    Read {
        input: PathBuf,
        format: ReadFormat,
    },
    Apply {
        input: PathBuf,
        edits: EditInput,
        output: PathBuf,
    },
    SuggestReduceChords {
        input: PathBuf,
        options: midi::ReduceChordsOptions,
    },
    SuggestBass {
        input: PathBuf,
        options: midi::BasslineOptions,
    },
    Chords {
        input: PathBuf,
        options: midi::ChordsOptions,
    },
    Roll {
        input: PathBuf,
        options: midi::RollOptions,
    },
    Analyze {
        input: PathBuf,
        json: bool,
    },
    Tracks {
        input: PathBuf,
    },
    Lint {
        input: PathBuf,
    },
    Fix {
        input: PathBuf,
        output: PathBuf,
        options: midi::FixOptions,
    },
    Humanize {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Dehumanize {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Swing {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Chordize {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Arpeggiate {
        input: PathBuf,
        output: PathBuf,
        edit: String,
    },
    Extract {
        input: PathBuf,
        output: PathBuf,
        selector: midi::TrackChannelSelector,
    },
    Mute {
        input: PathBuf,
        output: PathBuf,
        selector: midi::TrackChannelSelector,
    },
    Solo {
        input: PathBuf,
        output: PathBuf,
        selector: midi::TrackChannelSelector,
    },
    Split {
        input: PathBuf,
        out_dir: PathBuf,
        mode: midi::SplitMode,
    },
    Merge {
        inputs: Vec<PathBuf>,
        output: PathBuf,
    },
    Diff {
        before: PathBuf,
        after: PathBuf,
    },
    Render {
        input: PathBuf,
        output: PathBuf,
        soundfont: PathBuf,
        synth: String,
        sample_rate: u32,
    },
}

#[derive(Debug, Eq, PartialEq)]
enum EditInput {
    File(PathBuf),
    Stdin,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ReadFormat {
    Midy,
    Json,
    Csv,
}

/// Runs the command-line interface.
pub fn run(args: Vec<String>) -> Result<()> {
    match parse(args)? {
        Command::Help => {
            print!("{HELP}");
            Ok(())
        }
        Command::Man => {
            print!("{MANUAL}");
            Ok(())
        }
        Command::Version => {
            println!("{} {}", name(), env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Schema => {
            print!("{}", midi::EDIT_FORMAT_HELP);
            Ok(())
        }
        Command::Read { input, format } => {
            let bytes = std::fs::read(&input)?;
            match format {
                ReadFormat::Midy => print!("{}", midi::render_timeline(&bytes)?),
                ReadFormat::Json => print!("{}", midi::render_timeline_json(&bytes)?),
                ReadFormat::Csv => print!("{}", midi::render_timeline_csv(&bytes)?),
            }
            Ok(())
        }
        Command::Apply {
            input,
            edits,
            output,
        } => {
            let bytes = std::fs::read(&input)?;
            let rewritten = apply_edit_input(&bytes, edits)?;
            std::fs::write(&output, rewritten)?;
            Ok(())
        }
        Command::SuggestReduceChords { input, options } => {
            let bytes = std::fs::read(&input)?;
            print!("{}", midi::suggest_reduce_chords(&bytes, &options)?);
            Ok(())
        }
        Command::SuggestBass { input, options } => {
            let bytes = std::fs::read(&input)?;
            print!("{}", midi::suggest_bassline(&bytes, &options)?);
            Ok(())
        }
        Command::Chords { input, options } => {
            let bytes = std::fs::read(&input)?;
            print!("{}", midi::render_chords(&bytes, &options)?);
            Ok(())
        }
        Command::Roll { input, options } => {
            let bytes = std::fs::read(&input)?;
            print!("{}", midi::render_roll(&bytes, &options)?);
            Ok(())
        }
        Command::Analyze { input, json } => {
            let bytes = std::fs::read(&input)?;
            if json {
                print!("{}", midi::render_analysis_json(&bytes, input.to_str())?);
            } else {
                print!("{}", midi::render_analysis(&bytes, input.to_str())?);
            }
            Ok(())
        }
        Command::Tracks { input } => {
            let bytes = std::fs::read(&input)?;
            print!("{}", midi::render_tracks(&bytes)?);
            Ok(())
        }
        Command::Lint { input } => {
            let bytes = std::fs::read(&input)?;
            print!("{}", midi::render_lint(&bytes)?);
            Ok(())
        }
        Command::Fix {
            input,
            output,
            options,
        } => {
            let bytes = std::fs::read(&input)?;
            let rewritten = midi::fix_midi_with_options(&bytes, options)?;
            std::fs::write(output, rewritten)?;
            Ok(())
        }
        Command::Humanize {
            input,
            output,
            edit,
        }
        | Command::Dehumanize {
            input,
            output,
            edit,
        }
        | Command::Swing {
            input,
            output,
            edit,
        }
        | Command::Chordize {
            input,
            output,
            edit,
        }
        | Command::Arpeggiate {
            input,
            output,
            edit,
        } => {
            let bytes = std::fs::read(&input)?;
            let rewritten = midi::apply_edits(&bytes, &edit)?;
            std::fs::write(output, rewritten)?;
            Ok(())
        }
        Command::Extract {
            input,
            output,
            selector,
        } => {
            let bytes = std::fs::read(&input)?;
            let rewritten = midi::extract_selection(&bytes, &selector)?;
            std::fs::write(output, rewritten)?;
            Ok(())
        }
        Command::Mute {
            input,
            output,
            selector,
        } => {
            let bytes = std::fs::read(&input)?;
            let rewritten = midi::mute_selection(&bytes, &selector)?;
            std::fs::write(output, rewritten)?;
            Ok(())
        }
        Command::Solo {
            input,
            output,
            selector,
        } => {
            let bytes = std::fs::read(&input)?;
            let rewritten = midi::solo_selection(&bytes, &selector)?;
            std::fs::write(output, rewritten)?;
            Ok(())
        }
        Command::Split {
            input,
            out_dir,
            mode,
        } => {
            let bytes = std::fs::read(&input)?;
            std::fs::create_dir_all(&out_dir)?;
            for (name, output) in midi::split_selection(&bytes, mode)? {
                std::fs::write(out_dir.join(name), output)?;
            }
            Ok(())
        }
        Command::Merge { inputs, output } => {
            let bytes = inputs
                .iter()
                .map(std::fs::read)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let input_refs = bytes.iter().map(Vec::as_slice).collect::<Vec<_>>();
            let rewritten = midi::merge_midi(&input_refs)?;
            std::fs::write(output, rewritten)?;
            Ok(())
        }
        Command::Diff { before, after } => {
            let before = std::fs::read(before)?;
            let after = std::fs::read(after)?;
            print!("{}", midi::render_diff(&before, &after)?);
            Ok(())
        }
        Command::Render {
            input,
            output,
            soundfont,
            synth,
            sample_rate,
        } => render_audio(&input, &output, &soundfont, &synth, sample_rate),
    }
}

fn parse(args: Vec<String>) -> Result<Command> {
    match args.as_slice() {
        [_program] => Ok(Command::Help),
        [_program, flag] if flag == "-h" || flag == "--help" => Ok(Command::Help),
        [_program, flag] if flag == "--man" || flag == "man" => Ok(Command::Man),
        [_program, flag] if flag == "-V" || flag == "--version" => Ok(Command::Version),
        [_program, command] if command == "schema" || command == "format" => Ok(Command::Schema),
        [_program, command, rest @ ..] if command == "read" || command == "dump" => {
            parse_read(rest)
        }
        [_program, command, rest @ ..] if command == "apply" || command == "write" => {
            parse_apply(rest)
        }
        [_program, command, rest @ ..] if command == "suggest" => parse_suggest(rest),
        [_program, command, rest @ ..] if command == "chords" => parse_chords(rest),
        [_program, command, rest @ ..] if command == "roll" => parse_roll(rest),
        [_program, command, rest @ ..] if command == "analyze" => parse_analyze(rest),
        [_program, command, input] if command == "tracks" => Ok(Command::Tracks {
            input: input.into(),
        }),
        [_program, command, input] if command == "lint" => Ok(Command::Lint {
            input: input.into(),
        }),
        [_program, command, rest @ ..] if command == "fix" => parse_fix(rest),
        [_program, command, rest @ ..] if command == "humanize" || command == "humanise" => {
            parse_humanize(rest)
        }
        [_program, command, rest @ ..] if command == "dehumanize" || command == "dehumanise" => {
            parse_dehumanize(rest)
        }
        [_program, command, rest @ ..] if command == "swing" => parse_swing(rest),
        [_program, command, rest @ ..] if command == "chordize" || command == "chordise" => {
            parse_chordize(rest)
        }
        [_program, command, rest @ ..] if command == "arpeggiate" || command == "arp" => {
            parse_arpeggiate(rest)
        }
        [_program, command, rest @ ..] if command == "extract" => parse_transform(rest, "extract"),
        [_program, command, rest @ ..] if command == "mute" => parse_transform(rest, "mute"),
        [_program, command, rest @ ..] if command == "solo" => parse_transform(rest, "solo"),
        [_program, command, rest @ ..] if command == "split" => parse_split(rest),
        [_program, command, rest @ ..] if command == "merge" => parse_merge(rest),
        [_program, command, before, after] if command == "diff" => Ok(Command::Diff {
            before: before.into(),
            after: after.into(),
        }),
        [_program, command, rest @ ..] if command == "render" => parse_render(rest),
        [_program, unknown, ..] => Err(Error::Usage(format!(
            "unknown argument '{unknown}'. Use 'midy --help' for usage."
        ))),
        [] => Ok(Command::Help),
    }
}

#[derive(Debug)]
struct ParsedOptions {
    positional: Vec<String>,
    values: std::collections::HashMap<String, String>,
}

fn parse_read(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "read expects <input.mid> with optional --format midy|json|csv".to_owned(),
        ));
    }
    let format = match parsed.value("format").unwrap_or("midy") {
        "midy" | "ascii" | "text" => ReadFormat::Midy,
        "json" => ReadFormat::Json,
        "csv" => ReadFormat::Csv,
        value => {
            return Err(Error::Usage(format!(
                "--format must be midy, json, or csv, got '{value}'"
            )));
        }
    };
    Ok(Command::Read {
        input: parsed.positional[0].clone().into(),
        format,
    })
}

fn parse_suggest(rest: &[String]) -> Result<Command> {
    let Some(kind) = rest.first() else {
        return Err(Error::Usage(
            "suggest expects a kind, for example: suggest reduce-chords <input.mid>".to_owned(),
        ));
    };
    match kind.as_str() {
        "reduce-chords" | "reduce_chords" => {
            let parsed = parse_options(&rest[1..])?;
            if parsed.positional.len() != 1 {
                return Err(Error::Usage(
                    "suggest reduce-chords expects <input.mid>".to_owned(),
                ));
            }
            Ok(Command::SuggestReduceChords {
                input: parsed.positional[0].clone().into(),
                options: midi::ReduceChordsOptions {
                    keep: parse_keep(parsed.value("keep").unwrap_or("highest"))?,
                    query: parse_query_options(&parsed)?,
                    window: parse_optional_u64(parsed.value("window"), "window")?.unwrap_or(0),
                },
            })
        }
        "bass" | "bassline" | "bass-line" => {
            let parsed = parse_options(&rest[1..])?;
            if parsed.positional.len() != 1 {
                return Err(Error::Usage("suggest bass expects <input.mid>".to_owned()));
            }
            Ok(Command::SuggestBass {
                input: parsed.positional[0].clone().into(),
                options: midi::BasslineOptions {
                    query: parse_query_options(&parsed)?,
                    window: parse_optional_u64(parsed.value("window"), "window")?.unwrap_or(0),
                    output_track: parse_optional_usize(
                        parsed
                            .value("out-track")
                            .or_else(|| parsed.value("out_track")),
                        "out-track",
                    )?
                    .unwrap_or(0),
                    output_channel: parse_optional_u8(
                        parsed
                            .value("out-ch")
                            .or_else(|| parsed.value("out-channel"))
                            .or_else(|| parsed.value("out_ch"))
                            .or_else(|| parsed.value("out_channel")),
                        "out-ch",
                        15,
                    )?
                    .unwrap_or(0),
                    octave: parse_optional_i16(parsed.value("octave"), "octave")?.unwrap_or(2),
                    velocity: parse_optional_u8(
                        parsed.value("vel").or_else(|| parsed.value("velocity")),
                        "vel",
                        127,
                    )?
                    .unwrap_or(80),
                    duration: parsed
                        .value("dur")
                        .or_else(|| parsed.value("duration"))
                        .map(ToOwned::to_owned),
                },
            })
        }
        unknown => Err(Error::Usage(format!(
            "unknown suggest kind '{unknown}'. Try 'suggest reduce-chords' or 'suggest bass'."
        ))),
    }
}

fn parse_chords(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "chords expects <input.mid> with optional --track/--ch/--start/--end/--bars/--window"
                .to_owned(),
        ));
    }
    Ok(Command::Chords {
        input: parsed.positional[0].clone().into(),
        options: midi::ChordsOptions {
            query: parse_query_options(&parsed)?,
            window: parse_optional_u64(parsed.value("window"), "window")?.unwrap_or(0),
        },
    })
}

fn parse_roll(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "roll expects <input.mid> with optional --track/--ch/--start/--end/--bars/--grid"
                .to_owned(),
        ));
    }
    Ok(Command::Roll {
        input: parsed.positional[0].clone().into(),
        options: midi::RollOptions {
            query: parse_query_options(&parsed)?,
            grid: parsed.value("grid").map(ToOwned::to_owned),
            mode: parse_roll_mode(parsed.value("mode").unwrap_or("compact"))?,
        },
    })
}

fn parse_roll_mode(value: &str) -> Result<midi::RollMode> {
    match value {
        "compact" => Ok(midi::RollMode::Compact),
        "verbose" => Ok(midi::RollMode::Verbose),
        value => Err(Error::Usage(format!(
            "--mode must be compact or verbose, got '{value}'"
        ))),
    }
}

fn parse_analyze(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "analyze expects <input.mid> with optional --json".to_owned(),
        ));
    }
    Ok(Command::Analyze {
        input: parsed.positional[0].clone().into(),
        json: parsed.value("json").is_some(),
    })
}

fn parse_fix(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "fix expects <input.mid> <output.mid> [--stuck-mode remove|close] [--remove-empty-tracks]"
                .to_owned(),
        ));
    }
    let stuck_note_mode = match parsed.value("stuck-mode").unwrap_or("remove") {
        "remove" => midi::StuckNoteFixMode::Remove,
        "close" => midi::StuckNoteFixMode::Close,
        value => {
            return Err(Error::Usage(format!(
                "--stuck-mode must be remove or close, got '{value}'"
            )));
        }
    };
    Ok(Command::Fix {
        input: parsed.positional[0].clone().into(),
        output: parsed.positional[1].clone().into(),
        options: midi::FixOptions {
            stuck_note_mode,
            stuck_note_duration: parse_optional_u64(
                parsed.value("stuck-duration"),
                "stuck-duration",
            )?
            .unwrap_or(120),
            remove_empty_tracks: parsed.value("remove-empty-tracks").is_some(),
        },
    })
}

fn parse_humanize(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "humanize expects <input.mid> <output.mid> [--timing N] [--velocity N] [--seed N]"
                .to_owned(),
        ));
    }
    let mut edit = format!(
        "HUMANIZE timing={} velocity={} seed={}",
        parsed.value("timing").unwrap_or("12"),
        parsed
            .value("velocity")
            .or_else(|| parsed.value("vel"))
            .unwrap_or("8"),
        parsed.value("seed").unwrap_or("1"),
    );
    append_edit_filters(&mut edit, &parsed);
    edit.push('\n');
    Ok(Command::Humanize {
        input: parsed.positional[0].clone().into(),
        output: parsed.positional[1].clone().into(),
        edit,
    })
}

fn parse_dehumanize(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "dehumanize expects <input.mid> <output.mid> [--grid GRID] [--mode start|duration|both]"
                .to_owned(),
        ));
    }
    let mut edit = format!(
        "DEHUMANIZE grid={} mode={}",
        parsed.value("grid").unwrap_or("1/16"),
        parsed.value("mode").unwrap_or("start"),
    );
    append_edit_filters(&mut edit, &parsed);
    edit.push('\n');
    Ok(Command::Dehumanize {
        input: parsed.positional[0].clone().into(),
        output: parsed.positional[1].clone().into(),
        edit,
    })
}

fn parse_swing(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "swing expects <input.mid> <output.mid> [--amount 50..100] [--grid GRID]".to_owned(),
        ));
    }
    let mut edit = format!(
        "SWING amount={} grid={}",
        parsed.value("amount").unwrap_or("55"),
        parsed.value("grid").unwrap_or("1/8"),
    );
    append_edit_filters(&mut edit, &parsed);
    edit.push('\n');
    Ok(Command::Swing {
        input: parsed.positional[0].clone().into(),
        output: parsed.positional[1].clone().into(),
        edit,
    })
}

fn parse_chordize(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "chordize expects <input.mid> <output.mid> [--quality QUALITY|--intervals 0,4,7]"
                .to_owned(),
        ));
    }
    let mut edit = if let Some(intervals) = parsed.value("intervals") {
        format!("CHORDIZE intervals={intervals}")
    } else {
        format!(
            "CHORDIZE quality={}",
            parsed.value("quality").unwrap_or("maj")
        )
    };
    append_edit_filters(&mut edit, &parsed);
    edit.push('\n');
    Ok(Command::Chordize {
        input: parsed.positional[0].clone().into(),
        output: parsed.positional[1].clone().into(),
        edit,
    })
}

fn parse_arpeggiate(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "arpeggiate expects <input.mid> <output.mid> [--grid GRID] [--order up|down|updown]"
                .to_owned(),
        ));
    }
    let mut edit = format!(
        "ARPEGGIATE grid={} order={}",
        parsed.value("grid").unwrap_or("1/16"),
        parsed.value("order").unwrap_or("up"),
    );
    append_edit_filters(&mut edit, &parsed);
    edit.push('\n');
    Ok(Command::Arpeggiate {
        input: parsed.positional[0].clone().into(),
        output: parsed.positional[1].clone().into(),
        edit,
    })
}

fn append_edit_filters(edit: &mut String, parsed: &ParsedOptions) {
    for key in [
        "track", "ch", "channel", "key", "start", "end", "bar", "bars",
    ] {
        if let Some(value) = parsed.value(key) {
            edit.push(' ');
            edit.push_str(key);
            edit.push('=');
            edit.push_str(value);
        }
    }
}

fn parse_transform(rest: &[String], command: &str) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(format!(
            "{command} expects <input.mid> (--track N|--ch N) <output.mid>"
        )));
    }
    let selector = parse_track_channel_selector(&parsed)?;
    let input = parsed.positional[0].clone().into();
    let output = parsed.positional[1].clone().into();
    match command {
        "extract" => Ok(Command::Extract {
            input,
            output,
            selector,
        }),
        "mute" => Ok(Command::Mute {
            input,
            output,
            selector,
        }),
        "solo" => Ok(Command::Solo {
            input,
            output,
            selector,
        }),
        _ => unreachable!("transform parser only receives known commands"),
    }
}

fn parse_split(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 1 {
        return Err(Error::Usage(
            "split expects <input.mid> --by track|channel --out-dir <dir>".to_owned(),
        ));
    }
    let mode = match parsed.value("by").unwrap_or("track") {
        "track" | "tracks" => midi::SplitMode::Track,
        "channel" | "channels" | "ch" => midi::SplitMode::Channel,
        value => {
            return Err(Error::Usage(format!(
                "--by must be track or channel, got '{value}'"
            )));
        }
    };
    let out_dir = parsed
        .value("out-dir")
        .or_else(|| parsed.value("out_dir"))
        .ok_or_else(|| Error::Usage("split requires --out-dir <dir>".to_owned()))?;

    Ok(Command::Split {
        input: parsed.positional[0].clone().into(),
        out_dir: out_dir.into(),
        mode,
    })
}

fn parse_merge(rest: &[String]) -> Result<Command> {
    let mut output = None::<PathBuf>;
    let mut inputs = Vec::<PathBuf>::new();
    let mut index = 0;

    while index < rest.len() {
        match rest[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(path) = rest.get(index) else {
                    return Err(Error::Usage(
                        "missing output path after -o/--output".to_owned(),
                    ));
                };
                if output.replace(path.into()).is_some() {
                    return Err(Error::Usage("duplicate merge output path".to_owned()));
                }
            }
            flag if flag.starts_with('-') => {
                return Err(Error::Usage(format!(
                    "unknown merge option '{flag}'. Use 'midy --help' for usage."
                )));
            }
            value => inputs.push(value.into()),
        }
        index += 1;
    }

    if inputs.len() < 2 {
        return Err(Error::Usage(
            "merge expects at least two input MIDI files".to_owned(),
        ));
    }
    let output = output.ok_or_else(|| Error::Usage("merge requires -o <output.mid>".to_owned()))?;
    Ok(Command::Merge { inputs, output })
}

fn parse_render(rest: &[String]) -> Result<Command> {
    let parsed = parse_options(rest)?;
    if parsed.positional.len() != 2 {
        return Err(Error::Usage(
            "render expects <input.mid> <output.wav|output.flac> --soundfont <file.sf2>".to_owned(),
        ));
    }
    let soundfont = parsed
        .value("soundfont")
        .or_else(|| parsed.value("sf2"))
        .ok_or_else(|| Error::Usage("render requires --soundfont <file.sf2>".to_owned()))?;
    let sample_rate =
        parse_optional_u32(parsed.value("sample-rate"), "sample-rate")?.unwrap_or(44_100);
    if sample_rate == 0 {
        return Err(Error::Usage(
            "--sample-rate must be greater than zero".to_owned(),
        ));
    }
    Ok(Command::Render {
        input: parsed.positional[0].clone().into(),
        output: parsed.positional[1].clone().into(),
        soundfont: soundfont.into(),
        synth: parsed.value("synth").unwrap_or("fluidsynth").to_owned(),
        sample_rate,
    })
}

fn render_audio(
    input: &std::path::Path,
    output: &std::path::Path,
    soundfont: &std::path::Path,
    synth: &str,
    sample_rate: u32,
) -> Result<()> {
    let file_type = audio_file_type(output)?;
    let status = std::process::Command::new(synth)
        .arg("-ni")
        .arg("-F")
        .arg(output)
        .arg("-T")
        .arg(file_type)
        .arg("-r")
        .arg(sample_rate.to_string())
        .arg(soundfont)
        .arg(input)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Usage(format!(
            "{synth} failed while rendering '{}' to '{}' with status {status}",
            input.display(),
            output.display(),
        )))
    }
}

fn audio_file_type(output: &std::path::Path) -> Result<&'static str> {
    match output
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav") => Ok("wav"),
        Some("flac") => Ok("flac"),
        Some("oga") | Some("ogg") => Ok("oga"),
        Some("aiff") | Some("aif") => Ok("aiff"),
        Some(value) => Err(Error::Usage(format!(
            "unsupported render output extension '.{value}'; use .wav, .flac, .oga, or .aiff"
        ))),
        None => Err(Error::Usage(
            "render output needs an audio extension such as .wav or .flac".to_owned(),
        )),
    }
}

fn parse_options(rest: &[String]) -> Result<ParsedOptions> {
    let mut positional = Vec::new();
    let mut values = std::collections::HashMap::new();
    let mut index = 0;

    while index < rest.len() {
        let arg = &rest[index];
        if let Some(raw) = arg.strip_prefix("--") {
            let (key, value) = if let Some((key, value)) = raw.split_once('=') {
                (key.to_owned(), value.to_owned())
            } else if rest
                .get(index + 1)
                .is_some_and(|next| !next.starts_with("--"))
            {
                index += 1;
                (raw.to_owned(), rest[index].clone())
            } else {
                (raw.to_owned(), "true".to_owned())
            };
            if values.insert(key.clone(), value).is_some() {
                return Err(Error::Usage(format!("duplicate option '--{key}'")));
            }
        } else {
            positional.push(arg.clone());
        }
        index += 1;
    }

    Ok(ParsedOptions { positional, values })
}

impl ParsedOptions {
    fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

fn parse_query_options(parsed: &ParsedOptions) -> Result<midi::QueryOptions> {
    Ok(midi::QueryOptions {
        track: parse_optional_usize(parsed.value("track"), "track")?,
        channel: parse_optional_u8(
            parsed.value("ch").or_else(|| parsed.value("channel")),
            "ch",
            15,
        )?,
        key: parse_optional_u8(parsed.value("key"), "key", 127)?,
        start: parse_optional_u64(parsed.value("start"), "start")?,
        end: parse_optional_u64(parsed.value("end"), "end")?,
        bars: parsed.value("bars").map(ToOwned::to_owned),
    })
}

fn parse_track_channel_selector(parsed: &ParsedOptions) -> Result<midi::TrackChannelSelector> {
    let selector = midi::TrackChannelSelector {
        track: parse_optional_usize(parsed.value("track"), "track")?,
        channel: parse_optional_u8(
            parsed.value("ch").or_else(|| parsed.value("channel")),
            "ch",
            15,
        )?,
    };
    if selector.track.is_none() && selector.channel.is_none() {
        Err(Error::Usage(
            "select at least one of --track or --ch/--channel".to_owned(),
        ))
    } else {
        Ok(selector)
    }
}

fn parse_keep(value: &str) -> Result<midi::ChordKeep> {
    match value {
        "highest" | "top" => Ok(midi::ChordKeep::Highest),
        "lowest" | "bottom" => Ok(midi::ChordKeep::Lowest),
        "root" => Ok(midi::ChordKeep::Root),
        value if value.starts_with("nth=") => {
            let nth = value
                .trim_start_matches("nth=")
                .parse::<usize>()
                .map_err(|_| Error::Usage(format!("invalid keep value '{value}'")))?;
            if nth == 0 {
                Err(Error::Usage("nth keep value is 1-based".to_owned()))
            } else {
                Ok(midi::ChordKeep::Nth(nth))
            }
        }
        value => Err(Error::Usage(format!(
            "invalid keep value '{value}', expected highest, lowest, root, or nth=N"
        ))),
    }
}

fn parse_optional_usize(value: Option<&str>, name: &str) -> Result<Option<usize>> {
    value
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| Error::Usage(format!("--{name} must be an integer")))
        })
        .transpose()
}

fn parse_optional_u64(value: Option<&str>, name: &str) -> Result<Option<u64>> {
    value
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| Error::Usage(format!("--{name} must be an integer")))
        })
        .transpose()
}

fn parse_optional_u32(value: Option<&str>, name: &str) -> Result<Option<u32>> {
    value
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| Error::Usage(format!("--{name} must be an integer")))
        })
        .transpose()
}

fn parse_optional_i16(value: Option<&str>, name: &str) -> Result<Option<i16>> {
    value
        .map(|value| {
            value
                .parse::<i16>()
                .map_err(|_| Error::Usage(format!("--{name} must be an integer")))
        })
        .transpose()
}

fn parse_optional_u8(value: Option<&str>, name: &str, max: u8) -> Result<Option<u8>> {
    value
        .map(|value| {
            let parsed = value
                .parse::<u8>()
                .map_err(|_| Error::Usage(format!("--{name} must be an integer")))?;
            if parsed <= max {
                Ok(parsed)
            } else {
                Err(Error::Usage(format!("--{name} must be <= {max}")))
            }
        })
        .transpose()
}

fn parse_apply(rest: &[String]) -> Result<Command> {
    let mut output = None::<PathBuf>;
    let mut positional = Vec::<PathBuf>::new();
    let mut index = 0;

    while index < rest.len() {
        match rest[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                let Some(path) = rest.get(index) else {
                    return Err(Error::Usage(
                        "missing output path after -o/--output".to_owned(),
                    ));
                };
                output = Some(path.into());
            }
            flag if flag != "-" && flag.starts_with('-') => {
                return Err(Error::Usage(format!(
                    "unknown apply option '{flag}'. Use 'midy --help' for usage."
                )));
            }
            value => positional.push(value.into()),
        }
        index += 1;
    }

    match (output, positional.as_slice()) {
        (Some(output), [input]) => Ok(Command::Apply {
            input: input.clone(),
            edits: EditInput::Stdin,
            output,
        }),
        (Some(output), [input, edits]) => Ok(Command::Apply {
            input: input.clone(),
            edits: parse_edit_input(edits),
            output,
        }),
        (Some(_), []) => Err(Error::Usage(
            "apply expects <input.mid> when reading edits from stdin, or <input.mid> <edits.txt> -o <output.mid>".to_owned(),
        )),
        (Some(_), _) => Err(Error::Usage(
            "too many apply arguments; use <input.mid> <edits.txt> -o <output.mid> or pipe edits into <input.mid> [output.mid]".to_owned(),
        )),
        (None, [input]) => Ok(Command::Apply {
            input: input.clone(),
            edits: EditInput::Stdin,
            output: input.clone(),
        }),
        (None, [input, output]) => Ok(Command::Apply {
            input: input.clone(),
            edits: EditInput::Stdin,
            output: output.clone(),
        }),
        (None, []) => Err(Error::Usage(
            "apply expects <input.mid>; pipe edits on stdin or use <input.mid> <edits.txt> -o <output.mid>".to_owned(),
        )),
        (None, _) => Err(Error::Usage(
            "too many apply arguments without -o; pipe edits into <input.mid> [output.mid]".to_owned(),
        )),
    }
}

fn parse_edit_input(path: &PathBuf) -> EditInput {
    if path == std::path::Path::new("-") {
        EditInput::Stdin
    } else {
        EditInput::File(path.clone())
    }
}

fn read_edit_text(input: EditInput) -> Result<String> {
    match input {
        EditInput::File(path) => Ok(std::fs::read_to_string(path)?),
        EditInput::Stdin => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            Ok(text)
        }
    }
}

fn apply_edit_input(bytes: &[u8], input: EditInput) -> Result<Vec<u8>> {
    match input {
        EditInput::File(path) => {
            let text = std::fs::read_to_string(&path)?;
            match path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("json") => midi::apply_timeline_json_edits(bytes, &text),
                Some("csv") => midi::apply_timeline_csv_edits(bytes, &text),
                _ => midi::apply_edits(bytes, &text),
            }
        }
        EditInput::Stdin => {
            let text = read_edit_text(EditInput::Stdin)?;
            midi::apply_edits(bytes, &text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn defaults_to_help() {
        assert_eq!(parse(args(&["midy"])).unwrap(), Command::Help);
    }

    #[test]
    fn parses_help() {
        assert_eq!(parse(args(&["midy", "--help"])).unwrap(), Command::Help);
        assert_eq!(parse(args(&["midy", "-h"])).unwrap(), Command::Help);
    }

    #[test]
    fn parses_man() {
        assert_eq!(parse(args(&["midy", "--man"])).unwrap(), Command::Man);
        assert_eq!(parse(args(&["midy", "man"])).unwrap(), Command::Man);
    }

    #[test]
    fn parses_version() {
        assert_eq!(
            parse(args(&["midy", "--version"])).unwrap(),
            Command::Version
        );
        assert_eq!(parse(args(&["midy", "-V"])).unwrap(), Command::Version);
    }

    #[test]
    fn parses_read() {
        assert_eq!(
            parse(args(&["midy", "read", "song.mid"])).unwrap(),
            Command::Read {
                input: "song.mid".into(),
                format: ReadFormat::Midy,
            }
        );
        assert_eq!(
            parse(args(&["midy", "read", "song.mid", "--format", "json"])).unwrap(),
            Command::Read {
                input: "song.mid".into(),
                format: ReadFormat::Json,
            }
        );
        assert_eq!(
            parse(args(&["midy", "read", "--format=csv", "song.mid"])).unwrap(),
            Command::Read {
                input: "song.mid".into(),
                format: ReadFormat::Csv,
            }
        );
    }

    #[test]
    fn parses_schema() {
        assert_eq!(parse(args(&["midy", "schema"])).unwrap(), Command::Schema);
        assert_eq!(parse(args(&["midy", "format"])).unwrap(), Command::Schema);
    }

    #[test]
    fn parses_apply() {
        assert_eq!(
            parse(args(&[
                "midy",
                "apply",
                "song.mid",
                "edits.txt",
                "-o",
                "out.mid"
            ]))
            .unwrap(),
            Command::Apply {
                input: "song.mid".into(),
                edits: EditInput::File("edits.txt".into()),
                output: "out.mid".into(),
            }
        );
    }

    #[test]
    fn parses_apply_with_stdin_and_output_path() {
        assert_eq!(
            parse(args(&["midy", "apply", "song.mid", "out.mid"])).unwrap(),
            Command::Apply {
                input: "song.mid".into(),
                edits: EditInput::Stdin,
                output: "out.mid".into(),
            }
        );
    }

    #[test]
    fn parses_apply_with_stdin_and_overwrite() {
        assert_eq!(
            parse(args(&["midy", "apply", "song.mid"])).unwrap(),
            Command::Apply {
                input: "song.mid".into(),
                edits: EditInput::Stdin,
                output: "song.mid".into(),
            }
        );
    }

    #[test]
    fn parses_apply_with_stdin_and_dash_edit_file() {
        assert_eq!(
            parse(args(&["midy", "apply", "song.mid", "-", "-o", "out.mid"])).unwrap(),
            Command::Apply {
                input: "song.mid".into(),
                edits: EditInput::Stdin,
                output: "out.mid".into(),
            }
        );
    }

    #[test]
    fn parses_suggest_reduce_chords() {
        assert_eq!(
            parse(args(&[
                "midy",
                "suggest",
                "reduce-chords",
                "song.mid",
                "--keep",
                "lowest",
                "--track=1",
                "--window",
                "8",
            ]))
            .unwrap(),
            Command::SuggestReduceChords {
                input: "song.mid".into(),
                options: midi::ReduceChordsOptions {
                    keep: midi::ChordKeep::Lowest,
                    query: midi::QueryOptions {
                        track: Some(1),
                        ..midi::QueryOptions::default()
                    },
                    window: 8,
                },
            }
        );
    }

    #[test]
    fn parses_suggest_bass() {
        assert_eq!(
            parse(args(&[
                "midy",
                "suggest",
                "bass",
                "song.mid",
                "--track",
                "1",
                "--out-track",
                "2",
                "--out-ch",
                "0",
                "--octave",
                "2",
                "--vel",
                "90",
                "--dur",
                "1/4",
            ]))
            .unwrap(),
            Command::SuggestBass {
                input: "song.mid".into(),
                options: midi::BasslineOptions {
                    query: midi::QueryOptions {
                        track: Some(1),
                        ..midi::QueryOptions::default()
                    },
                    output_track: 2,
                    output_channel: 0,
                    octave: 2,
                    velocity: 90,
                    duration: Some("1/4".to_owned()),
                    ..midi::BasslineOptions::default()
                },
            }
        );
    }

    #[test]
    fn parses_chords_roll_and_analyze() {
        assert!(matches!(
            parse(args(&["midy", "chords", "song.mid", "--bars", "1..4"])).unwrap(),
            Command::Chords { .. }
        ));
        assert!(matches!(
            parse(args(&["midy", "roll", "song.mid", "--grid", "1/16"])).unwrap(),
            Command::Roll { .. }
        ));
        assert_eq!(
            parse(args(&["midy", "roll", "song.mid", "--mode", "verbose"])).unwrap(),
            Command::Roll {
                input: "song.mid".into(),
                options: midi::RollOptions {
                    mode: midi::RollMode::Verbose,
                    ..midi::RollOptions::default()
                },
            }
        );
        assert_eq!(
            parse(args(&["midy", "analyze", "song.mid"])).unwrap(),
            Command::Analyze {
                input: "song.mid".into(),
                json: false,
            }
        );
        assert_eq!(
            parse(args(&["midy", "analyze", "song.mid", "--json"])).unwrap(),
            Command::Analyze {
                input: "song.mid".into(),
                json: true,
            }
        );
        assert_eq!(
            parse(args(&["midy", "tracks", "song.mid"])).unwrap(),
            Command::Tracks {
                input: "song.mid".into()
            }
        );
        assert_eq!(
            parse(args(&["midy", "lint", "song.mid"])).unwrap(),
            Command::Lint {
                input: "song.mid".into()
            }
        );
        assert_eq!(
            parse(args(&["midy", "fix", "song.mid", "fixed.mid"])).unwrap(),
            Command::Fix {
                input: "song.mid".into(),
                output: "fixed.mid".into(),
                options: midi::FixOptions::default(),
            }
        );
        assert_eq!(
            parse(args(&[
                "midy",
                "fix",
                "song.mid",
                "fixed.mid",
                "--stuck-mode",
                "close",
                "--stuck-duration",
                "240",
                "--remove-empty-tracks",
            ]))
            .unwrap(),
            Command::Fix {
                input: "song.mid".into(),
                output: "fixed.mid".into(),
                options: midi::FixOptions {
                    stuck_note_mode: midi::StuckNoteFixMode::Close,
                    stuck_note_duration: 240,
                    remove_empty_tracks: true,
                },
            }
        );
    }

    #[test]
    fn parses_humanize_dehumanize_and_swing() {
        assert_eq!(
            parse(args(&[
                "midy",
                "humanize",
                "song.mid",
                "human.mid",
                "--timing",
                "12",
                "--velocity",
                "8",
                "--seed",
                "2",
                "--track",
                "1",
            ]))
            .unwrap(),
            Command::Humanize {
                input: "song.mid".into(),
                output: "human.mid".into(),
                edit: "HUMANIZE timing=12 velocity=8 seed=2 track=1\n".to_owned(),
            }
        );
        assert_eq!(
            parse(args(&[
                "midy",
                "dehumanize",
                "song.mid",
                "tight.mid",
                "--grid",
                "1/16",
                "--mode",
                "both",
            ]))
            .unwrap(),
            Command::Dehumanize {
                input: "song.mid".into(),
                output: "tight.mid".into(),
                edit: "DEHUMANIZE grid=1/16 mode=both\n".to_owned(),
            }
        );
        assert_eq!(
            parse(args(&[
                "midy",
                "swing",
                "song.mid",
                "swung.mid",
                "--amount",
                "55",
                "--grid",
                "1/8",
                "--bars",
                "1..4",
            ]))
            .unwrap(),
            Command::Swing {
                input: "song.mid".into(),
                output: "swung.mid".into(),
                edit: "SWING amount=55 grid=1/8 bars=1..4\n".to_owned(),
            }
        );
    }

    #[test]
    fn parses_chordize_and_arpeggiate() {
        assert_eq!(
            parse(args(&[
                "midy",
                "chordize",
                "melody.mid",
                "chords.mid",
                "--quality",
                "min7",
                "--track",
                "1",
            ]))
            .unwrap(),
            Command::Chordize {
                input: "melody.mid".into(),
                output: "chords.mid".into(),
                edit: "CHORDIZE quality=min7 track=1\n".to_owned(),
            }
        );
        assert_eq!(
            parse(args(&[
                "midy",
                "arpeggiate",
                "chords.mid",
                "arp.mid",
                "--grid",
                "1/16",
                "--order",
                "updown",
                "--bars",
                "1..4",
            ]))
            .unwrap(),
            Command::Arpeggiate {
                input: "chords.mid".into(),
                output: "arp.mid".into(),
                edit: "ARPEGGIATE grid=1/16 order=updown bars=1..4\n".to_owned(),
            }
        );
    }

    #[test]
    fn parses_track_channel_transforms() {
        assert_eq!(
            parse(args(&[
                "midy", "extract", "song.mid", "--track", "1", "out.mid"
            ]))
            .unwrap(),
            Command::Extract {
                input: "song.mid".into(),
                output: "out.mid".into(),
                selector: midi::TrackChannelSelector {
                    track: Some(1),
                    channel: None,
                },
            }
        );
        assert_eq!(
            parse(args(&["midy", "mute", "song.mid", "--ch", "9", "out.mid"])).unwrap(),
            Command::Mute {
                input: "song.mid".into(),
                output: "out.mid".into(),
                selector: midi::TrackChannelSelector {
                    track: None,
                    channel: Some(9),
                },
            }
        );
        assert!(matches!(
            parse(args(&["midy", "solo", "song.mid", "--track=0", "out.mid"])).unwrap(),
            Command::Solo { .. }
        ));
    }

    #[test]
    fn parses_split() {
        assert_eq!(
            parse(args(&[
                "midy",
                "split",
                "song.mid",
                "--by",
                "channel",
                "--out-dir",
                "split"
            ]))
            .unwrap(),
            Command::Split {
                input: "song.mid".into(),
                out_dir: "split".into(),
                mode: midi::SplitMode::Channel,
            }
        );
    }

    #[test]
    fn parses_merge() {
        assert_eq!(
            parse(args(&[
                "midy",
                "merge",
                "drums.mid",
                "bass.mid",
                "chords.mid",
                "-o",
                "song.mid",
            ]))
            .unwrap(),
            Command::Merge {
                inputs: vec!["drums.mid".into(), "bass.mid".into(), "chords.mid".into()],
                output: "song.mid".into(),
            }
        );
        assert!(parse(args(&["midy", "merge", "only.mid", "-o", "out.mid"])).is_err());
        assert!(parse(args(&["midy", "merge", "a.mid", "b.mid"])).is_err());
    }

    #[test]
    fn parses_diff() {
        assert_eq!(
            parse(args(&["midy", "diff", "before.mid", "after.mid"])).unwrap(),
            Command::Diff {
                before: "before.mid".into(),
                after: "after.mid".into(),
            }
        );
    }

    #[test]
    fn parses_render() {
        assert_eq!(
            parse(args(&[
                "midy",
                "render",
                "song.mid",
                "song.flac",
                "--soundfont",
                "gm.sf2",
                "--synth",
                "fluidsynth",
                "--sample-rate",
                "48000",
            ]))
            .unwrap(),
            Command::Render {
                input: "song.mid".into(),
                output: "song.flac".into(),
                soundfont: "gm.sf2".into(),
                synth: "fluidsynth".to_owned(),
                sample_rate: 48_000,
            }
        );
        assert!(parse(args(&["midy", "render", "song.mid", "song.wav"])).is_err());
    }

    #[test]
    fn rejects_unknown_args() {
        assert!(parse(args(&["midy", "inspect"])).is_err());
        assert!(parse(args(&["midy", "apply", "song.mid", "edits.txt", "out.mid"])).is_err());
    }
}
