//! Builds MIDI test data with `midly`, independent of the code under test.
#![allow(dead_code)]

use midly::{
    Format, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent, TrackEventKind,
    num::{u4, u7, u14, u15, u24, u28},
};

pub const PPQ: u16 = 96;

/// One track described by absolute ticks.
#[derive(Default)]
pub struct Track<'a> {
    events: Vec<(u64, TrackEventKind<'a>)>,
    end: u64,
}

impl<'a> Track<'a> {
    pub fn event(mut self, tick: u64, kind: TrackEventKind<'a>) -> Self {
        self.events.push((tick, kind));
        self
    }

    pub fn meta(self, tick: u64, meta: MetaMessage<'a>) -> Self {
        self.event(tick, TrackEventKind::Meta(meta))
    }

    pub fn midi(self, tick: u64, channel: u8, message: MidiMessage) -> Self {
        self.event(
            tick,
            TrackEventKind::Midi {
                channel: u4::from(channel),
                message,
            },
        )
    }

    pub fn on(self, tick: u64, channel: u8, key: u8, vel: u8) -> Self {
        self.midi(
            tick,
            channel,
            MidiMessage::NoteOn {
                key: u7::from(key),
                vel: u7::from(vel),
            },
        )
    }

    pub fn off(self, tick: u64, channel: u8, key: u8) -> Self {
        self.midi(
            tick,
            channel,
            MidiMessage::NoteOff {
                key: u7::from(key),
                vel: u7::from(64),
            },
        )
    }

    pub fn note(self, tick: u64, duration: u64, channel: u8, key: u8, vel: u8) -> Self {
        self.on(tick, channel, key, vel)
            .off(tick + duration, channel, key)
    }

    pub fn cc(self, tick: u64, channel: u8, controller: u8, value: u8) -> Self {
        self.midi(
            tick,
            channel,
            MidiMessage::Controller {
                controller: u7::from(controller),
                value: u7::from(value),
            },
        )
    }

    pub fn program(self, tick: u64, channel: u8, program: u8) -> Self {
        self.midi(
            tick,
            channel,
            MidiMessage::ProgramChange {
                program: u7::from(program),
            },
        )
    }

    pub fn bend(self, tick: u64, channel: u8, value: u16) -> Self {
        self.midi(
            tick,
            channel,
            MidiMessage::PitchBend {
                bend: PitchBend(u14::from(value)),
            },
        )
    }

    pub fn tempo(self, tick: u64, micros: u32) -> Self {
        self.meta(tick, MetaMessage::Tempo(u24::from(micros)))
    }

    pub fn time_signature(self, tick: u64, numerator: u8, power: u8) -> Self {
        self.meta(tick, MetaMessage::TimeSignature(numerator, power, 24, 8))
    }

    pub fn name(self, tick: u64, name: &'a [u8]) -> Self {
        self.meta(tick, MetaMessage::TrackName(name))
    }

    pub fn end(mut self, tick: u64) -> Self {
        self.end = tick;
        self
    }

    fn build(mut self) -> Vec<TrackEvent<'a>> {
        self.events.sort_by_key(|(tick, _)| *tick);
        let last = self.events.last().map_or(0, |(tick, _)| *tick);
        let mut previous = 0;
        let mut track = Vec::new();
        for (tick, kind) in self.events {
            track.push(TrackEvent {
                delta: u28::from((tick - previous) as u32),
                kind,
            });
            previous = tick;
        }
        track.push(TrackEvent {
            delta: u28::from((self.end.max(last) - previous) as u32),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        track
    }
}

pub fn track<'a>() -> Track<'a> {
    Track::default()
}

pub fn midi(tracks: Vec<Track<'_>>) -> Vec<u8> {
    let format = if tracks.len() > 1 {
        Format::Parallel
    } else {
        Format::SingleTrack
    };
    let smf = Smf {
        header: Header {
            format,
            timing: Timing::Metrical(u15::from(PPQ)),
        },
        tracks: tracks.into_iter().map(Track::build).collect(),
    };
    let mut out = Vec::new();
    smf.write_std(&mut out).expect("writing to memory");
    out
}

/// A small arrangement with the event types a real song tends to contain.
pub fn band() -> Vec<u8> {
    midi(vec![
        track()
            .name(0, b"Conductor")
            .tempo(0, 500_000)
            .time_signature(0, 4, 2)
            .meta(0, MetaMessage::KeySignature(0, false))
            .meta(0, MetaMessage::Marker(b"Verse 1"))
            .tempo(768, 400_000)
            .meta(384, MetaMessage::Text(b"quote \" and = sign"))
            .end(1536),
        track()
            .name(0, b"Piano Left")
            .program(0, 0, 4)
            .cc(0, 0, 64, 127)
            .note(0, 96, 0, 60, 80)
            .note(0, 96, 0, 64, 80)
            .note(0, 96, 0, 67, 80)
            .note(384, 96, 0, 65, 76)
            .note(384, 96, 0, 69, 76)
            .note(384, 96, 0, 72, 76)
            .cc(760, 0, 64, 0),
        track()
            .name(0, b"Bass")
            .program(0, 1, 33)
            .note(0, 192, 1, 36, 90)
            .note(384, 192, 1, 41, 90)
            .bend(400, 1, 9000),
        track()
            .name(0, b"Drums")
            .note(0, 24, 9, 36, 110)
            .note(0, 24, 9, 42, 70)
            .note(96, 24, 9, 42, 70)
            .note(192, 24, 9, 38, 100)
            .note(192, 24, 9, 42, 70),
    ])
}
