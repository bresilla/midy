//! Edit behavior, checked against raw MIDI events rather than midy's own reader.

mod common;

use common::{PPQ, band, midi, track};
use midly::{MetaMessage, MidiMessage, Smf, TrackEventKind};
use midy::midi::{
    ApplyOptions, ChordKeep, EditFormat, QueryOptions, ReadOptions, ReduceChordsOptions,
    apply_edits, apply_edits_with, apply_timeline_csv_edits, apply_timeline_json_edits, fix_midi,
    fix_midi_with_options, merge_midi, merge_midi_report, render_analysis, render_analysis_json,
    render_chords, render_diff, render_lint, render_roll, render_timeline,
    render_timeline_csv_with, render_timeline_json, render_timeline_json_with,
    render_timeline_with, render_tracks, suggest_bassline, suggest_reduce_chords,
};

fn note_lines(bytes: &[u8]) -> Vec<String> {
    render_timeline(bytes)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("NOTE "))
        .map(ToOwned::to_owned)
        .collect()
}

fn error(bytes: &[u8], edits: &str) -> String {
    apply_edits(bytes, edits).unwrap_err().to_string()
}

/// Absolute tick plus a short label for each event of a track.
fn raw_events(bytes: &[u8], track_index: usize) -> Vec<(u64, String)> {
    let smf = Smf::parse(bytes).unwrap();
    let mut tick = 0;
    smf.tracks[track_index]
        .iter()
        .map(|event| {
            tick += u64::from(event.delta.as_int());
            let label = match event.kind {
                TrackEventKind::Midi { message, .. } => match message {
                    MidiMessage::NoteOn { key, vel } => format!("on {key} {vel}"),
                    MidiMessage::NoteOff { key, .. } => format!("off {key}"),
                    MidiMessage::Controller { controller, value } => {
                        format!("cc {controller} {value}")
                    }
                    other => format!("{other:?}"),
                },
                TrackEventKind::Meta(MetaMessage::EndOfTrack) => "end".to_owned(),
                TrackEventKind::Meta(MetaMessage::Tempo(tempo)) => format!("tempo {tempo}"),
                other => format!("{other:?}"),
            };
            (tick, label)
        })
        .collect()
}

fn one_note() -> Vec<u8> {
    midi(vec![track().note(0, 96, 0, 60, 64)])
}

fn two_tracks() -> Vec<u8> {
    midi(vec![
        track().note(0, 96, 0, 60, 64),
        track().note(0, 96, 1, 64, 64),
    ])
}

mod strict_fields {
    use super::*;

    #[test]
    fn misspelled_filter_is_rejected_with_a_hint() {
        let message = error(&two_tracks(), "DELETE_NOTES trakc=1\n");
        assert!(message.contains("edit line 1"), "{message}");
        assert!(message.contains("'trakc'"), "{message}");
        assert!(message.contains("did you mean 'track'"), "{message}");
    }

    #[test]
    fn misspelled_filter_cannot_widen_a_transpose() {
        let message = error(&two_tracks(), "TRANSPOSE semitones=2 trak=1\n");
        assert!(message.contains("'trak'"), "{message}");
    }

    #[test]
    fn bare_values_are_rejected() {
        let message = error(&two_tracks(), "DELETE_NOTES 1\n");
        assert!(message.contains("bare value '1'"), "{message}");
    }

    #[test]
    fn aliases_of_a_mute_filter_are_checked() {
        let message = error(&two_tracks(), "MUTE chan=9\n");
        assert!(message.contains("'chan'"), "{message}");
    }

    #[test]
    fn destructive_commands_need_a_filter() {
        for edit in ["DELETE_NOTES\n", "MUTE\n", "SOLO\n"] {
            assert!(error(&two_tracks(), edit).contains("all=true"), "{edit}");
        }
        let cleared = apply_edits(&two_tracks(), "DELETE_NOTES all=true\n").unwrap();
        assert!(note_lines(&cleared).is_empty());
    }

    #[test]
    fn filter_naming_a_missing_track_fails() {
        let message = error(&two_tracks(), "TRANSPOSE semitones=1 track=9\n");
        assert!(message.contains("track 9 does not exist"), "{message}");
    }

    #[test]
    fn bars_and_ticks_cannot_be_mixed() {
        let message = error(&one_note(), "SHIFT ticks=1 bars=1 start=0\n");
        assert!(
            message.contains("either bar/bars or start/end"),
            "{message}"
        );
    }

    #[test]
    fn unknown_commands_get_a_suggestion() {
        let message = error(&one_note(), "TRANSPOZE semitones=1\n");
        assert!(message.contains("did you mean TRANSPOSE"), "{message}");
    }

    #[test]
    fn valid_positional_note_ids_still_work() {
        let edited = apply_edits(&one_note(), "DELETE_NOTE t0n0\n").unwrap();
        assert!(note_lines(&edited).is_empty());
        assert!(apply_edits(&one_note(), "SET_NOTE t0n0 key=61\n").is_ok());
    }

    #[test]
    fn code_fences_and_comments_are_skipped() {
        let edits = "```\n# comment\nTRANSPOSE semitones=1 # inline\n```\n";
        assert!(note_lines(&apply_edits(&one_note(), edits).unwrap())[0].contains("key=61"));
    }
}

mod event_order {
    use super::*;

