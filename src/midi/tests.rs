use super::model::collect_notes;
use super::*;

const EMPTY_SINGLE_TRACK_MIDI: &[u8] =
    b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x04\x00\xff\x2f\x00";
const ONE_NOTE_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x0c\x00\x90\x3c\x40\x60\x80\x3c\x40\x00\xff\x2f\x00";
const ONE_CHORD_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x1c\x00\x90\x3c\x40\x00\x90\x40\x40\x00\x90\x43\x40\x60\x80\x3c\x40\x00\x80\x40\x40\x00\x80\x43\x40\x00\xff\x2f\x00";
const TWO_CHANNEL_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x14\x00\x90\x3c\x40\x00\x91\x40\x40\x60\x80\x3c\x40\x00\x81\x40\x40\x00\xff\x2f\x00";
const STUCK_NOTE_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x08\x00\x90\x3c\x40\x60\xff\x2f\x00";
const TIME_SIGNATURE_CHANGE_MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x15\x00\xff\x58\x04\x03\x02\x18\x08\x82\x20\xff\x58\x04\x04\x02\x18\x08\x00\xff\x2f\x00";

#[test]
fn renders_note_timeline() {
    let text = render_timeline(ONE_NOTE_MIDI).unwrap();

    assert!(text.contains("MIDY_TIMELINE v1"));
    assert!(text.contains("HEADER format=single timing=metrical ticks_per_beat=96 tracks=1"));
    assert!(text.contains("SONG length_ticks=96 length_beats=1.000"));
    assert!(text.contains(
        "NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=0 dur=96 end=96 pos=1:1:0 end_pos=1:2:0 vel=64 off_vel=64"
    ));
}

#[test]
fn renders_timeline_json() {
    let text = render_timeline_json(ONE_NOTE_MIDI).unwrap();

    assert!(text.contains("\"kind\": \"MIDY_TIMELINE\""));
    assert!(text.contains("\"format\": \"single\""));
    assert!(text.contains("\"notes\""));
    assert!(text.contains("\"id\": \"t0n0\""));
    assert!(text.contains("\"key\": 60"));
}

#[test]
fn renders_timeline_csv() {
    let text = render_timeline_csv(ONE_NOTE_MIDI).unwrap();

    assert!(text.starts_with("id,track,ch,key,name,start,dur,end,vel,off_vel\n"));
    assert!(text.contains("t0n0,0,0,60,C4,0,96,96,64,64"));
}

#[test]
fn applies_timeline_json_rows() {
    let json = render_timeline_json(ONE_NOTE_MIDI)
        .unwrap()
        .replace("\"key\": 60", "\"key\": 62")
        .replace("\"name\": \"C4\"", "\"name\": \"D4\"");
    let rewritten = apply_timeline_json_edits(ONE_NOTE_MIDI, &json).unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=62 name=D4"));
}

#[test]
fn applies_timeline_csv_rows_and_deletes_missing_ids() {
    let csv = render_timeline_csv(ONE_CHORD_MIDI).unwrap();
    let edited_csv = csv
        .lines()
        .filter(|line| !line.contains(",64,E4,"))
        .collect::<Vec<_>>()
        .join("\n");
    let rewritten = apply_timeline_csv_edits(ONE_CHORD_MIDI, &edited_csv).unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("key=60 name=C4"));
    assert!(!text.contains("key=64 name=E4"));
    assert!(text.contains("key=67 name=G4"));
}

#[test]
fn renders_empty_diff_for_equal_notes() {
    let diff = render_diff(ONE_NOTE_MIDI, ONE_NOTE_MIDI).unwrap();

    assert!(diff.is_empty());
}

