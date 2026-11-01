//! Tokenizing edit lines and reading their `key=value` fields.
//!
//! [`Fields`] records every name a command asks for, so anything the command
//! never asked for can be reported as an unknown field.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use crate::midi::format::{parse_note_key, unescape_ascii};
use crate::midi::time::{Span, TimeSignatureMap};
use crate::text::closest_match;
use crate::{Error, Result};

/// File facts needed to resolve musical values.
#[derive(Debug, Clone, Copy)]
pub(in crate::midi) struct EditContext<'a> {
    pub(in crate::midi) ticks_per_beat: Option<u16>,
    pub(in crate::midi) time_map: Option<&'a TimeSignatureMap>,
}

#[derive(Debug)]
pub(super) struct Fields<'a> {
    label: String,
    values: HashMap<String, String>,
    positional: Vec<String>,
    asked: RefCell<HashSet<String>>,
    positional_used: Cell<usize>,
    context: EditContext<'a>,
}

/// Splits a line on whitespace outside double quotes, dropping any comment.
pub(super) fn tokenize(line: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut previous_was_space = true;

    for character in line.chars() {
        if in_quotes {
            if escaped {
                escaped = false;
                token.push(character);
            } else if character == '\\' {
                escaped = true;
                token.push(character);
            } else if character == '"' {
                in_quotes = false;
            } else {
                token.push(character);
            }
            continue;
        }
        if character == '#' && previous_was_space {
            break;
        }
        previous_was_space = character.is_whitespace();
        if character == '"' {
            in_quotes = true;
        } else if character.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(character);
        }
    }

    if in_quotes {
        return Err(Error::Edit("unterminated quoted value".to_owned()));
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    Ok(tokens)
}

pub(super) fn normalize_key(value: &str) -> String {
    value.to_ascii_lowercase().replace('-', "_")
}

impl<'a> Fields<'a> {
    pub(super) fn from_tokens(
        label: String,
        tokens: &[String],
        context: EditContext<'a>,
    ) -> Result<Self> {
        let mut values = HashMap::new();
        let mut positional = Vec::new();
        for token in tokens {
            match token.split_once('=') {
                Some((key, value)) => {
                    let key = normalize_key(key);
                    if values.insert(key.clone(), value.to_owned()).is_some() {
                        return Err(Error::Edit(format!("{label}: duplicate field '{key}'")));
                    }
                }
                None => positional.push(token.clone()),
            }
        }
        Ok(Self::new(label, values, positional, context))
    }

    pub(super) fn from_row(
        label: String,
        values: HashMap<String, String>,
        context: EditContext<'a>,
    ) -> Self {
        Self::new(label, values, Vec::new(), context)
    }

    fn new(
        label: String,
        values: HashMap<String, String>,
        positional: Vec<String>,
        context: EditContext<'a>,
    ) -> Self {
        Self {
            label,
            values,
            positional,
            asked: RefCell::new(HashSet::new()),
            positional_used: Cell::new(0),
            context,
        }
    }

    pub(super) fn error(&self, message: impl std::fmt::Display) -> Error {
        Error::Edit(format!("{}: {message}", self.label))
    }

    pub(super) fn raw(&self, key: &str) -> Option<&str> {
        self.asked.borrow_mut().insert(key.to_owned());
        self.values.get(key).map(String::as_str)
    }

    /// Accepts fields whose values are not used.
    pub(super) fn ignore(&self, keys: &[&str]) {
        self.asked
            .borrow_mut()
            .extend(keys.iter().map(|key| (*key).to_owned()));
    }

    /// Reads a field that may be spelled several ways; spellings must agree.
    pub(super) fn alias<T: PartialEq>(
        &self,
        keys: &[&str],
        parse: impl Fn(&Self, &str, &str) -> Result<T>,
    ) -> Result<Option<T>> {
        let mut found = None::<(&str, T)>;
        for key in keys {
            let Some(value) = self.raw(key) else {
                continue;
            };
            let parsed = parse(self, key, value)?;
            match &found {
                Some((first, first_value)) if *first_value != parsed => {
                    return Err(self.error(format!("fields '{first}' and '{key}' disagree")));
                }
                Some(_) => {}
                None => found = Some((key, parsed)),
            }
        }
        Ok(found.map(|(_, value)| value))
    }

    pub(super) fn missing(&self, key: &str) -> Error {
        self.error(format!("missing field '{key}'"))
    }

    pub(super) fn usize(&self, keys: &[&str]) -> Result<Option<usize>> {
        self.alias(keys, |fields, key, value| {
            value
                .parse::<usize>()
                .map_err(|_| fields.error(format!("field '{key}' must be an integer")))
        })
    }

    pub(super) fn u64(&self, keys: &[&str]) -> Result<Option<u64>> {
        self.alias(keys, |fields, key, value| {
            value
                .parse::<u64>()
                .map_err(|_| fields.error(format!("field '{key}' must be an integer")))
        })
    }

    pub(super) fn bounded(&self, keys: &[&str], max: u8) -> Result<Option<u8>> {
        self.alias(keys, |fields, key, value| match value.parse::<u64>() {
            Ok(parsed) if parsed <= u64::from(max) => Ok(parsed as u8),
            Ok(_) => Err(fields.error(format!("field '{key}' must be 0..{max}"))),
            Err(_) => Err(fields.error(format!("field '{key}' must be an integer"))),
        })
    }

    pub(super) fn channel(&self, keys: &[&str]) -> Result<Option<u8>> {
        self.bounded(keys, 15)
    }

    pub(super) fn u7(&self, keys: &[&str]) -> Result<Option<u8>> {
        self.bounded(keys, 127)
    }