    #[test]
    fn fix_writes_note_off_before_same_tick_note_on() {
        let overlapping = midi(vec![track().note(0, 96, 0, 60, 80).note(48, 96, 0, 60, 80)]);
        let fixed = fix_midi(&overlapping).unwrap();
        let events = raw_events(&fixed, 0);
        let at_48 = events
            .iter()
            .filter(|(tick, _)| *tick == 48)
            .map(|(_, label)| label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(at_48, ["off 60", "on 60 80"]);
    }

    #[test]
    fn added_note_ending_at_another_note_start_releases_first() {
        let later = midi(vec![track().note(96, 96, 0, 60, 80)]);
        let edited = apply_edits(
            &later,
            "ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=80\n",
        )
        .unwrap();
        let events = raw_events(&edited, 0);
        let at_96 = events
            .iter()
            .filter(|(tick, _)| *tick == 96)
            .map(|(_, label)| label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(at_96, ["off 60", "on 60 80"]);
    }

    #[test]
    fn zero_length_note_keeps_on_before_off() {
        let zero = midi(vec![
            track()
                .on(0, 0, 60, 80)
                .off(0, 0, 60)
                .note(48, 48, 0, 62, 80),
        ]);
        let edited = apply_edits(&zero, "TRANSPOSE semitones=1 key=62\n").unwrap();
        let events = raw_events(&edited, 0);
        assert_eq!(events[0].1, "on 60 80");
        assert_eq!(events[1].1, "off 60");
    }

    #[test]
    fn added_events_precede_notes_on_the_same_tick() {
        let edited = apply_edits(&one_note(), "SET_PROGRAM track=0 ch=0 program=5\n").unwrap();
        let events = raw_events(&edited, 0);
        assert!(events[0].1.contains("ProgramChange"), "{events:?}");
        assert_eq!(events[1].1, "on 60 64");
    }
}

mod velocity {
    use super::*;

    #[test]
    fn zero_velocity_is_rejected_everywhere_it_would_silence_a_note() {
        for edit in [
            "ADD_NOTE track=0 ch=0 key=60 start=0 dur=96 vel=0\n",
            "SET_NOTE id=t0n0 vel=0\n",
            "VELOCITY set=0\n",
        ] {
            assert!(error(&one_note(), edit).contains("1..127"), "{edit}");
        }
    }

    #[test]
    fn note_off_velocity_can_be_zero() {
        let edited = apply_edits(&one_note(), "SET_NOTE id=t0n0 off_vel=0\n").unwrap();
        assert!(note_lines(&edited)[0].contains("off_vel=0"));
    }
}

mod zero_length_notes {
    use super::*;

    fn with_zero_length() -> Vec<u8> {
        midi(vec![
            track()
                .on(0, 0, 36, 64)
                .off(0, 0, 36)
                .note(0, 96, 0, 60, 64),
        ])
    }

    #[test]
    fn unrelated_edits_still_apply() {
        let edited = apply_edits(&with_zero_length(), "VELOCITY add=1 key=60\n").unwrap();
        let lines = note_lines(&edited);
        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("key=36") && line.contains("dur=0"))
        );
    }

    #[test]
    fn an_unmodified_timeline_applies() {
        let midi = with_zero_length();
        assert!(apply_edits(&midi, &render_timeline(&midi).unwrap()).is_ok());
    }

    #[test]
    fn setting_a_duration_to_zero_is_still_rejected_and_names_the_note() {
        let message = error(&one_note(), "SET_NOTE id=t0n0 dur=0\n");
        assert!(message.contains("t0n0"), "{message}");
        assert!(message.contains("edit line 1"), "{message}");
    }
}

mod quantize {
    use super::*;

    fn with_short_note() -> Vec<u8> {
        midi(vec![track().note(0, 96, 0, 60, 80).note(4, 6, 0, 42, 80)])
    }

    #[test]
    fn short_notes_keep_one_grid_step() {
        let quantized = apply_edits(&with_short_note(), "QUANTIZE grid=1/16 mode=both\n").unwrap();
        assert!(
            note_lines(&quantized)
                .iter()
                .any(|line| line.contains("key=42") && line.contains("dur=24"))
        );
    }

    #[test]
    fn duration_mode_keeps_one_grid_step() {
        let quantized = apply_edits(
            &with_short_note(),
            "QUANTIZE grid=24 mode=duration key=42\n",
        )
        .unwrap();
        assert!(
            note_lines(&quantized)
                .iter()
                .any(|line| line.contains("key=42") && line.contains("dur=24"))
        );
    }

    #[test]
    fn scaling_keeps_notes_audible() {
        let scaled = apply_edits(&one_note(), "SCALE_DURATION factor=1/1000\n").unwrap();
        assert!(note_lines(&scaled)[0].contains("dur=1 "));
    }
}

mod timeline_notes {
    use super::*;

    fn edited(replace: (&str, &str)) -> Vec<u8> {
        let midi = one_note();
        let timeline = render_timeline(&midi).unwrap();
        assert!(timeline.contains(replace.0), "{timeline}");
        apply_edits(&midi, &timeline.replace(replace.0, replace.1)).unwrap()
    }

    #[test]
    fn editing_start_with_stale_position_applies() {
        assert!(note_lines(&edited((" start=0 ", " start=24 ")))[0].contains("start=24 dur=96"));
    }

    #[test]
    fn editing_only_the_position_applies() {
        assert!(note_lines(&edited((" pos=1:1:0 ", " pos=2:1:0 ")))[0].contains("start=384"));
    }

    #[test]
    fn editing_only_the_name_applies() {
        assert!(note_lines(&edited((" name=C4 ", " name=D4 ")))[0].contains("key=62"));
    }

    #[test]
    fn editing_only_the_end_changes_the_duration() {
        assert!(note_lines(&edited((" end=96 ", " end=120 ")))[0].contains("dur=120"));
    }

    #[test]
    fn conflicting_edits_of_one_value_are_rejected() {
        let midi = one_note();
        let timeline = render_timeline(&midi)
            .unwrap()
            .replace(" start=0 ", " start=24 ")
            .replace(" pos=1:1:0 ", " pos=2:1:0 ");
        assert!(
            apply_edits(&midi, &timeline)
                .unwrap_err()
                .to_string()
                .contains("disagree")
        );
    }

    #[test]
    fn a_note_line_without_id_points_at_add_note() {
        let message = error(&one_note(), "NOTE track=0 ch=0 key=60 start=0 dur=96\n");
        assert!(message.contains("ADD_NOTE"), "{message}");
    }

    #[test]
    fn tricky_track_names_do_not_break_an_unmodified_timeline() {
        let named = midi(vec![track().name(0, b"x=1 x=2 x=3").note(0, 96, 0, 60, 64)]);
        assert!(apply_edits(&named, &render_timeline(&named).unwrap()).is_ok());
    }