#[test]
fn renders_diff_that_recreates_after_notes() {
    let after = apply_edits(ONE_NOTE_MIDI, "SET_NOTE id=t0n0 key=62 vel=90\n").unwrap();
    let diff = render_diff(ONE_NOTE_MIDI, &after).unwrap();
    let recreated = apply_edits(ONE_NOTE_MIDI, &diff).unwrap();

    assert!(diff.contains("DELETE_NOTE id=t0n0"));
    assert!(diff.contains("ADD_NOTE track=0 ch=0 key=62 start=0 dur=96 vel=90 off_vel=64"));
    assert_eq!(
        render_timeline(&recreated).unwrap(),
        render_timeline(&after).unwrap()
    );
}

#[test]
fn renders_non_note_event_diff_comments() {
    let diff = render_diff(EMPTY_SINGLE_TRACK_MIDI, TIME_SIGNATURE_CHANGE_MIDI).unwrap();

    assert!(diff.contains("# NON_NOTE_DIFF removed=0 added=2"));
    assert!(diff.contains("ADD_EVENT track=0 tick=0 kind=time_signature"));
}

#[test]
fn applies_add_note_command() {
    let rewritten = apply_edits(
        EMPTY_SINGLE_TRACK_MIDI,
        "ADD_NOTE track=0 ch=0 key=64 start=0 dur=48 vel=80\n",
    )
    .unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains(
        "NOTE id=t0n0 track=0 ch=0 key=64 name=E4 start=0 dur=48 end=48 pos=1:1:0 end_pos=1:1:48 vel=80 off_vel=64"
    ));
}

#[test]
fn applies_set_note_command() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "SET_NOTE id=t0n0 key=61 dur=48\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains(
        "NOTE id=t0n0 track=0 ch=0 key=61 name=C#4 start=0 dur=48 end=48 pos=1:1:0 end_pos=1:1:48 vel=64 off_vel=64"
    ));
}

#[test]
fn applies_delete_note_command() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "DELETE_NOTE id=t0n0\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(!text.lines().any(|line| line.starts_with("NOTE ")));
}

#[test]
fn applies_whole_file_transpose() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "TRANSPOSE semitones=7\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=67 name=G4"));
}

#[test]
fn applies_filtered_shift() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=24 track=0 ch=0\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("start=24 dur=96 end=120 pos=1:1:24 end_pos=1:2:24"));
}

#[test]
fn applies_scale_duration() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "SCALE_DURATION factor=1/2\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("start=0 dur=48 end=48"));
}

#[test]
fn applies_quantize_both() {
    let shifted = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=17\n").unwrap();
    let quantized = apply_edits(&shifted, "QUANTIZE grid=24 mode=both\n").unwrap();
    let text = render_timeline(&quantized).unwrap();

    assert!(text.contains("start=24 dur=96 end=120"));
}

#[test]
fn applies_delete_notes_filter() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "DELETE_NOTES start=0 end=96 key=60\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(!text.lines().any(|line| line.starts_with("NOTE ")));
}

#[test]
fn applies_modified_timeline_note_line() {
    let timeline = render_timeline(ONE_NOTE_MIDI).unwrap();
    let edited_timeline = timeline.replace("key=60 name=C4", "key=62 name=D4");
    let rewritten = apply_edits(ONE_NOTE_MIDI, &edited_timeline).unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=62 name=D4"));
}

#[test]
fn applies_musical_add_note_aliases() {
    let rewritten = apply_edits(
        EMPTY_SINGLE_TRACK_MIDI,
        "ADD_NOTE track=0 ch=0 key=C4 at=2:1 dur=1/4 vel=80\n",
    )
    .unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains(
        "NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=384 dur=96 end=480 pos=2:1:0 end_pos=2:2:0 vel=80 off_vel=64"
    ));
}

#[test]
fn uses_time_signature_map_for_musical_positions() {
    let rewritten = apply_edits(
        TIME_SIGNATURE_CHANGE_MIDI,
        "\
ADD_NOTE track=0 ch=0 key=C4 at=1:3 dur=beat vel=80
ADD_NOTE track=0 ch=0 key=E4 at=2:4 dur=beat vel=80
",
    )
    .unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("key=60 name=C4 start=192 dur=96 end=288 pos=1:3:0 end_pos=2:1:0"));
    assert!(text.contains("key=64 name=E4 start=576 dur=96 end=672 pos=2:4:0 end_pos=3:1:0"));
}