    /// Reads a note-on velocity, where zero would mean note-off.
    pub(super) fn velocity(&self, keys: &[&str]) -> Result<Option<u8>> {
        let velocity = self.u7(keys)?;
        if velocity == Some(0) {
            return Err(self.error(format!(
                "field '{}' must be 1..127; velocity 0 would turn the note off",
                keys[0]
            )));
        }
        Ok(velocity)
    }

    pub(super) fn i16(&self, keys: &[&str]) -> Result<Option<i16>> {
        self.alias(keys, |fields, key, value| {
            value
                .parse::<i16>()
                .map_err(|_| fields.error(format!("field '{key}' must be a signed integer")))
        })
    }

    pub(super) fn key(&self, keys: &[&str]) -> Result<Option<u8>> {
        self.alias(keys, |fields, key, value| {
            parse_note_key(value)
                .map_err(|message| fields.error(format!("field '{key}' {message}")))
        })
    }

    /// Reads a tick count or `BAR:BEAT[:TICK]` position.
    pub(super) fn time(&self, keys: &[&str]) -> Result<Option<u64>> {
        self.alias(keys, |fields, key, value| fields.time_value(key, value))
    }

    fn time_value(&self, key: &str, value: &str) -> Result<u64> {
        if !value.contains(':') {
            return value
                .parse::<u64>()
                .map_err(|_| self.error(format!("field '{key}' must be an integer")));
        }
        let time_map = self.context.time_map.ok_or_else(|| {
            self.error(format!(
                "field '{key}' uses musical position but MIDI timing is not metrical"
            ))
        })?;
        let parts = value.split(':').collect::<Vec<_>>();
        if !(2..=3).contains(&parts.len()) {
            return Err(self.error(format!("field '{key}' must be BAR:BEAT or BAR:BEAT:TICK")));
        }
        let part = |text: &str| {
            text.parse::<u64>()
                .map_err(|_| self.error(format!("field '{key}' must be BAR:BEAT or BAR:BEAT:TICK")))
        };
        let bar = part(parts[0])?;
        let beat = part(parts[1])?;
        let tick = parts.get(2).map_or(Ok(0), |text| part(text))?;
        if bar == 0 || beat == 0 {
            return Err(self.error(format!("field '{key}' bar and beat are 1-based")));
        }
        time_map
            .position_to_tick(bar, beat, tick)
            .map_err(|message| self.error(format!("field '{key}' {message}")))
    }

    pub(super) fn span(&self, keys: &[&str]) -> Result<Option<Span>> {
        self.alias(keys, |fields, key, value| {
            Span::parse(value, fields.context.ticks_per_beat)
                .map_err(|message| fields.error(format!("field '{key}' {message}")))
        })
    }

    pub(super) fn resolve_span(&self, span: Span, anchor: u64) -> Result<u64> {
        span.ticks_at(self.context.time_map, anchor)
            .map_err(|message| self.error(message))
    }

    pub(super) fn signed_span(&self, keys: &[&str]) -> Result<Option<(bool, Span)>> {
        self.alias(keys, |fields, key, value| {
            let (negative, value) = match value.strip_prefix('-') {
                Some(value) => (true, value),
                None => (false, value.strip_prefix('+').unwrap_or(value)),
            };
            Span::parse(value, fields.context.ticks_per_beat)
                .map(|span| (negative, span))
                .map_err(|message| fields.error(format!("field '{key}' {message}")))
        })
    }

    pub(super) fn bar_range(&self, keys: &[&str]) -> Result<Option<(u64, u64)>> {
        self.alias(keys, |fields, key, value| {
            let time_map = fields.context.time_map.ok_or_else(|| {
                fields.error(format!("field '{key}' requires metrical MIDI timing"))
            })?;
            time_map
                .bar_range_ticks(value)
                .map_err(|error| fields.error(error))
        })
    }

    pub(super) fn boolean(&self, keys: &[&str]) -> Result<Option<bool>> {
        self.alias(keys, |fields, key, value| match value {
            "true" | "yes" | "1" => Ok(true),
            "false" | "no" | "0" => Ok(false),
            _ => Err(fields.error(format!("field '{key}' must be true or false"))),
        })
    }

    pub(super) fn text(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.raw(key)
            .map(|value| {
                unescape_ascii(value)
                    .map_err(|message| self.error(format!("field '{key}' {message}")))
            })
            .transpose()
    }

    /// Reads a note id from `id=` or the first bare word.
    pub(super) fn id(&self) -> Result<String> {
        if let Some(id) = self.raw("id") {
            return Ok(id.to_owned());
        }
        match self.positional.first() {
            Some(id) => {
                self.positional_used.set(1);
                Ok(id.clone())
            }
            None => Err(self.error("missing note id")),
        }
    }

    /// Rejects whatever the command did not ask for.
    pub(super) fn finish(&self, command: &str) -> Result<()> {
        if let Some(extra) = self.positional.get(self.positional_used.get()) {
            return Err(self.error(format!(
                "{command} does not take the bare value '{extra}'; write fields as name=value"
            )));
        }
        let asked = self.asked.borrow();
        let mut unknown = self
            .values
            .keys()
            .filter(|key| !asked.contains(*key))
            .collect::<Vec<_>>();
        unknown.sort();
        let Some(first) = unknown.first() else {
            return Ok(());
        };
        let mut known = asked.iter().map(String::as_str).collect::<Vec<_>>();
        known.sort_unstable();
        let hint = closest_match(first, known.iter().copied())
            .map(|name| format!("; did you mean '{name}'?"))
            .unwrap_or_default();
        Err(self.error(format!(
            "{command} does not accept field '{first}'{hint} Accepted: {}",
            known.join(", ")
        )))
    }
}