    #[test]
    fn removed_lines_warn_and_prune_deletes() {
        let midi = two_tracks();
        let kept = render_timeline(&midi)
            .unwrap()
            .lines()
            .filter(|line| !line.contains("id=t1n0"))
            .collect::<Vec<_>>()
            .join("\n");

        let applied =
            apply_edits_with(&midi, &kept, EditFormat::Auto, ApplyOptions::default()).unwrap();
        assert_eq!(note_lines(&applied.bytes).len(), 2);
        assert!(
            applied
                .report
                .warnings
                .iter()
                .any(|warning| warning.contains("--prune"))
        );

        let pruned = apply_edits_with(
            &midi,
            &kept,
            EditFormat::Auto,
            ApplyOptions {
                prune: true,
                ..ApplyOptions::default()
            },
        )
        .unwrap();
        assert_eq!(note_lines(&pruned.bytes).len(), 1);
        assert_eq!(pruned.report.notes_deleted, 1);
    }

    #[test]
    fn prune_respects_a_scoped_timeline() {
        let midi = two_tracks();
        let options = ReadOptions {
            query: QueryOptions {
                track: Some(1),
                ..QueryOptions::default()
            },
            ..ReadOptions::default()
        };
        let scoped = render_timeline_with(&midi, &options).unwrap();
        assert!(scoped.contains("SCOPE track=1"));
        let pruned = apply_edits_with(
            &midi,
            &scoped,
            EditFormat::Auto,
            ApplyOptions {
                prune: true,
                ..ApplyOptions::default()
            },
        )
        .unwrap();
        assert_eq!(note_lines(&pruned.bytes).len(), 2);
    }

