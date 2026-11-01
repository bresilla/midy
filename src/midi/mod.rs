//! Reading, rendering and editing standard MIDI files.

/// Appends one formatted line to a `String`.
macro_rules! emit {
    ($out:expr, $($arg:tt)*) => {{
        use std::fmt::Write as _;
        let _ = writeln!($out, $($arg)*);
    }};
}

mod analysis;
mod arrange;
mod chords;
mod diff;
mod edit;
mod format;
mod lint;
mod model;
mod options;
mod roll;
mod suggest;
mod time;
mod timeline;

pub use analysis::{render_analysis, render_analysis_json, render_tracks};
pub use arrange::{
    extract_selection, merge_midi, merge_midi_report, mute_selection, solo_selection,
    split_selection,
};
pub use chords::render_chords;
pub use diff::render_diff;
pub use edit::{
    apply_edits, apply_edits_with, apply_timeline_csv_edits, apply_timeline_json_edits,
    render_edit_report,
};
pub use format::parse_note_key;
pub use lint::{fix_midi, fix_midi_with_options, lint_report, render_lint};
pub use options::{
    ApplyOptions, ApplyOutput, BasslineOptions, ChordKeep, ChordsOptions, CommandReport,
    DEFAULT_ROLL_COLUMNS, EditFormat, EditReport, EventDetail, FixOptions, QueryOptions,
    ReadOptions, ReduceChordsOptions, RollMode, RollOptions, SplitMode, StuckNoteFixMode,
    TrackChannelSelector,
};
pub use roll::render_roll;
pub use suggest::{suggest_bassline, suggest_reduce_chords};
pub use timeline::{
    render_timeline, render_timeline_csv, render_timeline_csv_with, render_timeline_json,
    render_timeline_json_with, render_timeline_with,
};

use crate::Result;

/// Machine-readable edit command help.
pub const EDIT_FORMAT_HELP: &str = include_str!("schema.txt");

/// Parses a standard MIDI file from memory using `midly`.
pub fn parse_smf(bytes: &[u8]) -> Result<midly::Smf<'_>> {
    Ok(midly::Smf::parse(bytes)?)
}

#[cfg(test)]
mod tests;