#[test]
fn rejects_beats_outside_the_current_time_signature() {
    let error = apply_edits(
        TIME_SIGNATURE_CHANGE_MIDI,
        "ADD_NOTE track=0 ch=0 key=C4 at=1:4 dur=beat vel=80\n",
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("beat 4 exceeds time signature 3/4")
    );
}

#[test]
fn uses_time_signature_map_for_bar_filters() {
    let notes = apply_edits(
        TIME_SIGNATURE_CHANGE_MIDI,
        "\
ADD_NOTE track=0 ch=0 key=C4 at=1:3 dur=beat vel=80
ADD_NOTE track=0 ch=0 key=E4 at=2:4 dur=beat vel=80
",
    )
    .unwrap();
    let shifted = apply_edits(&notes, "SHIFT by=beat bars=2\n").unwrap();
    let text = render_timeline(&shifted).unwrap();

    assert!(text.contains("key=60 name=C4 start=192"));
    assert!(text.contains("key=64 name=E4 start=672"));
}

#[test]
fn applies_note_name_patch_and_fraction_duration() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "SET_NOTE id=t0n0 key=F#4 dur=1/8\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=66 name=F#4 start=0 dur=48"));
}

#[test]
fn applies_bar_filter_and_fraction_shift() {
    let rewritten = apply_edits(ONE_NOTE_MIDI, "SHIFT by=1/4 bar=1\n").unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=96"));
}

#[test]
fn quantizes_to_fractional_grid() {
    let shifted = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=73\n").unwrap();
    let quantized = apply_edits(&shifted, "QUANTIZE grid=1/4 mode=start\n").unwrap();
    let text = render_timeline(&quantized).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=96"));
}

#[test]
fn applies_humanize_reproducibly() {
    let first = apply_edits(ONE_NOTE_MIDI, "HUMANIZE timing=12 velocity=8 seed=7\n").unwrap();
    let second = apply_edits(ONE_NOTE_MIDI, "HUMANIZE timing=12 velocity=8 seed=7\n").unwrap();
    let notes = collect_notes(&parse_smf(&first).unwrap());
    let note = notes.first().unwrap();

    assert_eq!(first, second);
    assert!(note.start <= 12);
    assert!((56..=72).contains(&note.velocity));
}

#[test]
fn applies_dehumanize() {
    let shifted = apply_edits(ONE_NOTE_MIDI, "SHIFT ticks=17\n").unwrap();
    let dehumanized = apply_edits(&shifted, "DEHUMANIZE grid=24 mode=both\n").unwrap();
    let text = render_timeline(&dehumanized).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=24"));
}

#[test]
fn applies_swing_to_odd_grid_subdivisions() {
    let notes = apply_edits(
        EMPTY_SINGLE_TRACK_MIDI,
        "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=24 vel=64
ADD_NOTE track=0 ch=0 key=62 start=48 dur=24 vel=64
",
    )
    .unwrap();
    let swung = apply_edits(&notes, "SWING amount=75 grid=1/8\n").unwrap();
    let text = render_timeline(&swung).unwrap();

    assert!(text.contains("key=60 name=C4 start=0"));
    assert!(text.contains("key=62 name=D4 start=72"));
}

#[test]
fn applies_velocity_scale_add_set_and_compress() {
    let scaled = apply_edits(ONE_NOTE_MIDI, "VELOCITY scale=1/2\n").unwrap();
    assert!(render_timeline(&scaled).unwrap().contains("vel=32"));

    let added = apply_edits(ONE_NOTE_MIDI, "VELOCITY add=10\n").unwrap();
    assert!(render_timeline(&added).unwrap().contains("vel=74"));

    let set = apply_edits(ONE_NOTE_MIDI, "VELOCITY set=96\n").unwrap();
    assert!(render_timeline(&set).unwrap().contains("vel=96"));

    let compressed = apply_edits(ONE_NOTE_MIDI, "VELOCITY compress=1/2 center=80\n").unwrap();
    assert!(render_timeline(&compressed).unwrap().contains("vel=72"));
}

