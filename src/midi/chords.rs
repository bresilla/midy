//! Chord detection and naming.

use std::collections::BTreeMap;

use super::format::{note_name, pc_name};
use super::model::Note;
use super::model::collect_notes;
use super::options::{ChordKeep, ChordsOptions};
use super::parse_smf;
use super::time::TimeSignatureMap;
use crate::Result;

/// Notes sounding together.
pub(super) struct ChordSlice {
    pub(super) tick: u64,
    pub(super) keys: Vec<u8>,
}

impl ChordSlice {
    pub(super) fn line(&self, time_map: Option<&TimeSignatureMap>) -> String {
        let join = |values: Vec<String>| values.join(",");
        let analysis = analyze_chord(&self.keys);
        let position = time_map
            .map(|map| format!(" pos={}", map.tick_position(self.tick)))
            .unwrap_or_default();
        format!(
            "CHORD tick={}{} notes={} keys={} name={} inversion={} bass={} confidence={:.2}",
            self.tick,
            position,
            join(self.keys.iter().map(|key| note_name(*key)).collect()),
            join(self.keys.iter().map(ToString::to_string).collect()),
            analysis.name,
            analysis.inversion,
            self.keys
                .first()
                .copied()
                .map(note_name)
                .unwrap_or_default(),
            analysis.confidence,
        )
    }
}

/// Finds the chords in `notes`, considering them as one texture.
pub(super) fn chord_slices(notes: &mut [Note], window: u64) -> Vec<ChordSlice> {
    chord_groups(notes, window)
        .into_iter()
        .filter(|group| group.len() > 1)
        .filter_map(|group| {
            let tick = group.iter().map(|note| note.start).min()?;
            let mut keys = group.iter().map(|note| note.key).collect::<Vec<_>>();
            keys.sort_unstable();
            keys.dedup();
            Some(ChordSlice { tick, keys })
        })
        .collect()
}

/// Renders detected chord slices.
pub fn render_chords(bytes: &[u8], options: &ChordsOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let time_map = TimeSignatureMap::from_smf(&smf);
    let query = options.query.resolved(&smf, time_map.as_ref())?;
    let mut notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_pitched(note))
        .collect::<Vec<_>>();
    let slices = chord_slices(&mut notes, options.window);
    let mut out = String::new();

    emit!(
        out,
        "MIDY_CHORDS v1 window={} chords={}",
        options.window,
        slices.len()
    );
    for slice in &slices {
        emit!(out, "{}", slice.line(time_map.as_ref()));
    }

    Ok(out)
}

/// Groups notes that start together.
pub(super) fn chord_groups(notes: &mut [Note], window: u64) -> Vec<Vec<&Note>> {
    notes.sort_by_key(|note| {
        (
            note.start,
            note.track,
            note.channel,
            note.key,
            note.on_event_index,
        )
    });
    let mut groups = Vec::<Vec<&Note>>::new();
    let mut current = Vec::<&Note>::new();
    let mut group_start = None::<u64>;

    for note in notes.iter() {
        if group_start.is_some_and(|start| note.start > start.saturating_add(window)) {
            groups.push(std::mem::take(&mut current));
            group_start = Some(note.start);
        } else if group_start.is_none() {
            group_start = Some(note.start);
        }
        current.push(note);
    }

    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

/// Groups notes that start together on the same track and channel.
pub(super) fn chord_groups_by_voice(notes: Vec<Note>, window: u64) -> Vec<Vec<Note>> {
    let mut voices = BTreeMap::<(usize, u8), Vec<Note>>::new();
    for note in notes {
        voices
            .entry((note.track, note.channel))
            .or_default()
            .push(note);
    }
    let mut groups = Vec::new();
    for voice in voices.values_mut() {
        for group in chord_groups(voice, window) {
            groups.push(group.into_iter().cloned().collect());
        }
    }
    groups
}

pub(super) fn choose_chord_note(group: &[&Note], keep: &ChordKeep) -> usize {
    match keep {
        ChordKeep::Highest => group
            .iter()
            .enumerate()
            .max_by_key(|(_, note)| note.key)
            .map(|(index, _)| index)
            .unwrap_or(0),
        ChordKeep::Lowest => group
            .iter()
            .enumerate()
            .min_by_key(|(_, note)| note.key)
            .map(|(index, _)| index)
            .unwrap_or(0),
        ChordKeep::Root => {
            let keys = group.iter().map(|note| note.key).collect::<Vec<_>>();
            let root = analyze_chord(&keys).root_pc;
            group
                .iter()
                .enumerate()
                .filter(|(_, note)| note.key % 12 == root)
                .min_by_key(|(_, note)| note.key)
                .map(|(index, _)| index)
                .unwrap_or_else(|| choose_chord_note(group, &ChordKeep::Lowest))
        }
        ChordKeep::Nth(nth) => {
            let mut indexed = group.iter().enumerate().collect::<Vec<_>>();
            indexed.sort_by_key(|(_, note)| note.key);
            let index = nth.saturating_sub(1).min(indexed.len().saturating_sub(1));
            indexed.get(index).map(|(index, _)| *index).unwrap_or(0)
        }
    }
}

#[derive(Debug, Clone)]
struct ChordAnalysis {
    name: String,
    root_pc: u8,
    inversion: String,
    confidence: f64,
}

fn analyze_chord(keys: &[u8]) -> ChordAnalysis {
    let mut pitch_classes = keys.iter().map(|key| key % 12).collect::<Vec<_>>();
    pitch_classes.sort_unstable();
    pitch_classes.dedup();
    let bass_pc = keys.iter().min().copied().unwrap_or(60) % 12;
    let patterns: &[(&str, &[u8])] = &[
        ("maj7", &[0, 4, 7, 11]),
        ("7", &[0, 4, 7, 10]),
        ("min7", &[0, 3, 7, 10]),
        ("mMaj7", &[0, 3, 7, 11]),
        ("m7b5", &[0, 3, 6, 10]),
        ("dim7", &[0, 3, 6, 9]),
        ("maj", &[0, 4, 7]),
        ("min", &[0, 3, 7]),
        ("dim", &[0, 3, 6]),
        ("aug", &[0, 4, 8]),
        ("sus2", &[0, 2, 7]),
        ("sus4", &[0, 5, 7]),
    ];

    let mut best = None::<(u8, &str, usize)>;
    let root_order = (0..12).map(|offset| (bass_pc + offset) % 12);
    for root in root_order {
        let intervals = pitch_classes
            .iter()
            .map(|pc| (pc + 12 - root) % 12)
            .collect::<Vec<_>>();
        for (name, required) in patterns {
            if required.iter().all(|interval| intervals.contains(interval)) {
                let score = required.len();
                if best.is_none_or(|(_, _, best_score)| score > best_score) {
                    best = Some((root, *name, score));
                }
            }
        }
    }

    let (root_pc, quality, score) = best.unwrap_or((bass_pc, "unknown", 1));
    let name = if quality == "unknown" {
        format!("{}unknown", pc_name(root_pc))
    } else {
        format!("{}{}", pc_name(root_pc), quality)
    };
    let inversion = if bass_pc == root_pc {
        "root".to_owned()
    } else {
        format!("over_{}", pc_name(bass_pc))
    };
    let confidence = score as f64 / pitch_classes.len().max(1) as f64;

    ChordAnalysis {
        name,
        root_pc,
        inversion,
        confidence,
    }
}

/// Returns the pitch class of the detected chord root.
pub(super) fn chord_root(keys: &[u8]) -> u8 {
    analyze_chord(keys).root_pc
}
