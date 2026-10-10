//! The two-tone "ding" played when a bet hits the target multiplier.

use std::time::Duration;

use rodio::source::{SineWave, Source};
use rodio::{OutputStream, OutputStreamHandle, Sink};

pub struct Sound {
    // The stream must stay alive for as long as we want to play anything.
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
}

impl Sound {
    pub fn new() -> Self {
        match OutputStream::try_default() {
            Ok((stream, handle)) => Self { _stream: Some(stream), handle: Some(handle) },
            Err(e) => {
                tracing::warn!("no audio output: {e}");
                Self { _stream: None, handle: None }
            }
        }
    }

    pub fn ding(&self) {
        let Some(handle) = &self.handle else { return };
        let Ok(sink) = Sink::try_new(handle) else { return };
        let first = SineWave::new(880.0).take_duration(Duration::from_millis(90)).fade_in(Duration::from_millis(8)).amplify(0.18);
        let second = SineWave::new(1320.0).take_duration(Duration::from_millis(420)).fade_in(Duration::from_millis(8)).amplify(0.16);
        sink.append(first);
        sink.append(second);
        sink.detach();
    }
}