#[test]
fn applies_crescendo() {
    let notes = apply_edits(
        EMPTY_SINGLE_TRACK_MIDI,
        "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=62 start=96 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=64 start=192 dur=48 vel=64
",
    )
    .unwrap();
    let crescendo = apply_edits(
        &notes,
        "CRESCENDO start_vel=40 end_vel=100 start=0 end=193\n",
    )
    .unwrap();
    let text = render_timeline(&crescendo).unwrap();

    assert!(text.contains("key=60 name=C4 start=0 dur=48 end=48 pos=1:1:0 end_pos=1:1:48 vel=40"));
    assert!(
        text.contains("key=62 name=D4 start=96 dur=48 end=144 pos=1:2:0 end_pos=1:2:48 vel=70")
    );
    assert!(
        text.contains("key=64 name=E4 start=192 dur=48 end=240 pos=1:3:0 end_pos=1:3:48 vel=100")
    );
}

#[test]
fn suggests_reducing_chord_to_highest_note() {
    let edits = suggest_reduce_chords(
        ONE_CHORD_MIDI,
        &ReduceChordsOptions {
            keep: ChordKeep::Highest,
            ..ReduceChordsOptions::default()
        },
    )
    .unwrap();

    assert!(edits.contains("DELETE_NOTE id=t0n0"));
    assert!(edits.contains("DELETE_NOTE id=t0n1"));
    assert!(!edits.contains("DELETE_NOTE id=t0n2"));
}

#[test]
fn suggests_bassline_from_chord_roots() {
    let edits = suggest_bassline(
        ONE_CHORD_MIDI,
        &BasslineOptions {
            output_track: Some(0),
            output_channel: 0,
            octave: 2,
            velocity: 90,
            ..BasslineOptions::default()
        },
    )
    .unwrap();
    let rewritten = apply_edits(ONE_CHORD_MIDI, &edits).unwrap();
    let text = render_timeline(&rewritten).unwrap();

    assert!(edits.contains("ADD_NOTE track=0 ch=0 key=36 start=0 dur=96 vel=90"));
    assert!(text.contains("key=36 name=C2"));
}

#[test]
fn renders_chord_names() {
    let text = render_chords(ONE_CHORD_MIDI, &ChordsOptions::default()).unwrap();

    assert!(text.contains("MIDY_CHORDS v1"));
    assert!(text.contains("name=Cmaj"));
    assert!(text.contains("notes=C4,E4,G4"));
}

#[test]
fn arpeggiates_block_chords() {
    let arpeggiated = apply_edits(ONE_CHORD_MIDI, "ARPEGGIATE grid=24 order=up\n").unwrap();
    let text = render_timeline(&arpeggiated).unwrap();

    assert!(text.contains("key=60 name=C4 start=0"));
    assert!(text.contains("key=64 name=E4 start=24"));
    assert!(text.contains("key=67 name=G4 start=48"));
}

#[test]
fn chordizes_single_notes() {
    let chordized = apply_edits(ONE_NOTE_MIDI, "CHORDIZE quality=maj\n").unwrap();
    let text = render_timeline(&chordized).unwrap();
    let note_lines = text
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .collect::<Vec<_>>();

    assert_eq!(note_lines.len(), 3);
    assert!(text.contains("key=60 name=C4"));
    assert!(text.contains("key=64 name=E4"));
    assert!(text.contains("key=67 name=G4"));
}

