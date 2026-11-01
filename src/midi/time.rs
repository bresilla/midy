//! Time-signature map, musical positions and meter-aware lengths.

use midly::{MetaMessage, Smf};

use super::format::ticks_per_beat;
use super::model::meta_events;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct TimeSignatureValue {
    pub(super) numerator: u8,
    pub(super) denominator: u32,
}

impl Default for TimeSignatureValue {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct TimeSignatureSegment {
    pub(super) start_tick: u64,
    pub(super) start_bar: u64,
    pub(super) signature: TimeSignatureValue,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct TimeSignatureMap {
    pub(super) ticks_per_quarter: u16,
    pub(super) segments: Vec<TimeSignatureSegment>,
}

impl TimeSignatureMap {
    pub(super) fn from_smf(smf: &Smf<'_>) -> Option<Self> {
        let ticks_per_quarter = ticks_per_beat(smf.header.timing)?;
        let mut changes = meta_events(smf)
            .into_iter()
            .filter_map(|(_, tick, meta)| {
                if let MetaMessage::TimeSignature(numerator, denominator_power, _, _) = meta {
                    TimeSignatureValue::from_meta(numerator, denominator_power)
                        .map(|signature| (tick, signature))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        changes.sort_by_key(|(tick, _)| *tick);

        let mut map = Self {
            ticks_per_quarter,
            segments: vec![TimeSignatureSegment {
                start_tick: 0,
                start_bar: 1,
                signature: TimeSignatureValue::default(),
            }],
        };

        for (tick, signature) in changes {
            if let Some(last) = map.segments.last_mut()
                && last.start_tick == tick
            {
                last.signature = signature;
                continue;
            }

            let last = *map
                .segments
                .last()
                .expect("time signature map always has a default segment");
            let delta = tick.saturating_sub(last.start_tick);
            let bar_ticks = map.bar_ticks_for(last.signature);
            let bars_advanced = if delta == 0 {
                0
            } else {
                delta.div_ceil(bar_ticks)
            };
            map.segments.push(TimeSignatureSegment {
                start_tick: tick,
                start_bar: last.start_bar.saturating_add(bars_advanced),
                signature,
            });
        }

        Some(map)
    }

    pub(super) fn beat_ticks_at(&self, tick: u64) -> u64 {
        self.beat_ticks_for(self.segment_for_tick(tick).signature)
    }

    pub(super) fn bar_ticks_at(&self, tick: u64) -> u64 {
        self.bar_ticks_for(self.segment_for_tick(tick).signature)
    }

    pub(super) fn tick_position(&self, tick: u64) -> String {
        let segment = self.segment_for_tick(tick);
        let delta = tick.saturating_sub(segment.start_tick);
        let beat_ticks = self.beat_ticks_for(segment.signature);
        let bar_ticks = beat_ticks.saturating_mul(u64::from(segment.signature.numerator));
        let bar = segment.start_bar + delta / bar_ticks;
        let tick_in_bar = delta % bar_ticks;
        let beat = tick_in_bar / beat_ticks + 1;
        let tick_in_beat = tick_in_bar % beat_ticks;

        format!("{bar}:{beat}:{tick_in_beat}")
    }

    pub(super) fn position_to_tick(
        &self,
        bar: u64,
        beat: u64,
        tick: u64,
    ) -> std::result::Result<u64, String> {
        if bar == 0 || beat == 0 {
            return Err("bar and beat are 1-based".to_owned());
        }
        let segment = self.segment_for_bar(bar)?;
        if beat > u64::from(segment.signature.numerator) {
            return Err(format!(
                "beat {beat} exceeds time signature {}/{} at bar {bar}",
                segment.signature.numerator, segment.signature.denominator
            ));
        }
        let beat_ticks = self.beat_ticks_for(segment.signature);
        if tick >= beat_ticks {
            return Err(format!(
                "tick-in-beat must be less than beat length {beat_ticks}"
            ));
        }
        let bar_start = self
            .bar_start_tick(bar)
            .map_err(|error| error.to_string())?;
        bar_start
            .checked_add((beat - 1).saturating_mul(beat_ticks))
            .and_then(|base| base.checked_add(tick))
            .ok_or_else(|| "position overflowed".to_owned())
    }

    pub(super) fn bar_range_ticks(&self, value: &str) -> Result<(u64, u64)> {
        let parse_bar = |raw: &str| {
            raw.parse::<u64>()
                .map_err(|_| Error::Usage(format!("invalid bar number '{raw}'")))
                .and_then(|bar| {
                    if bar == 0 {
                        Err(Error::Usage("bar numbers are 1-based".to_owned()))
                    } else {
                        Ok(bar)
                    }
                })
        };

        if let Some((start, end)) = value.split_once("..") {
            let start = parse_bar(start)?;
            let end = parse_bar(end)?;
            if start > end {
                return Err(Error::Usage("--bars start must be <= end".to_owned()));
            }
            let end_exclusive = end
                .checked_add(1)
                .ok_or_else(|| Error::Usage("--bars end overflowed".to_owned()))?;
            Ok((
                self.bar_start_tick(start)?,
                self.bar_start_tick(end_exclusive)?,
            ))
        } else {
            let bar = parse_bar(value)?;
            let end_exclusive = bar
                .checked_add(1)
                .ok_or_else(|| Error::Usage("--bars end overflowed".to_owned()))?;
            Ok((
                self.bar_start_tick(bar)?,
                self.bar_start_tick(end_exclusive)?,
            ))
        }
    }

    pub(super) fn bar_start_tick(&self, bar: u64) -> Result<u64> {
        if bar == 0 {
            return Err(Error::Usage("bar numbers are 1-based".to_owned()));
        }
        let segment = self
            .segment_for_bar(bar)
            .map_err(|message| Error::Usage(message.to_owned()))?;
        let bars_after_segment_start = bar.saturating_sub(segment.start_bar);
        let bar_ticks = self.bar_ticks_for(segment.signature);
        segment
            .start_tick
            .checked_add(bars_after_segment_start.saturating_mul(bar_ticks))
            .ok_or_else(|| Error::Usage("bar range overflowed".to_owned()))
    }

    pub(super) fn segment_for_tick(&self, tick: u64) -> TimeSignatureSegment {
        let index = self
            .segments
            .iter()
            .rposition(|segment| segment.start_tick <= tick)
            .unwrap_or(0);
        self.segments[index]
    }

    pub(super) fn segment_for_bar(
        &self,
        bar: u64,
    ) -> std::result::Result<TimeSignatureSegment, String> {
        let Some(index) = self
            .segments
            .iter()
            .rposition(|segment| segment.start_bar <= bar)
        else {
            return Err(format!("bar {bar} is before the first bar"));
        };
        Ok(self.segments[index])
    }

    pub(super) fn beat_ticks_for(&self, signature: TimeSignatureValue) -> u64 {
        let quarter_ticks = u64::from(self.ticks_per_quarter);
        let denominator = u64::from(signature.denominator);
        ((quarter_ticks * 4 + denominator / 2) / denominator).max(1)
    }

    pub(super) fn bar_ticks_for(&self, signature: TimeSignatureValue) -> u64 {
        self.beat_ticks_for(signature)
            .saturating_mul(u64::from(signature.numerator))
            .max(1)
    }
}

impl TimeSignatureValue {
    pub(super) fn from_meta(numerator: u8, denominator_power: u8) -> Option<Self> {
        if numerator == 0 || denominator_power > 31 {
            return None;
        }
        Some(Self {
            numerator,
            denominator: 1_u32 << denominator_power,
        })
    }
}

pub(super) fn note_position_fields(
    start: u64,
    end: u64,
    time_map: Option<&TimeSignatureMap>,
) -> String {
    let Some(time_map) = time_map else {
        return String::new();
    };

    format!(
        " pos={} end_pos={}",
        time_map.tick_position(start),
        time_map.tick_position(end),
    )
}

/// A length in ticks, or a musical unit whose size depends on the active meter.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum Span {
    Ticks(u64),
    Beat,
    Bar,
}

impl Span {
    /// Parses ticks, `N/D`, the triplet form `N/Dt`, `beat`, or `bar`.
    pub(super) fn parse(
        value: &str,
        ticks_per_beat: Option<u16>,
    ) -> std::result::Result<Self, String> {
        let metrical = || ticks_per_beat.ok_or_else(|| "requires metrical MIDI timing".to_owned());
        match value.to_ascii_lowercase().as_str() {
            "beat" => metrical().map(|_| Self::Beat),
            "bar" => metrical().map(|_| Self::Bar),
            text if text.contains('/') => {
                fraction_ticks(text, u64::from(metrical()?)).map(Self::Ticks)
            }
            text => text
                .parse::<u64>()
                .map(Self::Ticks)
                .map_err(|_| "must be ticks, a fraction like 1/8 or 1/8t, beat, or bar".to_owned()),
        }
    }

    /// Converts to ticks using the meter active at `anchor`.
    pub(super) fn ticks_at(
        self,
        time_map: Option<&TimeSignatureMap>,
        anchor: u64,
    ) -> std::result::Result<u64, String> {
        match (self, time_map) {
            (Self::Ticks(ticks), _) => Ok(ticks),
            (Self::Beat, Some(map)) => Ok(map.beat_ticks_at(anchor)),
            (Self::Bar, Some(map)) => Ok(map.bar_ticks_at(anchor)),
            (_, None) => Err("requires metrical MIDI timing".to_owned()),
        }
    }
}

fn fraction_ticks(value: &str, ticks_per_beat: u64) -> std::result::Result<u64, String> {
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or_else(|| "must be a fraction".to_owned())?;
    let (denominator, triplet) = match denominator.strip_suffix('t') {
        Some(denominator) => (denominator, true),
        None => (denominator, false),
    };
    let invalid = || "must be a fraction like 1/8 or 1/8t".to_owned();
    let numerator = numerator.parse::<u64>().map_err(|_| invalid())?;
    let denominator = denominator.parse::<u64>().map_err(|_| invalid())?;
    if numerator == 0 || denominator == 0 {
        return Err("fraction must be greater than zero".to_owned());
    }
    let (scale, divide) = if triplet { (2, 3) } else { (1, 1) };
    let overflow = || "fraction overflowed".to_owned();
    let top = ticks_per_beat
        .checked_mul(4)
        .and_then(|ticks| ticks.checked_mul(numerator))
        .and_then(|ticks| ticks.checked_mul(scale))
        .ok_or_else(overflow)?;
    let bottom = denominator.checked_mul(divide).ok_or_else(overflow)?;
    let rounded = top.checked_add(bottom / 2).ok_or_else(overflow)?;
    Ok((rounded / bottom).max(1))
}

/// Resolves a `--grid`/`--dur` style option value to ticks.
pub(super) fn option_ticks(
    name: &str,
    value: &str,
    ticks_per_beat: Option<u16>,
    time_map: Option<&TimeSignatureMap>,
) -> Result<u64> {
    let ticks = Span::parse(value, ticks_per_beat)
        .and_then(|span| span.ticks_at(time_map, 0))
        .map_err(|message| Error::Usage(format!("--{name} {message}")))?;
    if ticks == 0 {
        return Err(Error::Usage(format!("--{name} must be greater than zero")));
    }
    Ok(ticks)
}
