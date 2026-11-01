//! Names, escaping and number formats shared by renderers and parsers.

use midly::{Format, Timing};

use crate::{Error, Result};

const PITCH_CLASSES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub(super) fn format_name(format: Format) -> &'static str {
    match format {
        Format::SingleTrack => "single",
        Format::Parallel => "parallel",
        Format::Sequential => "sequential",
    }
}

pub(super) fn timing_fields(timing: Timing) -> String {
    match timing {
        Timing::Metrical(ticks) => format!("timing=metrical ticks_per_beat={}", ticks.as_int()),
        Timing::Timecode(fps, subframes) => {
            format!(
                "timing=timecode fps={} ticks_per_frame={}",
                fps.as_int(),
                subframes
            )
        }
    }
}

pub(super) fn ticks_per_beat(timing: Timing) -> Option<u16> {
    match timing {
        Timing::Metrical(ticks) if ticks.as_int() > 0 => Some(ticks.as_int()),
        Timing::Metrical(_) => None,
        Timing::Timecode(_, _) => None,
    }
}

pub(super) fn pc_name(pc: u8) -> &'static str {
    PITCH_CLASSES[usize::from(pc % 12)]
}

pub(super) fn note_name(key: u8) -> String {
    format!("{}{}", pc_name(key), i16::from(key) / 12 - 1)
}

/// Parses a MIDI key given as a number or as a note name such as `C4` or `Bb2`.
pub fn parse_note_key(value: &str) -> std::result::Result<u8, String> {
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return match value.parse::<u8>() {
            Ok(key) if key <= 127 => Ok(key),
            _ => Err("must be 0..127".to_owned()),
        };
    }

    let mut chars = value.chars();
    let Some(letter) = chars.next() else {
        return Err("cannot be empty".to_owned());
    };
    let base_pc = match letter.to_ascii_uppercase() {
        'C' => 0_i16,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err("must be 0..127 or a note name like C4/F#3/Bb2".to_owned()),
    };

    let rest = chars.as_str();
    let (accidental, octave_text) = if let Some(octave) = rest.strip_prefix('#') {
        (1_i16, octave)
    } else if let Some(octave) = rest
        .strip_prefix('b')
        .or_else(|| rest.strip_prefix('B'))
        .or_else(|| rest.strip_prefix('♭'))
    {
        (-1_i16, octave)
    } else {
        (0_i16, rest)
    };

    if octave_text.is_empty() {
        return Err("note name must include an octave, for example C4".to_owned());
    }
    let octave = octave_text
        .parse::<i16>()
        .map_err(|_| "has an invalid octave".to_owned())?;
    let midi_key = octave
        .checked_add(1)
        .and_then(|octave| octave.checked_mul(12))
        .and_then(|base| base.checked_add(base_pc + accidental))
        .ok_or_else(|| "note name overflowed".to_owned())?;
    if (0..=127).contains(&midi_key) {
        Ok(midi_key as u8)
    } else {
        Err("note name is outside MIDI range 0..127".to_owned())
    }
}

pub(super) fn quote_ascii(bytes: &[u8]) -> String {
    let mut quoted = String::from("\"");
    for byte in bytes {
        for escaped in std::ascii::escape_default(*byte) {
            quoted.push(char::from(escaped));
        }
    }
    quoted.push('"');
    quoted
}

/// Reverses the escapes written by [`quote_ascii`].
pub(super) fn unescape_ascii(value: &str) -> std::result::Result<Vec<u8>, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte != b'\\' {
            out.push(byte);
            continue;
        }
        let escape = *bytes
            .get(index)
            .ok_or_else(|| "ends with a lone backslash".to_owned())?;
        index += 1;
        match escape {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'\\' | b'"' | b'\'' => out.push(escape),
            b'x' => {
                let digits = bytes
                    .get(index..index + 2)
                    .and_then(|digits| std::str::from_utf8(digits).ok())
                    .ok_or_else(|| "has an incomplete \\x escape".to_owned())?;
                let value = u8::from_str_radix(digits, 16)
                    .map_err(|_| "has an invalid \\x escape".to_owned())?;
                out.push(value);
                index += 2;
            }
            other => return Err(format!("has an unknown escape '\\{}'", char::from(other))),
        }
    }
    Ok(out)
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn hex_decode(value: &str) -> std::result::Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) || !value.is_ascii() {
        return Err("must be an even number of hex digits".to_owned());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "must contain only hex digits".to_owned())
        })
        .collect()
}

pub(super) fn json_string(value: &str) -> String {
    serde_json::Value::from(value).to_string()
}

pub(super) fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

pub(super) fn parse_csv_row(line: &str) -> Vec<String> {
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(character) = chars.next() {
        if quoted {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(character);
            }
        } else if character == '"' && field.is_empty() {
            quoted = true;
        } else if character == ',' {
            row.push(std::mem::take(&mut field));
        } else {
            field.push(character);
        }
    }
    row.push(field);
    row
}

pub(super) fn checked_delta(delta: u64) -> Result<midly::num::u28> {
    use midly::num::u28;
    if delta <= u64::from(u28::max_value().as_int()) {
        Ok(u28::from(delta as u32))
    } else {
        Err(Error::Edit(format!(
            "delta {delta} is too large for a MIDI variable-length tick value"
        )))
    }
}