    #[test]
    fn prune_refuses_input_that_is_not_a_timeline() {
        let result = apply_edits_with(
            &one_note(),
            "TRANSPOSE semitones=1\n",
            EditFormat::Text,
            ApplyOptions {
                prune: true,
                ..ApplyOptions::default()
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn changing_a_read_only_line_warns() {
        let midi = band();
        let timeline = render_timeline(&midi)
            .unwrap()
            .replace("bpm=120.000", "bpm=140.000");
        let applied =
            apply_edits_with(&midi, &timeline, EditFormat::Auto, ApplyOptions::default()).unwrap();
        assert!(
            applied
                .report
                .warnings
                .iter()
                .any(|warning| warning.contains("read-only"))
        );
    }

    #[test]
    fn an_unchanged_timeline_reports_no_warnings() {
        let midi = band();
        let timeline = render_timeline(&midi).unwrap();
        let applied =
            apply_edits_with(&midi, &timeline, EditFormat::Auto, ApplyOptions::default()).unwrap();
        assert_eq!(applied.report.warnings, Vec::<String>::new());
        assert_eq!(applied.report.notes_changed, 0);
    }
}

mod structured_rows {
    use super::*;

    fn notes() -> Vec<u8> {
        midi(vec![
            track()
                .note(0, 96, 0, 60, 80)
                .note(96, 96, 0, 64, 80)
                .note(192, 96, 0, 67, 80),
        ])
    }

    fn compact(json: &str) -> String {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        serde_json::to_string(&value).unwrap()
    }

    #[test]
    fn minified_json_keeps_every_note() {
        let midi = notes();
        let edited =
            apply_timeline_json_edits(&midi, &compact(&render_timeline_json(&midi).unwrap()));
        assert_eq!(note_lines(&edited.unwrap()).len(), 3);
    }

    #[test]
    fn pretty_printed_json_keeps_every_note() {
        let midi = notes();
        let value: serde_json::Value =
            serde_json::from_str(&render_timeline_json(&midi).unwrap()).unwrap();
        let pretty = serde_json::to_string_pretty(&value).unwrap();
        assert_eq!(
            note_lines(&apply_timeline_json_edits(&midi, &pretty).unwrap()).len(),
            3
        );
    }

    #[test]
    fn a_bare_array_of_rows_is_accepted() {
        let midi = notes();
        let rows = r#"[{"id":"t0n0","track":0,"ch":0,"key":61,"start":0,"dur":96,"vel":80}]"#;
        let lines = note_lines(&apply_timeline_json_edits(&midi, rows).unwrap());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("key=61"));
    }

    #[test]
    fn misspelled_json_fields_are_rejected() {
        let rows = r#"[{"id":"t0n0","trak":0}]"#;
        let message = apply_timeline_json_edits(&notes(), rows)
            .unwrap_err()
            .to_string();
        assert!(message.contains("did you mean 'track'"), "{message}");
    }

    #[test]
    fn invalid_json_is_reported() {
        assert!(apply_timeline_json_edits(&notes(), "{not json").is_err());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let rows = r#"[{"id":"t0n0","key":60},{"id":"t0n0","key":61}]"#;
        let message = apply_timeline_json_edits(&notes(), rows)
            .unwrap_err()
            .to_string();
        assert!(message.contains("duplicate note id"), "{message}");
    }

    #[test]
    fn new_rows_add_notes() {
        let midi = notes();
        let csv = "id,track,ch,key,start,dur,vel\n,0,0,72,0,96,80\n";
        let lines = note_lines(&apply_timeline_csv_edits(&midi, csv).unwrap());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("key=72"));
    }

    #[test]
    fn row_velocity_zero_is_rejected() {
        let csv = "id,track,ch,key,start,dur,vel\nt0n0,0,0,60,0,96,0\n";
        assert!(apply_timeline_csv_edits(&notes(), csv).is_err());
    }

    #[test]
    fn csv_with_byte_order_mark_and_crlf_applies() {
        let midi = notes();
        let csv = "\u{feff}id,track,ch,key,start,dur,vel\r\nt0n0,0,0,61,0,96,80\r\n";
        assert!(note_lines(&apply_timeline_csv_edits(&midi, csv).unwrap())[0].contains("key=61"));
    }

    #[test]
    fn scoped_json_never_deletes_other_tracks() {
        let midi = two_tracks();
        let options = ReadOptions {
            query: QueryOptions {
                track: Some(1),
                ..QueryOptions::default()
            },
            ..ReadOptions::default()
        };
        let json = render_timeline_json_with(&midi, &options).unwrap();
        assert_eq!(
            note_lines(&apply_timeline_json_edits(&midi, &json).unwrap()).len(),
            2
        );
    }

    #[test]
    fn scoped_csv_never_deletes_other_tracks() {
        let midi = two_tracks();
        let options = ReadOptions {
            query: QueryOptions {
                track: Some(1),
                ..QueryOptions::default()
            },
            ..ReadOptions::default()
        };
        let csv = render_timeline_csv_with(&midi, &options).unwrap();
        assert!(csv.lines().next().unwrap().ends_with(",scope"));
        assert_eq!(
            note_lines(&apply_timeline_csv_edits(&midi, &csv).unwrap()).len(),
            2
        );
    }

    #[test]
    fn prune_is_refused_for_rows() {
        let result = apply_edits_with(
            &notes(),
            "id,track,ch,key,start,dur,vel\n",
            EditFormat::Csv,
            ApplyOptions {
                prune: true,
                ..ApplyOptions::default()
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn formats_are_detected_from_the_text() {
        let midi = notes();
        let json = render_timeline_json(&midi).unwrap();
        let applied = apply_edits_with(&midi, &json, EditFormat::Auto, ApplyOptions::default());
        assert_eq!(applied.unwrap().report.notes_after, 3);

        let csv = "id,track,ch,key,start,dur,vel\nt0n0,0,0,60,0,96,80\n";
        let applied = apply_edits_with(&midi, csv, EditFormat::Auto, ApplyOptions::default());
        assert_eq!(applied.unwrap().report.notes_after, 1);
    }
}

mod drums {
    use super::*;

    #[test]
    fn transpose_leaves_percussion_alone_by_default() {
        let edited = apply_edits(&band(), "TRANSPOSE semitones=2\n").unwrap();
        let lines = note_lines(&edited);
        assert!(lines.iter().any(|line| line.contains("ch=9 key=36")));
        assert!(lines.iter().any(|line| line.contains("ch=0 key=62")));
    }

    #[test]
    fn percussion_can_be_named_explicitly() {
        let by_channel = apply_edits(&band(), "TRANSPOSE semitones=2 ch=9\n").unwrap();
        assert!(
            note_lines(&by_channel)
                .iter()
                .any(|line| line.contains("ch=9 key=38"))
        );
        let all = apply_edits(&band(), "TRANSPOSE semitones=2 drums=true\n").unwrap();
        assert!(
            note_lines(&all)
                .iter()
                .any(|line| line.contains("ch=9 key=38"))
        );
    }

    #[test]
    fn timing_commands_include_percussion_unless_excluded() {
        let shifted = apply_edits(&band(), "SHIFT ticks=1 drums=false\n").unwrap();
        assert!(
            note_lines(&shifted)
                .iter()
                .any(|line| line.contains("ch=9 key=36") && line.contains("start=0 "))
        );
        let only = apply_edits(&band(), "VELOCITY set=1 drums=only\n").unwrap();
        assert!(
            note_lines(&only)
                .iter()
                .any(|line| line.contains("ch=0") && line.contains("vel=80"))
        );
    }

    #[test]
    fn reduce_chords_stays_within_each_voice() {
        let band_notes = midi(vec![
            track().note(0, 96, 0, 72, 80),
            track().note(0, 96, 1, 36, 80),
            track().note(0, 96, 9, 38, 80),
        ]);
        let edits = suggest_reduce_chords(&band_notes, &ReduceChordsOptions::default()).unwrap();
        assert_eq!(edits, "");

        let two_voices = midi(vec![
            track().note(0, 96, 0, 60, 80).note(0, 96, 0, 67, 80),
            track().note(0, 96, 1, 36, 80),
        ]);
        let edits = suggest_reduce_chords(
            &two_voices,
            &ReduceChordsOptions {
                keep: ChordKeep::Highest,
                ..ReduceChordsOptions::default()
            },
        )
        .unwrap();
        assert_eq!(edits, "DELETE_NOTE id=t0n0\n");
    }

    #[test]
    fn chord_analysis_ignores_percussion() {
        let text = render_chords(&band(), &Default::default()).unwrap();
        assert!(!text.contains("name=Cunknown"), "{text}");
        assert!(text.contains("keys=36,60,64,67"), "{text}");
    }
}

mod time_edits {
    use super::*;

    fn automated() -> Vec<u8> {
        midi(vec![
            track()
                .tempo(0, 500_000)
                .tempo(192, 400_000)
                .cc(96, 0, 64, 127)
                .note(0, 192, 0, 60, 80)
                .end(384),
        ])
    }

    fn tick_of(events: &[(u64, String)], label: &str) -> u64 {
        events
            .iter()
            .find(|(_, text)| text.contains(label))
            .unwrap()
            .0
    }

    #[test]
    fn whole_song_scale_moves_every_event_and_the_track_end() {
        let scaled = apply_edits(&automated(), "SCALE_TIME factor=1/2\n").unwrap();
        let events = raw_events(&scaled, 0);
        assert_eq!(tick_of(&events, "cc 64"), 48);
        assert_eq!(tick_of(&events, "tempo 400000"), 96);
        assert_eq!(tick_of(&events, "off 60"), 96);
        assert_eq!(tick_of(&events, "end"), 192);
    }

    #[test]
    fn whole_song_shift_keeps_setup_events_at_the_start() {
        let shifted = apply_edits(&automated(), "SHIFT ticks=24\n").unwrap();
        let events = raw_events(&shifted, 0);
        assert_eq!(tick_of(&events, "tempo 500000"), 0);
        assert_eq!(tick_of(&events, "tempo 400000"), 216);
        assert_eq!(tick_of(&events, "cc 64"), 120);
        assert_eq!(tick_of(&events, "end"), 408);
    }

    #[test]
    fn notes_only_leaves_events_alone() {
        let scaled = apply_edits(&automated(), "SCALE_TIME factor=1/2 notes_only=true\n").unwrap();
        let events = raw_events(&scaled, 0);
        assert_eq!(tick_of(&events, "cc 64"), 96);
        assert_eq!(tick_of(&events, "tempo 400000"), 192);
    }

    #[test]
    fn a_filtered_shift_moves_matching_channel_events_only() {
        let midi = midi(vec![
            track().tempo(96, 400_000),
            track().cc(96, 0, 1, 10).note(0, 48, 0, 60, 80),
            track().cc(96, 1, 1, 10).note(0, 48, 1, 62, 80),
        ]);
        let shifted = apply_edits(&midi, "SHIFT ticks=10 track=1\n").unwrap();
        assert_eq!(tick_of(&raw_events(&shifted, 1), "cc 1"), 106);
        assert_eq!(tick_of(&raw_events(&shifted, 2), "cc 1"), 96);
        assert_eq!(tick_of(&raw_events(&shifted, 0), "tempo"), 96);
    }

    #[test]
    fn beat_and_bar_follow_the_active_meter() {
        let six_eight = midi(vec![track().time_signature(0, 6, 3).end(576)]);
        let edited = apply_edits(
            &six_eight,
            "ADD_NOTE track=0 ch=0 key=60 at=1:2 dur=beat vel=80\n",
        )
        .unwrap();
        assert!(note_lines(&edited)[0].contains("start=48 dur=48"));

        let changing = midi(vec![
            track()
                .time_signature(0, 3, 2)
                .time_signature(288, 4, 2)
                .end(960),
        ]);
        let edited = apply_edits(
            &changing,
            "ADD_NOTE track=0 ch=0 key=60 at=2:1 dur=bar vel=80\n",
        )
        .unwrap();
        assert!(note_lines(&edited)[0].contains("dur=384"));
    }

    #[test]
    fn set_note_measures_bars_at_the_notes_new_position() {
        let changing = midi(vec![
            track()
                .time_signature(0, 3, 2)
                .time_signature(288, 4, 2)
                .note(0, 96, 0, 60, 80),
        ]);
        let edited = apply_edits(&changing, "SET_NOTE id=t0n0 at=2:1 dur=bar\n").unwrap();
        assert!(note_lines(&edited)[0].contains("start=288 dur=384"));
    }

    #[test]
    fn swing_follows_the_conventional_percentage() {
        let pair = midi(vec![track().note(0, 24, 0, 60, 64).note(48, 24, 0, 62, 64)]);
        for (amount, start) in [(50, 48), (66, 63), (75, 72), (100, 96)] {
            let swung = apply_edits(&pair, &format!("SWING amount={amount} grid=1/8\n")).unwrap();
            assert!(
                note_lines(&swung)
                    .iter()
                    .any(|line| line.contains(&format!("key=62 name=D4 start={start} "))),
                "amount {amount}"
            );
        }
    }

    #[test]
    fn triplet_grids_work_everywhere() {
        let midi = one_note();
        assert!(apply_edits(&midi, "QUANTIZE grid=1/8t\n").is_ok());
        let roll = render_roll(
            &midi,
            &midy::midi::RollOptions {
                grid: Some("1/8t".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(roll.contains("grid_ticks=32"), "{roll}");
    }
}

mod arpeggios {
    use super::*;

    fn seventh() -> Vec<u8> {
        midi(vec![
            track()
                .note(0, 96, 0, 60, 80)
                .note(0, 96, 0, 64, 80)
                .note(0, 96, 0, 67, 80)
                .note(0, 96, 0, 71, 80),
        ])
    }

    fn sequence(bytes: &[u8]) -> Vec<u64> {
        let mut notes = note_lines(bytes)
            .iter()
            .map(|line| {
                let field = |name: &str| {
                    line.split_whitespace()
                        .find_map(|part| part.strip_prefix(name))
                        .unwrap()
                        .parse::<u64>()
                        .unwrap()
                };
                (field("start="), field("key="))
            })
            .collect::<Vec<_>>();
        notes.sort_unstable();
        notes.into_iter().map(|(_, key)| key).collect()
    }

    #[test]
    fn updown_rises_then_falls() {
        let edited = apply_edits(&seventh(), "ARPEGGIATE grid=24 order=updown\n").unwrap();
        assert_eq!(sequence(&edited), [60, 64, 67, 71, 67, 64]);
    }

    #[test]
    fn outside_keeps_the_previous_alternating_order() {
        let edited = apply_edits(&seventh(), "ARPEGGIATE grid=24 order=outside\n").unwrap();
        assert_eq!(sequence(&edited), [60, 71, 64, 67]);
    }
}

mod event_editing {
    use super::*;

    #[test]
    fn set_tempo_replaces_the_tempo_at_that_tick() {
        let edited = apply_edits(&band(), "SET_TEMPO bpm=150\n").unwrap();
        let text = render_timeline(&edited).unwrap();
        assert!(text.contains("kind=tempo us_per_quarter=400000 bpm=150.000"));
        assert_eq!(text.matches("tick=0 kind=tempo").count(), 1);
        assert!(text.contains("tick=768 kind=tempo"));
    }

    #[test]
    fn set_tempo_adds_at_a_new_position() {
        let edited = apply_edits(&one_note(), "SET_TEMPO bpm=60 at=2:1\n").unwrap();
        assert!(
            render_timeline(&edited)
                .unwrap()
                .contains("tick=384 kind=tempo us_per_quarter=1000000")
        );
    }

    #[test]
    fn set_program_replaces_or_adds() {
        let edited = apply_edits(&band(), "SET_PROGRAM track=2 ch=1 program=40\n").unwrap();
        let text = render_timeline(&edited).unwrap();
        assert!(text.contains("track=2 tick=0 kind=program_change ch=1 program=40"));
        assert!(!text.contains("program=33"));
    }

    #[test]
    fn add_event_round_trips_through_read() {
        let edits = "\
ADD_EVENT track=0 tick=0 kind=cc ch=0 controller=64 value=127
ADD_EVENT track=0 tick=1 kind=pitch_bend ch=0 value=-8192
ADD_EVENT track=0 tick=2 kind=channel_aftertouch ch=0 pressure=40
ADD_EVENT track=0 tick=3 kind=poly_aftertouch ch=0 key=C4 pressure=41
ADD_EVENT track=0 tick=4 kind=marker text=\"Verse # 1\"
ADD_EVENT track=0 tick=5 kind=time_signature numerator=6 denominator=8
ADD_EVENT track=0 tick=6 kind=key_signature sharps=-1 minor=true
ADD_EVENT track=0 tick=7 kind=sysex hex=7e7f0901f7
";
        let edited = apply_edits(&one_note(), edits).unwrap();
        let text = render_timeline(&edited).unwrap();
        for expected in [
            "kind=cc ch=0 controller=64 value=127",
            "kind=pitch_bend ch=0 value=-8192",
            "kind=channel_aftertouch ch=0 pressure=40",
            "kind=poly_aftertouch ch=0 key=60 pressure=41",
            "kind=marker text=\"Verse # 1\"",
            "kind=time_signature numerator=6 denominator=8",
            "kind=key_signature sharps=-1 minor=true",
            "kind=sysex bytes=5 hex=7e7f0901f7",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
    }

    #[test]
    fn delete_events_filters_by_kind_and_controller() {
        let edited = apply_edits(&band(), "DELETE_EVENTS kind=cc controller=64\n").unwrap();
        assert!(!render_timeline(&edited).unwrap().contains("controller=64"));
        let message = error(&band(), "DELETE_EVENTS kind=ccc\n");
        assert!(message.contains("did you mean 'cc'"), "{message}");
        assert!(error(&band(), "DELETE_EVENTS\n").contains("all=true"));
    }

    #[test]
    fn add_track_upgrades_a_single_track_file() {
        let edited = apply_edits(
            &one_note(),
            "ADD_TRACK name=\"Lead Synth\"\nADD_NOTE track=1 ch=1 key=72 start=0 dur=96 vel=80\n",
        )
        .unwrap();
        let text = render_timeline(&edited).unwrap();
        assert!(text.contains("HEADER format=parallel"), "{text}");
        assert!(
            text.contains("kind=track_name text=\"Lead Synth\""),
            "{text}"
        );
        assert!(text.contains("NOTE id=t1n0 track=1"), "{text}");
    }

    #[test]
    fn set_track_name_replaces_the_name() {
        let edited = apply_edits(&band(), "SET_TRACK_NAME track=2 name=Sub\n").unwrap();
        assert!(render_tracks(&edited).unwrap().contains("names=Sub"));
    }
}

mod reports {
    use super::*;

    #[test]
    fn counts_each_command_and_summarizes() {
        let edits = "TRANSPOSE semitones=1 track=1\nDELETE_NOTES key=C8\nADD_NOTE track=1 ch=0 key=60 start=0 dur=1 vel=1\n";
        let applied =
            apply_edits_with(&band(), edits, EditFormat::Text, ApplyOptions::default()).unwrap();
        let report = applied.report;
        assert_eq!(report.commands[0].matched, 6);
        assert_eq!(report.commands[1].matched, 0);
        assert_eq!(report.notes_added, 1);
        assert_eq!(report.notes_changed, 6);
        assert_eq!(report.warnings, ["line 2: DELETE_NOTES matched nothing"]);
    }

    #[test]
    fn strict_mode_turns_warnings_into_errors() {
        let result = apply_edits_with(
            &band(),
            "DELETE_NOTES key=C8\n",
            EditFormat::Text,
            ApplyOptions {
                strict: true,
                ..ApplyOptions::default()
            },
        );
        assert!(result.unwrap_err().to_string().contains("matched nothing"));
    }

    #[test]
    fn deleting_twice_is_an_error() {
        let message = error(
            &one_note(),
            "DELETE_NOTE id=t0n0\nSET_NOTE id=t0n0 key=61\n",
        );
        assert!(message.contains("deleted by an earlier line"), "{message}");
    }
}

mod reading {
    use super::*;

    fn query(apply: impl FnOnce(&mut QueryOptions)) -> ReadOptions {
        let mut options = ReadOptions::default();
        apply(&mut options.query);
        options
    }

    #[test]
    fn shows_controllers_bends_markers_and_text() {
        let text = render_timeline(&band()).unwrap();
        for expected in [
            "EVENT track=1 tick=0 kind=cc ch=0 controller=64 value=127",
            "kind=pitch_bend ch=1 value=808",
            "kind=marker text=\"Verse 1\"",
            "kind=text text=\"quote \\\" and = sign\"",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
    }

    #[test]
    fn scope_filters_notes_and_events() {
        let text = render_timeline_with(&band(), &query(|q| q.track = Some(2))).unwrap();
        assert!(text.contains("SCOPE track=2"));
        assert!(
            text.lines()
                .filter(|line| line.starts_with("NOTE "))
                .all(|line| line.contains("track=2"))
        );
        assert!(!text.contains("kind=tempo"));
    }

    #[test]
    fn bars_resolve_to_ticks_in_the_scope() {
        let text =
            render_timeline_with(&band(), &query(|q| q.bars = Some("2".to_owned()))).unwrap();
        assert!(text.contains("SCOPE start=384 end=768"));
        assert!(text.contains("key=65"));
        assert!(!text.contains("key=60 "));
    }

    #[test]
    fn a_missing_track_is_an_error() {
        assert!(render_timeline_with(&band(), &query(|q| q.track = Some(9))).is_err());
    }

    #[test]
    fn compact_lines_are_shorter_and_still_apply() {
        let midi = band();
        let options = ReadOptions {
            compact: true,
            events: midy::midi::EventDetail::None,
            ..ReadOptions::default()
        };
        let compact = render_timeline_with(&midi, &options).unwrap();
        assert!(compact.len() * 2 < render_timeline(&midi).unwrap().len());
        let edited = apply_edits(&midi, &compact.replace("key=60 ", "key=61 ")).unwrap();
        assert!(
            note_lines(&edited)
                .iter()
                .any(|line| line.contains("key=61"))
        );
    }

    #[test]
    fn event_detail_can_hide_performance_data() {
        let options = ReadOptions {
            events: midy::midi::EventDetail::Basic,
            ..ReadOptions::default()
        };
        let text = render_timeline_with(&band(), &options).unwrap();
        assert!(!text.contains("kind=cc"));
        assert!(text.contains("kind=tempo"));
        assert!(text.contains("kind=program_change"));
    }

    #[test]
    fn stuck_notes_are_visible_and_editable() {
        let stuck = midi(vec![track().on(0, 0, 60, 80).end(96)]);
        let text = render_timeline(&stuck).unwrap();
        assert!(text.contains("STUCK id=t0n0 track=0 ch=0 key=60 name=C4 start=0 vel=80"));

        let moved = apply_edits(&stuck, "TRANSPOSE semitones=2\n").unwrap();
        assert!(
            render_timeline(&moved)
                .unwrap()
                .contains("STUCK id=t0n0 track=0 ch=0 key=62")
        );

        let closed = apply_edits(&stuck, "SET_NOTE id=t0n0 dur=48\n").unwrap();
        assert!(note_lines(&closed)[0].contains("dur=48"));

        let removed = apply_edits(&stuck, "DELETE_NOTE id=t0n0\n").unwrap();
        assert!(!render_timeline(&removed).unwrap().contains("STUCK"));
    }

    #[test]
    fn a_time_signature_with_a_huge_denominator_does_not_panic() {
        let odd = midi(vec![
            track().time_signature(0, 4, 40).note(0, 96, 0, 60, 64),
        ]);
        assert!(
            render_timeline(&odd)
                .unwrap()
                .contains("denominator_power=40")
        );
        assert!(render_analysis(&odd, None).is_ok());
    }
}

mod analysis {
    use super::*;

    fn melody(keys: &[u8]) -> Vec<u8> {
        let mut part = track();
        for (index, key) in keys.iter().enumerate() {
            let tick = index as u64 * u64::from(PPQ);
            part = part.note(tick, 96, 0, *key, 80);
        }
        midi(vec![part])
    }

    #[test]
    fn density_counts_notes() {
        let many = melody(&[60; 8]);
        let few = midi(vec![
            track().note(0, 96, 0, 60, 80).note(672, 96, 0, 60, 80),
        ]);
        assert!(render_tracks(&many).unwrap().contains("density=1.000"));
        assert!(render_tracks(&few).unwrap().contains("density=0.250"));
    }

    #[test]
    fn distinguishes_minor_from_major() {
        let a_minor = melody(&[69, 72, 76, 69, 71, 72, 74, 76, 72, 69, 64, 69]);
        let text = render_analysis(&a_minor, None).unwrap();
        assert!(text.contains("key_guess=A_minor"), "{text}");
    }

    #[test]
    fn percussion_does_not_pull_the_key() {
        let mut hats = track();
        for index in 0..64 {
            hats = hats.note(index * 24, 12, 9, 42, 70);
        }
        let song = midi(vec![
            track()
                .note(0, 96, 0, 60, 80)
                .note(96, 96, 0, 64, 80)
                .note(192, 96, 0, 67, 80)
                .note(288, 96, 0, 72, 80)
                .note(384, 96, 0, 65, 80)
                .note(480, 96, 0, 67, 80)
                .note(576, 192, 0, 60, 80),
            hats,
        ]);
        let text = render_analysis(&song, None).unwrap();
        assert!(text.contains("key_guess=C_major"), "{text}");
    }

    #[test]
    fn reports_the_declared_key() {
        let declared = midi(vec![
            track()
                .meta(0, MetaMessage::KeySignature(-3, true))
                .note(0, 96, 0, 60, 80),
        ]);
        assert!(
            render_analysis(&declared, None)
                .unwrap()
                .contains("key_declared=C_minor")
        );
    }

    #[test]
    fn chord_ties_prefer_the_bass_as_root() {
        let sus = midi(vec![
            track()
                .note(0, 96, 0, 55, 80)
                .note(0, 96, 0, 57, 80)
                .note(0, 96, 0, 62, 80),
        ]);
        let text = render_chords(&sus, &Default::default()).unwrap();
        assert!(text.contains("name=Gsus2"), "{text}");
    }

    #[test]
    fn json_analysis_is_valid_and_complete() {
        let text = render_analysis_json(&band(), Some("a\"b.mid")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["ticks_per_beat"], 96);
        assert_eq!(value["tempos"].as_array().unwrap().len(), 2);
        assert_eq!(value["time_signatures"][0]["numerator"], 4);
        assert!(!value["chords"].as_array().unwrap().is_empty());
    }

    #[test]
    fn json_timeline_is_valid_with_every_section() {
        let text = render_timeline_json(&band()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["notes"].as_array().unwrap().len(), 13);
        assert!(!value["events"].as_array().unwrap().is_empty());
        assert_eq!(value["stuck_notes"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn roll_reports_truncation() {
        let long = midi(vec![track().note(0, 96_000, 0, 60, 80)]);
        let roll = render_roll(&long, &Default::default()).unwrap();
        assert!(
            roll.contains("columns=256 total_columns=4000 truncated=true"),
            "{roll}"
        );

        let full = render_roll(
            &long,
            &midy::midi::RollOptions {
                max_columns: 0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(full.contains("columns=4000 total_columns=4000 truncated=false"));
    }
}

mod lint_and_fix {
    use super::*;

    #[test]
    fn finds_overlaps_past_the_adjacent_note() {
        let nested = midi(vec![
            track()
                .note(0, 300, 0, 60, 80)
                .note(10, 10, 0, 60, 80)
                .note(100, 10, 0, 60, 80),
        ]);
        let lint = render_lint(&nested).unwrap();
        assert_eq!(lint.matches("WARN overlap").count(), 2, "{lint}");
    }

    #[test]
    fn warns_about_orphan_note_offs_and_fix_removes_them() {
        let orphan = midi(vec![track().off(0, 0, 60).note(10, 10, 0, 62, 80)]);
        assert!(
            render_lint(&orphan)
                .unwrap()
                .contains("WARN orphan_note_off")
        );
        let fixed = fix_midi(&orphan).unwrap();
        assert!(!render_lint(&fixed).unwrap().contains("orphan"));
    }

    #[test]
    fn closing_a_stuck_note_keeps_its_velocity() {
        let stuck = midi(vec![track().on(0, 0, 60, 80).end(96)]);
        let closed = fix_midi_with_options(
            &stuck,
            midy::midi::FixOptions {
                stuck_note_mode: midy::midi::StuckNoteFixMode::Close,
                stuck_note_duration: 48,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(note_lines(&closed)[0].contains("vel=80"));
    }
}

mod merging {
    use super::*;

    #[test]
    fn keeps_the_first_tempo_map_and_reports_conflicts() {
        let first = midi(vec![track().tempo(0, 500_000).note(0, 96, 0, 60, 64)]);
        let second = midi(vec![track().tempo(0, 250_000).note(0, 96, 1, 62, 64)]);
        let (merged, warnings) = merge_midi_report(&[&first, &second]).unwrap();
        let text = render_timeline(&merged).unwrap();
        assert!(text.contains("us_per_quarter=500000"));
        assert!(!text.contains("us_per_quarter=250000"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("input 1"));
        assert_eq!(note_lines(&merged).len(), 2);
    }

    #[test]
    fn identical_tempo_maps_are_kept_silently() {
        let first = midi(vec![track().tempo(0, 500_000)]);
        let (_, warnings) = merge_midi_report(&[&first, &first]).unwrap();
        assert!(warnings.is_empty());
    }

    #[test]
    fn timing_mismatches_still_fail() {
        let other =
            b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x30MTrk\x00\x00\x00\x04\x00\xff\x2f\x00";
        assert!(merge_midi(&[&one_note(), other]).is_err());
    }
}

mod diffing {
    use super::*;

    #[test]
    fn reports_controller_and_bend_changes() {
        let before = one_note();
        let after = midi(vec![
            track()
                .cc(0, 0, 64, 127)
                .bend(0, 0, 9000)
                .note(0, 96, 0, 60, 64),
        ]);
        let diff = render_diff(&before, &after).unwrap();
        assert!(diff.contains("# NON_NOTE_DIFF removed=0 added=2"), "{diff}");
        assert!(diff.contains("ADD_EVENT track=0 tick=0 kind=cc"), "{diff}");
    }

    #[test]
    fn output_recreates_the_target_including_events_and_tracks() {
        let before = one_note();
        let after = apply_edits(
            &band(),
            "TRANSPOSE semitones=3 track=1\nSET_TEMPO bpm=99\nSET_PROGRAM track=2 ch=1 program=2\n",
        )
        .unwrap();
        let diff = render_diff(&before, &after).unwrap();
        let recreated = apply_edits(&before, &diff).unwrap();
        assert_eq!(render_diff(&recreated, &after).unwrap(), "", "{diff}");
    }

    #[test]
    fn replaying_keeps_same_tick_notes_with_their_own_lengths() {
        let before = midi(vec![
            track().note(0, 129, 0, 70, 89).note(0, 153, 0, 70, 127),
        ]);
        let after = apply_edits(&before, "VELOCITY add=1\n").unwrap();
        let replayed = apply_edits(&before, &render_diff(&before, &after).unwrap()).unwrap();
        assert_eq!(render_diff(&after, &replayed).unwrap(), "");
    }

    #[test]
    fn lengthening_the_first_of_two_same_tick_notes_keeps_each_velocity() {
        let before = midi(vec![
            track().note(0, 100, 0, 70, 50).note(0, 200, 0, 70, 90),
        ]);
        let after = apply_edits(&before, "SET_NOTE id=t0n0 dur=300\n").unwrap();
        let lines = note_lines(&after);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("dur=300") && line.contains("vel=50"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("dur=200") && line.contains("vel=90"))
        );
    }

    #[test]
    fn identical_files_have_an_empty_diff() {
        assert_eq!(render_diff(&band(), &band()).unwrap(), "");
    }
}

mod suggestions {
    use super::*;

    fn chord() -> Vec<u8> {
        midi(vec![
            track()
                .note(0, 96, 0, 60, 80)
                .note(0, 96, 0, 64, 80)
                .note(0, 96, 0, 67, 80),
        ])
    }

    #[test]
    fn bass_goes_to_a_new_named_track_by_default() {
        let midi = chord();
        let edits = suggest_bassline(&midi, &Default::default()).unwrap();
        assert!(edits.starts_with("ADD_TRACK name=Bass\n"), "{edits}");
        assert!(edits.contains("ADD_NOTE track=1 "), "{edits}");
        let applied = apply_edits(&midi, &edits).unwrap();
        assert!(
            note_lines(&applied)
                .iter()
                .any(|line| line.contains("track=1") && line.contains("key=36"))
        );
    }

    #[test]
    fn a_missing_output_track_is_created() {
        let midi = chord();
        let options = midy::midi::BasslineOptions {
            output_track: Some(2),
            ..Default::default()
        };
        let edits = suggest_bassline(&midi, &options).unwrap();
        assert!(apply_edits(&midi, &edits).is_ok(), "{edits}");
    }

    #[test]
    fn durations_accept_every_length_form() {
        let midi = chord();
        for (value, ticks) in [("1/8t", 32), ("beat", 96), ("bar", 384), ("100", 100)] {
            let options = midy::midi::BasslineOptions {
                duration: Some(value.to_owned()),
                ..Default::default()
            };
            let edits = suggest_bassline(&midi, &options).unwrap();
            assert!(edits.contains(&format!("dur={ticks} ")), "{value}: {edits}");
        }
    }
}
