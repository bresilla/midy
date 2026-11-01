//! ASCII piano roll.

use super::format::{note_name, ticks_per_beat};
use super::model::{Note, collect_notes};
use super::options::{RollMode, RollOptions};
use super::parse_smf;
use super::time::{TimeSignatureMap, option_ticks};
use crate::Result;

/// Renders a compact ASCII piano roll.
pub fn render_roll(bytes: &[u8], options: &RollOptions) -> Result<String> {
    let smf = parse_smf(bytes)?;
    let beat_ticks = ticks_per_beat(smf.header.timing);
    let time_map = TimeSignatureMap::from_smf(&smf);
    let query = options.query.resolved(&smf, time_map.as_ref())?;
    let notes = collect_notes(&smf)
        .into_iter()
        .filter(|note| query.matches_note(note))
        .collect::<Vec<_>>();
    let grid = match options.grid.as_deref() {
        Some(value) => option_ticks("grid", value, beat_ticks, time_map.as_ref())?,
        None => beat_ticks.map_or(120, |ticks| u64::from(ticks) / 4).max(1),
    };

    let start = query
        .start
        .or_else(|| notes.iter().map(|note| note.start).min())
        .unwrap_or_default();
    let end = query
        .end
        .or_else(|| notes.iter().map(note_end).max())
        .unwrap_or(start)
        .max(start.saturating_add(grid));
    let total_columns = (end - start).div_ceil(grid);
    let columns = match options.max_columns {
        0 => total_columns,
        limit => total_columns.min(limit),
    };
    let low = notes.iter().map(|note| note.key).min().unwrap_or(60);
    let high = notes.iter().map(|note| note.key).max().unwrap_or(60);
    let mut out = String::new();

    emit!(
        out,
        "MIDY_ROLL v1 start={} end={} grid_ticks={} columns={} total_columns={} truncated={} low={} high={} mode={}",
        start,
        end,
        grid,
        columns,
        total_columns,
        columns < total_columns,
        low,
        high,
        match options.mode {
            RollMode::Compact => "compact",
            RollMode::Verbose => "verbose",
        },
    );

    let width = columns as usize;
    let mut rows = vec![vec![b'.'; width]; 128];
    for note in &notes {
        let first = note.start.saturating_sub(start) / grid;
        let last = note_end(note).saturating_sub(1).max(note.start);
        let last = (last.saturating_sub(start) / grid).min(columns.saturating_sub(1));
        if first < columns && note_end(note) > start {
            rows[usize::from(note.key)][first as usize..=last as usize].fill(b'#');
        }
    }
    for key in (low..=high).rev() {
        emit!(
            out,
            "{:>4} | {} |",
            note_name(key),
            String::from_utf8_lossy(&rows[usize::from(key)])
        );
    }

    if options.mode == RollMode::Verbose {
        for column in 0..columns {
            let cell_start = start + column * grid;
            let cell_end = cell_start.saturating_add(grid);
            let mut active = notes
                .iter()
                .filter(|note| note.start < cell_end && note_end(note) > cell_start)
                .map(|note| format!("{}:{}:{}", note_name(note.key), note.track, note.channel))
                .collect::<Vec<_>>();
            active.sort();
            active.dedup();
            let position = time_map
                .as_ref()
                .map(|map| format!(" pos={}", map.tick_position(cell_start)))
                .unwrap_or_default();
            emit!(
                out,
                "CELL column={} tick={}{} notes={}",
                column,
                cell_start,
                position,
                if active.is_empty() {
                    "-".to_owned()
                } else {
                    active.join(",")
                },
            );
        }
    }

    Ok(out)
}

fn note_end(note: &Note) -> u64 {
    note.start.saturating_add(note.duration)
}