#[test]
fn chordizes_with_custom_intervals() {
    let chordized = apply_edits(ONE_NOTE_MIDI, "CHORDIZE intervals=0,3,7,10\n").unwrap();
    let text = render_timeline(&chordized).unwrap();

    assert!(text.contains("key=60 name=C4"));
    assert!(text.contains("key=63 name=D#4"));
    assert!(text.contains("key=67 name=G4"));
    assert!(text.contains("key=70 name=A#4"));
}

#[test]
fn blocks_arpeggios_to_grid_start() {
    let arpeggiated = apply_edits(ONE_CHORD_MIDI, "ARPEGGIATE grid=24 order=up\n").unwrap();
    let blocked = apply_edits(&arpeggiated, "BLOCK_CHORD grid=96\n").unwrap();
    let text = render_timeline(&blocked).unwrap();

    assert!(text.contains("key=60 name=C4 start=0"));
    assert!(text.contains("key=64 name=E4 start=0"));
    assert!(text.contains("key=67 name=G4 start=0"));
}

#[test]
fn inverts_chords() {
    let inverted = apply_edits(ONE_CHORD_MIDI, "INVERT_CHORDS inversion=1\n").unwrap();
    let text = render_timeline(&inverted).unwrap();

    assert!(
        !text
            .lines()
            .any(|line| line.starts_with("NOTE ") && line.contains("key=60"))
    );
    assert!(text.contains("key=64 name=E4"));
    assert!(text.contains("key=67 name=G4"));
    assert!(text.contains("key=72 name=C5"));
}

#[test]
fn doubles_notes_by_octave() {
    let doubled = apply_edits(ONE_NOTE_MIDI, "DOUBLE octave=-1\n").unwrap();
    let text = render_timeline(&doubled).unwrap();
    let note_lines = text
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .collect::<Vec<_>>();

    assert_eq!(note_lines.len(), 2);
    assert!(
        note_lines
            .iter()
            .any(|line| line.contains("key=60 name=C4"))
    );
    assert!(
        note_lines
            .iter()
            .any(|line| line.contains("key=48 name=C3"))
    );
}

#[test]
fn voice_leads_chords_by_octave() {
    let chords = apply_edits(
        EMPTY_SINGLE_TRACK_MIDI,
        "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=64 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=67 start=0 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=67 start=96 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=71 start=96 dur=48 vel=64
ADD_NOTE track=0 ch=0 key=74 start=96 dur=48 vel=64
",
    )
    .unwrap();
    let voiced = apply_edits(&chords, "VOICE_LEAD max_jump=7\n").unwrap();
    let text = render_timeline(&voiced).unwrap();

    assert!(text.contains("key=55 name=G3 start=96"));
    assert!(text.contains("key=59 name=B3 start=96"));
    assert!(text.contains("key=62 name=D4 start=96"));
}

#[test]
fn renders_ascii_roll() {
    let text = render_roll(ONE_CHORD_MIDI, &RollOptions::default()).unwrap();

    assert!(text.contains("MIDY_ROLL v1"));
    assert!(text.contains("  G4 | #"));
    assert!(text.contains("  C4 | #"));
}

#[test]
fn renders_verbose_ascii_roll_cells() {
    let text = render_roll(
        ONE_CHORD_MIDI,
        &RollOptions {
            mode: RollMode::Verbose,
            ..RollOptions::default()
        },
    )
    .unwrap();

    assert!(text.contains("mode=verbose"));
    assert!(text.contains("CELL column=0 tick=0 pos=1:1:0 notes=C4:0:0,E4:0:0,G4:0:0"));
}

#[test]
fn renders_analysis() {
    let text = render_analysis(ONE_CHORD_MIDI, Some("chord.mid")).unwrap();

    assert!(text.contains("ANALYZE file=chord.mid"));
    assert!(text.contains("track=0 role=chords"));
    assert!(text.contains("key_guess="));
    assert!(text.contains("chords:"));
}

#[test]
fn renders_analysis_json() {
    let text = render_analysis_json(ONE_CHORD_MIDI, Some("chord.mid")).unwrap();

    assert!(text.contains("\"file\": \"chord.mid\""));
    assert!(text.contains("\"track_summaries\""));
    assert!(text.contains("\"role\": \"chords\""));
}

