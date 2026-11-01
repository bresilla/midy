//! Text edits: parsing, the in-memory session, and transforms.

mod apply;
mod command;
mod fields;
mod parse;
mod session;
mod structured;
mod transform;

pub use apply::{apply_edits, apply_edits_with, render_edit_report};
pub use structured::{apply_timeline_csv_edits, apply_timeline_json_edits};

pub(super) use session::{EditSession, EditableNote, TimedEvent, finish_track};
