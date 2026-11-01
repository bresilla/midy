//! Rendering the difference between two files as edit commands.

use std::collections::{HashMap, VecDeque};

use midly::Smf;

use super::model::{Note, scan_notes};
use super::options::{EventDetail, QueryOptions};
use super::parse_smf;
use super::timeline::timeline_event_lines;
use crate::Result;

type NoteSignature = (usize, u8, u8, u64, u64, u8, u8);

fn note_signature(note: &Note) -> NoteSignature {
    (
        note.track,
        note.channel,
        note.key,
        note.start,
        note.duration,
        note.velocity,
        note.off_velocity,
    )
}

fn sort_key(note: &Note) -> (usize, u64, u8, u8, u64, u8, u8, usize) {
    (
        note.track,
        note.start,
        note.channel,
        note.key,
        note.duration,
        note.velocity,
        note.off_velocity,
        note.on_event_index,
    )
}

/// Renders a diff as edit commands that can be piped into apply.
pub fn render_diff(before: &[u8], after: &[u8]) -> Result<String> {
    let before_smf = parse_smf(before)?;
    let after_smf = parse_smf(after)?;
    let mut before_notes = scan_notes(&before_smf).notes;
    let mut after_notes = scan_notes(&after_smf).notes;
    before_notes.sort_by_key(sort_key);
    after_notes.sort_by_key(sort_key);

    let mut unmatched_after = HashMap::<NoteSignature, VecDeque<usize>>::new();
    for (index, note) in after_notes.iter().enumerate() {
        unmatched_after
            .entry(note_signature(note))
            .or_default()
            .push_back(index);
    }

    let mut matched_after = vec![false; after_notes.len()];
    let mut removed_notes = Vec::new();
    for note in &before_notes {
        match unmatched_after
            .get_mut(&note_signature(note))
            .and_then(VecDeque::pop_front)
        {
            Some(index) => matched_after[index] = true,
            None => removed_notes.push(note),
        }
    }
    let added_notes = after_notes
        .iter()
        .zip(&matched_after)
        .filter_map(|(note, matched)| (!matched).then_some(note))
        .collect::<Vec<_>>();
    let new_tracks = after_smf
        .tracks
        .len()
        .saturating_sub(before_smf.tracks.len());
    let (removed_events, added_events) = event_changes(&before_smf, &after_smf);

    let mut out = String::new();
    let changed = !removed_notes.is_empty()
        || !added_notes.is_empty()
        || new_tracks > 0
        || !removed_events.is_empty()
        || !added_events.is_empty();
    if !changed {
        return Ok(out);
    }

    emit!(out, "# MIDY_DIFF v1");
    if after_smf.tracks.len() < before_smf.tracks.len() {
        emit!(
            out,
            "# WARN after has {} tracks and before has {}; empty tracks are not removed",
            after_smf.tracks.len(),
            before_smf.tracks.len(),
        );
    }
    for note in removed_notes {
        emit!(out, "DELETE_NOTE id={}", note.id);
    }
    for _ in 0..new_tracks {
        emit!(out, "ADD_TRACK");
    }
    for note in added_notes {
        emit!(
            out,
            "ADD_NOTE track={} ch={} key={} start={} dur={} vel={} off_vel={}",
            note.track,
            note.channel,
            note.key,
            note.start,
            note.duration,
            note.velocity,
            note.off_velocity,
        );
    }
    if !removed_events.is_empty() || !added_events.is_empty() {
        emit!(
            out,
            "# NON_NOTE_DIFF removed={} added={}",
            removed_events.len(),
            added_events.len()
        );
    }
    for fields in removed_events {
        emit!(out, "DELETE_EVENT {fields}");
    }
    for fields in added_events {
        emit!(out, "ADD_EVENT {fields}");
    }

    Ok(out)
}

/// Compares non-note events as multisets, returning their edit fields.
fn event_changes(before: &Smf<'_>, after: &Smf<'_>) -> (Vec<String>, Vec<String>) {
    let mut remaining = HashMap::<String, usize>::new();
    for line in event_fields(before) {
        *remaining.entry(line).or_default() += 1;
    }

    let mut added = Vec::new();
    for line in event_fields(after) {
        match remaining.get_mut(&line) {
            Some(count) if *count > 0 => *count -= 1,
            _ => added.push(line),
        }
    }

    let mut removed = remaining
        .into_iter()
        .flat_map(|(line, count)| std::iter::repeat_n(line, count))
        .collect::<Vec<_>>();
    removed.sort();
    added.sort();
    (removed, added)
}

fn event_fields(smf: &Smf<'_>) -> Vec<String> {
    let scan = scan_notes(smf);
    timeline_event_lines(smf, &scan, EventDetail::All, &QueryOptions::default())
        .into_iter()
        .filter(|line| !line.contains("kind=end_of_track"))
        .filter_map(|line| line.split_once(' ').map(|(_, fields)| fields.to_owned()))
        .collect()
}