#[test]
fn renders_tracks() {
    let text = render_tracks(ONE_CHORD_MIDI).unwrap();

    assert!(text.contains("MIDY_TRACKS v1 tracks=1"));
    assert!(text.contains("TRACK index=0 role=chords"));
    assert!(text.contains("channels=0"));
}

#[test]
fn lints_duplicate_overlap_empty_and_stuck_notes() {
    let empty_lint = render_lint(EMPTY_SINGLE_TRACK_MIDI).unwrap();
    assert!(empty_lint.contains("WARN empty_track track=0"));

    let messy = apply_edits(
        EMPTY_SINGLE_TRACK_MIDI,
        "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=48 dur=96 vel=80
",
    )
    .unwrap();
    let messy_lint = render_lint(&messy).unwrap();
    assert!(messy_lint.contains("WARN duplicate_note"));
    assert!(messy_lint.contains("WARN overlap"));

    let stuck_lint = render_lint(STUCK_NOTE_MIDI).unwrap();
    assert!(stuck_lint.contains("WARN stuck_note"));
}

#[test]
fn fixes_duplicate_and_overlapping_notes() {
    let messy = apply_edits(
        EMPTY_SINGLE_TRACK_MIDI,
        "\
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80
ADD_NOTE track=0 ch=0 key=60 start=48 dur=96 vel=80
",
    )
    .unwrap();
    let fixed = fix_midi(&messy).unwrap();
    let text = render_timeline(&fixed).unwrap();
    let note_lines = text
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .collect::<Vec<_>>();

    assert_eq!(note_lines.len(), 2);
    assert!(
        note_lines
            .iter()
            .any(|line| line.contains("start=0 dur=48"))
    );
    assert!(
        note_lines
            .iter()
            .any(|line| line.contains("start=48 dur=96"))
    );
    assert!(!render_lint(&fixed).unwrap().contains("WARN duplicate_note"));
}

#[test]
fn fixes_stuck_note_on() {
    let fixed = fix_midi(STUCK_NOTE_MIDI).unwrap();
    let text = render_timeline(&fixed).unwrap();

    assert!(!text.lines().any(|line| line.starts_with("NOTE ")));
    assert!(!render_lint(&fixed).unwrap().contains("WARN stuck_note"));
}

#[test]
fn can_close_stuck_note_on_with_missing_note_off() {
    let fixed = fix_midi_with_options(
        STUCK_NOTE_MIDI,
        FixOptions {
            stuck_note_mode: StuckNoteFixMode::Close,
            stuck_note_duration: 48,
            ..FixOptions::default()
        },
    )
    .unwrap();
    let text = render_timeline(&fixed).unwrap();

    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60 name=C4 start=0 dur=48"));
    assert!(!render_lint(&fixed).unwrap().contains("WARN stuck_note"));
}

#[test]
fn can_remove_empty_tracks_during_fix() {
    let merged = merge_midi(&[EMPTY_SINGLE_TRACK_MIDI, ONE_NOTE_MIDI]).unwrap();
    let fixed = fix_midi_with_options(
        &merged,
        FixOptions {
            remove_empty_tracks: true,
            ..FixOptions::default()
        },
    )
    .unwrap();
    let text = render_timeline(&fixed).unwrap();

    assert!(text.contains("HEADER format=single timing=metrical ticks_per_beat=96 tracks=1"));
    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60"));
}

#[test]
fn mutes_selected_channel() {
    let rewritten = mute_selection(
        TWO_CHANNEL_MIDI,
        &TrackChannelSelector {
            track: None,
            channel: Some(0),
        },
    )
    .unwrap();
    let text = render_timeline(&rewritten).unwrap();
    let note_lines = text
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .collect::<Vec<_>>();

    assert!(!note_lines.iter().any(|line| line.contains("key=60")));
    assert!(note_lines.iter().any(|line| line.contains("key=64")));
}

