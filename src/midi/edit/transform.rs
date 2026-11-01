//! Note transforms used by the edit commands.

use crate::midi::model::ConcreteNote;
use crate::{Error, Result};

use super::command::{ArpeggioOrder, NoteFilter, QuantizeMode, VelocityCommand};
use super::session::{EditSession, EditableNote};

pub(super) fn matching_notes_mut<'a>(
    notes: &'a mut [EditableNote],
    filter: &NoteFilter,
) -> Vec<&'a mut EditableNote> {
    notes
        .iter_mut()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
        .collect()
}
pub(super) fn shift_tick(tick: u64, by: i64) -> Result<u64> {
    if by >= 0 {
        tick.checked_add(by as u64)
            .ok_or_else(|| Error::Edit("shifted tick overflowed u64".to_owned()))
    } else {
        tick.checked_sub(by.unsigned_abs()).ok_or_else(|| {
            Error::Edit(format!(
                "shift by {by} would move tick {tick} before the start"
            ))
        })
    }
}

pub(super) fn quantize_tick(tick: u64, grid: u64) -> Result<u64> {
    tick.checked_add(grid / 2)
        .map(|tick| (tick / grid) * grid)
        .ok_or_else(|| Error::Edit("quantized tick overflowed u64".to_owned()))
}

pub(super) fn apply_humanize(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    timing: u64,
    velocity: u8,
    seed: u64,
) -> Result<usize> {
    let timing_span = timing
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| Error::Edit("humanize timing range overflowed".to_owned()))?;
    let velocity_span = u64::from(velocity) * 2 + 1;
    let mut matched = 0;

    for (index, editable) in notes.iter_mut().enumerate() {
        if editable.deleted || !filter.matches(&editable.note) {
            continue;
        }
        matched += 1;
        if timing > 0 {
            let raw = stable_random(seed, index as u64, 0) % timing_span;
            let offset = raw as i64 - timing as i64;
            editable.note.start = shift_tick_clamped(editable.note.start, offset)?;
        }
        if velocity > 0 {
            let raw = stable_random(seed, index as u64, 1) % velocity_span;
            let offset = raw as i16 - i16::from(velocity);
            editable.note.velocity =
                clamp_note_on_velocity(i64::from(editable.note.velocity) + i64::from(offset));
        }
    }

    Ok(matched)
}

pub(super) fn apply_block_chord(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    grid: u64,
) -> usize {
    let mut matching = matching_notes_mut(notes, filter);
    for editable in &mut matching {
        editable.note.start = (editable.note.start / grid) * grid;
    }
    matching.len()
}

pub(super) fn apply_invert_chords(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    inversion: i16,
) -> Result<usize> {
    let groups = chord_groups_indices(notes, filter, 2);
    let matched = groups.iter().map(Vec::len).sum();
    for group in groups {
        for _ in 0..inversion.unsigned_abs() {
            let Some(index) = inversion_index(notes, &group, inversion >= 0) else {
                continue;
            };
            let semitones = if inversion >= 0 { 12 } else { -12 };
            notes[index].note.key = transpose_key(notes[index].note.key, semitones)?;
        }
    }
    Ok(matched)
}

pub(super) fn apply_voice_lead(notes: &mut [EditableNote], filter: &NoteFilter, max_jump: u8) {
    let groups = chord_groups_indices(notes, filter, 2);
    let mut previous_key = None::<(usize, u8, Vec<u8>)>;
    for group in groups {
        let track = notes[group[0]].note.track;
        let channel = notes[group[0]].note.channel;
        let mut ordered = group.clone();
        ordered.sort_by_key(|index| (notes[*index].note.key, *index));

        if let Some((previous_track, previous_channel, previous_keys)) = previous_key.as_ref()
            && *previous_track == track
            && *previous_channel == channel
            && !previous_keys.is_empty()
        {
            for (position, index) in ordered.iter().enumerate() {
                let target = previous_keys[position.min(previous_keys.len() - 1)];
                notes[*index].note.key =
                    best_voice_led_key(notes[*index].note.key, target, max_jump);
            }
        }

        let mut current_keys = ordered
            .iter()
            .map(|index| notes[*index].note.key)
            .collect::<Vec<_>>();
        current_keys.sort_unstable();
        previous_key = Some((track, channel, current_keys));
    }
}

