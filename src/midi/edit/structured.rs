//! Note rows from `midy read --format json|csv`.

use std::collections::{HashMap, HashSet};

use midly::Smf;
use serde_json::Value;

use crate::midi::format::parse_csv_row;
use crate::midi::model::collect_notes;
use crate::midi::options::{ApplyOptions, EditFormat};
use crate::text::closest_match;
use crate::{Error, Result};

use super::apply::{Membership, Plan};
use super::apply_edits_with;
use super::command::{EditCommand, LineCommand};
use super::fields::{EditContext, Fields};
use super::parse::{parse_add_note, parse_note_row, parse_scope};

const ROW_FIELDS: &[&str] = &[
    "id", "track", "ch", "key", "name", "start", "dur", "end", "vel", "off_vel", "scope",
];
const REQUIRED_COLUMNS: &[&str] = &["id", "track", "ch", "key", "start", "dur", "vel"];

/// Applies note rows from `midy read --format json`.
pub fn apply_timeline_json_edits(bytes: &[u8], json: &str) -> Result<Vec<u8>> {
    Ok(apply_edits_with(bytes, json, EditFormat::Json, ApplyOptions::default())?.bytes)
}

/// Applies note rows from `midy read --format csv`.
pub fn apply_timeline_csv_edits(bytes: &[u8], csv: &str) -> Result<Vec<u8>> {
    Ok(apply_edits_with(bytes, csv, EditFormat::Csv, ApplyOptions::default())?.bytes)
}

/// Resolves [`EditFormat::Auto`] by looking at the first meaningful text.
pub(super) fn detect_format(format: EditFormat, text: &str) -> EditFormat {
    if format != EditFormat::Auto {
        return format;
    }
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return EditFormat::Json;
    }
    let first_line = trimmed.lines().next().unwrap_or_default();
    let is_csv_header = !first_line.contains('=')
        && first_line
            .split(',')
            .any(|column| column.trim().trim_matches('"') == "id")
        && first_line.contains(',');
    if is_csv_header {
        EditFormat::Csv
    } else {
        EditFormat::Text
    }
}

struct Rows {
    rows: Vec<(String, HashMap<String, String>)>,
    scope: Option<String>,
}

pub(super) fn parse_rows(
    smf: &Smf<'_>,
    text: &str,
    format: EditFormat,
    context: EditContext<'_>,
) -> Result<Plan> {
    let parsed = match format {
        EditFormat::Json => json_rows(text)?,
        _ => csv_rows(text)?,
    };

    let notes = collect_notes(smf);
    let known = notes
        .iter()
        .map(|note| note.id.as_str())
        .collect::<HashSet<_>>();
    let mut listed = HashSet::new();
    let mut commands = Vec::with_capacity(parsed.rows.len());

    for (index, (label, values)) in parsed.rows.into_iter().enumerate() {
        let fields = Fields::from_row(label, values, context);
        let id = fields
            .raw("id")
            .map(str::trim)
            .filter(|id| !id.is_empty() && *id != "-")
            .map(ToOwned::to_owned);
        fields.ignore(&["scope"]);

        let (name, command) = match id {
            Some(id) if known.contains(id.as_str()) => {
                if !listed.insert(id.clone()) {
                    return Err(fields.error(format!("duplicate note id '{id}'")));
                }
                let row = parse_note_row(&fields)?;
                ("NOTE", EditCommand::Note { id, row })
            }
            _ => {
                fields.ignore(&["name", "end"]);
                ("ADD_NOTE", EditCommand::Add(parse_add_note(&fields)?))
            }
        };
        fields.finish(name)?;
        commands.push(LineCommand {
            line: index + 1,
            name,
            command,
        });
    }

    let scope = parsed
        .scope
        .map(|scope| parse_scope(&scope, context))
        .transpose()?
        .unwrap_or_default();
    Ok(Plan {
        commands,
        membership: Some(Membership {
            scope,
            listed,
            delete_missing: true,
        }),
        warnings: Vec::new(),
    })
}

fn json_rows(text: &str) -> Result<Rows> {
    let document = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
        .map_err(|error| Error::Edit(format!("invalid timeline JSON: {error}")))?;
    let (notes, scope) = match &document {
        Value::Array(rows) => (rows, None),
        Value::Object(object) => {
            let notes = object
                .get("notes")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::Edit("timeline JSON has no \"notes\" array".to_owned()))?;
            (notes, object.get("scope").and_then(Value::as_str))
        }
        _ => {
            return Err(Error::Edit(
                "timeline JSON must be an object with \"notes\" or an array of rows".to_owned(),
            ));
        }
    };
    if notes.is_empty() {
        return Err(Error::Edit(
            "timeline JSON did not contain any note rows".to_owned(),
        ));
    }

    let mut rows = Vec::with_capacity(notes.len());
    for (index, row) in notes.iter().enumerate() {
        let label = format!("timeline JSON note {}", index + 1);
        let object = row
            .as_object()
            .ok_or_else(|| Error::Edit(format!("{label}: must be an object")))?;
        let mut values = HashMap::with_capacity(object.len());
        for (name, value) in object {
            if !ROW_FIELDS.contains(&name.as_str()) {
                let hint = closest_match(name, ROW_FIELDS.iter().copied())
                    .map(|known| format!("; did you mean '{known}'?"))
                    .unwrap_or_default();
                return Err(Error::Edit(format!(
                    "{label}: unknown field '{name}'{hint}"
                )));
            }
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Number(number) => number.to_string(),
                Value::Null => continue,
                other => {
                    return Err(Error::Edit(format!(
                        "{label}: field '{name}' must be a number or string, got {other}"
                    )));
                }
            };
            values.insert(name.clone(), text);
        }
        if !values.contains_key("id") {
            values.insert("id".to_owned(), String::new());
        }
        rows.push((label, values));
    }

    Ok(Rows {
        rows,
        scope: scope.map(ToOwned::to_owned),
    })
}

fn csv_rows(text: &str) -> Result<Rows> {
    let mut lines = text
        .trim_start_matches('\u{feff}')
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let (_, header) = lines
        .next()
        .ok_or_else(|| Error::Edit("timeline CSV is empty".to_owned()))?;
    let headers = parse_csv_row(header)
        .into_iter()
        .map(|name| name.trim().to_owned())
        .collect::<Vec<_>>();
    for name in REQUIRED_COLUMNS {
        if !headers.iter().any(|header| header == name) {
            return Err(Error::Edit(format!(
                "timeline CSV is missing required column '{name}'"
            )));
        }
    }

    let mut rows = Vec::new();
    let mut scope = None;
    for (index, line) in lines {
        let label = format!("timeline CSV line {}", index + 1);
        let cells = parse_csv_row(line);
        if cells.len() > headers.len() {
            return Err(Error::Edit(format!("{label}: has too many columns")));
        }
        let mut values = HashMap::new();
        for (name, cell) in headers.iter().zip(cells) {
            if name == "scope" {
                if !cell.is_empty() {
                    scope.get_or_insert(cell);
                }
            } else if ROW_FIELDS.contains(&name.as_str()) && !cell.is_empty() {
                values.insert(name.clone(), cell);
            }
        }
        rows.push((label, values));
    }
    if rows.is_empty() {
        return Err(Error::Edit(
            "timeline CSV did not contain any note rows".to_owned(),
        ));
    }
    Ok(Rows { rows, scope })
}