#[test]
fn applies_mute_and_solo_edit_commands() {
    let muted = apply_edits(TWO_CHANNEL_MIDI, "MUTE ch=0\n").unwrap();
    let muted_text = render_timeline(&muted).unwrap();
    let muted_lines = muted_text
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .collect::<Vec<_>>();

    assert!(!muted_lines.iter().any(|line| line.contains("key=60")));
    assert!(muted_lines.iter().any(|line| line.contains("key=64")));

    let soloed = apply_edits(TWO_CHANNEL_MIDI, "SOLO ch=1\n").unwrap();
    let soloed_text = render_timeline(&soloed).unwrap();
    let soloed_lines = soloed_text
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .collect::<Vec<_>>();

    assert!(!soloed_lines.iter().any(|line| line.contains("key=60")));
    assert!(soloed_lines.iter().any(|line| line.contains("key=64")));
}

#[test]
fn applies_move_track_and_set_channel_edit_commands() {
    let merged = merge_midi(&[ONE_NOTE_MIDI, ONE_CHORD_MIDI]).unwrap();
    let moved = apply_edits(&merged, "MOVE_TRACK from=1 to=0 key=67\n").unwrap();
    let moved_text = render_timeline(&moved).unwrap();

    assert!(
        moved_text
            .lines()
            .any(|line| line.starts_with("NOTE ") && line.contains("track=0 ch=0 key=67"))
    );
    assert!(
        !moved_text
            .lines()
            .any(|line| { line.starts_with("NOTE ") && line.contains("track=1 ch=0 key=67") })
    );

    let changed = apply_edits(TWO_CHANNEL_MIDI, "SET_CHANNEL track=0 ch=2 from_ch=1\n").unwrap();
    let changed_text = render_timeline(&changed).unwrap();
    assert!(
        changed_text
            .lines()
            .any(|line| line.starts_with("NOTE ") && line.contains("ch=2 key=64"))
    );
}

#[test]
fn extracts_selected_channel() {
    let rewritten = extract_selection(
        TWO_CHANNEL_MIDI,
        &TrackChannelSelector {
            track: None,
            channel: Some(1),
        },
    )
    .unwrap();
    let text = render_timeline(&rewritten).unwrap();
    let note_lines = text
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .collect::<Vec<_>>();

    assert!(!note_lines.iter().any(|line| line.contains("key=60")));
    assert!(note_lines.iter().any(|line| line.contains("key=64")));
}

#[test]
fn splits_by_channel() {
    let outputs = split_selection(TWO_CHANNEL_MIDI, SplitMode::Channel).unwrap();

    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].0, "ch-0.mid");
    assert_eq!(outputs[1].0, "ch-1.mid");
    let ch0 = render_timeline(&outputs[0].1).unwrap();
    let ch1 = render_timeline(&outputs[1].1).unwrap();
    assert!(
        ch0.lines()
            .any(|line| line.starts_with("NOTE ") && line.contains("key=60"))
    );
    assert!(
        ch1.lines()
            .any(|line| line.starts_with("NOTE ") && line.contains("key=64"))
    );
}

#[test]
fn merges_midi_files_as_parallel_tracks() {
    let merged = merge_midi(&[ONE_NOTE_MIDI, ONE_CHORD_MIDI]).unwrap();
    let text = render_timeline(&merged).unwrap();

    assert!(text.contains("HEADER format=parallel timing=metrical ticks_per_beat=96 tracks=2"));
    assert!(text.contains("TRACK index=0"));
    assert!(text.contains("TRACK index=1"));
    assert!(text.contains("NOTE id=t0n0 track=0 ch=0 key=60"));
    assert!(text.contains("NOTE id=t1n0 track=1 ch=0 key=60"));
    assert!(text.contains("NOTE id=t1n2 track=1 ch=0 key=67"));
}