pub(super) fn chord_groups_indices(
    notes: &[EditableNote],
    filter: &NoteFilter,
    minimum_len: usize,
) -> Vec<Vec<usize>> {
    let mut indices = notes
        .iter()
        .enumerate()
        .filter(|(_, editable)| !editable.deleted && filter.matches(&editable.note))
        .map(|(index, editable)| {
            (
                editable.note.track,
                editable.note.channel,
                editable.note.start,
                editable.note.key,
                index,
            )
        })
        .collect::<Vec<_>>();
    indices.sort_unstable();

    let mut groups = Vec::<Vec<usize>>::new();
    let mut current_key = None::<(usize, u8, u64)>;
    let mut current = Vec::<usize>::new();
    for (track, channel, start, _, index) in indices {
        let key = (track, channel, start);
        if current_key.is_some_and(|current_key| current_key != key) {
            if current.len() >= minimum_len {
                groups.push(current);
            }
            current = Vec::new();
        }
        current_key = Some(key);
        current.push(index);
    }
    if current.len() >= minimum_len {
        groups.push(current);
    }
    groups
}

pub(super) fn inversion_index(
    notes: &[EditableNote],
    group: &[usize],
    move_lowest: bool,
) -> Option<usize> {
    if move_lowest {
        group
            .iter()
            .copied()
            .min_by_key(|index| notes[*index].note.key)
    } else {
        group
            .iter()
            .copied()
            .max_by_key(|index| notes[*index].note.key)
    }
}

pub(super) fn best_voice_led_key(key: u8, target: u8, max_jump: u8) -> u8 {
    let mut nearest = key;
    let mut nearest_distance = u8::MAX;
    let mut constrained = None::<(u8, u8)>;
    for octave in -10..=10 {
        let candidate = i16::from(key) + octave * 12;
        if !(0..=127).contains(&candidate) {
            continue;
        }
        let candidate = candidate as u8;
        let distance = candidate.abs_diff(target);
        if distance < nearest_distance {
            nearest = candidate;
            nearest_distance = distance;
        }
        if distance <= max_jump && constrained.is_none_or(|(_, best)| distance < best) {
            constrained = Some((candidate, distance));
        }
    }
    constrained
        .map(|(candidate, _)| candidate)
        .unwrap_or(nearest)
}

pub(super) fn stable_random(seed: u64, index: u64, salt: u64) -> u64 {
    let mut value =
        seed ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ salt.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

pub(super) fn shift_tick_clamped(tick: u64, by: i64) -> Result<u64> {
    if by >= 0 {
        tick.checked_add(by as u64)
            .ok_or_else(|| Error::Edit("shifted tick overflowed u64".to_owned()))
    } else {
        Ok(tick.saturating_sub(by.unsigned_abs()))
    }
}

pub(super) fn apply_velocity(value: u8, command: VelocityCommand) -> Result<u8> {
    match command {
        VelocityCommand::Scale(factor) => {
            let scaled = factor.scale(u64::from(value))?;
            Ok(clamp_note_on_velocity(scaled as i64))
        }
        VelocityCommand::Add(delta) => {
            Ok(clamp_note_on_velocity(i64::from(value) + i64::from(delta)))
        }
        VelocityCommand::Set(value) => Ok(value),
        VelocityCommand::Compress { factor, center } => {
            let delta = i64::from(value) - i64::from(center);
            let magnitude = delta.unsigned_abs();
            let scaled = factor.scale(magnitude)? as i64;
            let signed = if delta < 0 { -scaled } else { scaled };
            Ok(clamp_note_on_velocity(i64::from(center) + signed))
        }
    }
}

pub(super) fn clamp_note_on_velocity(value: i64) -> u8 {
    value.clamp(1, 127) as u8
}

pub(super) fn apply_crescendo(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    start_velocity: u8,
    end_velocity: u8,
) -> Result<usize> {
    let matching = notes
        .iter()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
        .map(|editable| editable.note.start)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Ok(0);
    }
    let start_tick = filter
        .start
        .unwrap_or_else(|| matching.iter().copied().min().unwrap_or_default());
    let end_tick = filter.end.unwrap_or_else(|| {
        matching
            .iter()
            .copied()
            .max()
            .unwrap_or(start_tick)
            .max(start_tick.saturating_add(1))
    });
    let span = end_tick.saturating_sub(start_tick).max(1);
    let span_i64 =
        i64::try_from(span).map_err(|_| Error::Edit("crescendo span is too large".to_owned()))?;
    let delta = i64::from(end_velocity) - i64::from(start_velocity);

    for editable in notes
        .iter_mut()
        .filter(|editable| !editable.deleted && filter.matches(&editable.note))
    {
        let offset = editable.note.start.saturating_sub(start_tick).min(span);
        let offset_i64 = i64::try_from(offset)
            .map_err(|_| Error::Edit("crescendo offset is too large".to_owned()))?;
        let numerator = delta
            .checked_mul(offset_i64)
            .ok_or_else(|| Error::Edit("crescendo velocity overflowed".to_owned()))?;
        let interpolated = i64::from(start_velocity) + rounded_div_i64(numerator, span_i64);
        editable.note.velocity = clamp_note_on_velocity(interpolated);
    }

    Ok(matching.len())
}

pub(super) fn rounded_div_i64(numerator: i64, denominator: i64) -> i64 {
    if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        (numerator - denominator / 2) / denominator
    }
}

pub(super) fn transpose_key(key: u8, semitones: i16) -> Result<u8> {
    let out_of_range = || Error::Edit(format!("transpose would move key {key} outside 0..127"));
    let transposed = i16::from(key)
        .checked_add(semitones)
        .ok_or_else(out_of_range)?;
    if (0..=127).contains(&transposed) {
        Ok(transposed as u8)
    } else {
        Err(out_of_range())
    }
}

/// Snaps a note to the grid; a note shorter than the grid keeps one grid step.
pub(super) fn quantize_note(note: &mut ConcreteNote, grid: u64, mode: QuantizeMode) -> Result<()> {
    match mode {
        QuantizeMode::Start => note.start = quantize_tick(note.start, grid)?,
        QuantizeMode::Duration if note.duration == 0 => {}
        QuantizeMode::Duration => note.duration = quantize_tick(note.duration, grid)?.max(grid),
        QuantizeMode::Both if note.duration == 0 => note.start = quantize_tick(note.start, grid)?,
        QuantizeMode::Both => {
            let end = note
                .start
                .checked_add(note.duration)
                .ok_or_else(|| Error::Edit("note end tick overflowed u64".to_owned()))?;
            let start = quantize_tick(note.start, grid)?;
            note.start = start;
            note.duration = quantize_tick(end, grid)?.saturating_sub(start).max(grid);
        }
    }
    Ok(())
}

/// Delays every second grid step so a pair splits `amount` percent to the first note.
pub(super) fn apply_swing(
    notes: &mut [EditableNote],
    filter: &NoteFilter,
    amount: u8,
    grid: u64,
) -> Result<usize> {
    let delay = grid
        .checked_mul(2)
        .and_then(|pair| pair.checked_mul(u64::from(amount.saturating_sub(50))))
        .map(|value| (value + 50) / 100)
        .ok_or_else(|| Error::Edit("swing delay overflowed".to_owned()))?;

    let mut matching = matching_notes_mut(notes, filter);
    for editable in &mut matching {
        if !(editable.note.start / grid).is_multiple_of(2) {
            editable.note.start = editable
                .note
                .start
                .checked_add(delay)
                .ok_or_else(|| Error::Edit("swing moved a note past u64 range".to_owned()))?;
        }
    }
    Ok(matching.len())
}

pub(super) fn apply_arpeggiate(
    session: &mut EditSession<'_>,
    filter: &NoteFilter,
    grid: u64,
    order: ArpeggioOrder,
) -> Result<usize> {
    let groups = chord_groups_indices(&session.notes, filter, 2);
    let matched = groups.iter().map(Vec::len).sum();
    for group in groups {
        let base_start = group
            .iter()
            .map(|index| session.notes[*index].note.start)
            .min()
            .unwrap_or_default();
        let mut ascending = group.clone();
        ascending.sort_by_key(|index| (session.notes[*index].note.key, *index));

        let mut sequence = match order {
            ArpeggioOrder::Up | ArpeggioOrder::UpDown => ascending.clone(),
            ArpeggioOrder::Down => ascending.iter().rev().copied().collect(),
            ArpeggioOrder::Outside => outside_in(&ascending),
        };
        if order == ArpeggioOrder::UpDown {
            for index in ascending[1..ascending.len() - 1].iter().rev() {
                let copy = session.notes[*index].note.clone();
                sequence.push(session.add_note(copy));
            }
        }

        for (position, index) in sequence.into_iter().enumerate() {
            session.notes[index].note.start = grid
                .checked_mul(position as u64)
                .and_then(|offset| base_start.checked_add(offset))
                .ok_or_else(|| Error::Edit("arpeggio tick overflowed".to_owned()))?;
        }
    }
    Ok(matched)
}

fn outside_in(ascending: &[usize]) -> Vec<usize> {
    let mut alternated = Vec::with_capacity(ascending.len());
    let (mut left, mut right) = (0, ascending.len());
    while left < right {
        alternated.push(ascending[left]);
        left += 1;
        if left < right {
            right -= 1;
            alternated.push(ascending[right]);
        }
    }
    alternated
}
