//! What a hit plays: the two-tone "ding", or a file the user picked in Settings → Sound
//! (any audio file, or a video for its sound).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rodio::source::{SineWave, Source};
use rodio::{OutputStream, OutputStreamHandle, Sink};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// What the file picker offers. Videos (mp4, m4v, mov) play their AAC or ALAC sound.
pub const EXTENSIONS: &[&str] = &["mp3", "mp4", "m4a", "m4v", "mov", "aac", "wav", "ogg", "flac"];

const NO_SOUND: &str = "it has no sound FastBet can read";

pub struct Sound {
    // The stream must stay alive for as long as we want to play anything.
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    /// The user's file while it plays: the next hit starts it over.
    file: Arc<Mutex<Option<Sink>>>,
}

impl Sound {
    pub fn new() -> Self {
        match OutputStream::try_default() {
            Ok((stream, handle)) => Self { _stream: Some(stream), handle: Some(handle), file: Arc::default() },
            Err(e) => {
                tracing::warn!("no audio output: {e}");
                Self { _stream: None, handle: None, file: Arc::default() }
            }
        }
    }

    /// Plays `file`, or the ding when there is none or it can't be played.
    pub fn play(&self, file: Option<PathBuf>) {
        let Some(handle) = self.handle.clone() else { return };
        let Some(path) = file else { return ding(&handle) };
        let playing = self.file.clone();
        // opening the file and decoding its first samples stays off the UI thread
        std::thread::spawn(move || match FileSource::open(&path) {
            Ok(src) => {
                let Ok(sink) = Sink::try_new(&handle) else { return };
                sink.append(src);
                if let Some(old) = playing.lock().replace(sink) {
                    old.stop();
                }
            }
            Err(e) => {
                tracing::warn!("hit sound {}: {e}", path.display());
                ding(&handle);
            }
        });
    }

    /// The user's file is playing.
    pub fn playing(&self) -> bool {
        self.file.lock().as_ref().is_some_and(|s| !s.empty())
    }

    pub fn stop(&self) {
        if let Some(s) = self.file.lock().take() {
            s.stop();
        }
    }
}

fn ding(handle: &OutputStreamHandle) {
    let Ok(sink) = Sink::try_new(handle) else { return };
    let first = SineWave::new(880.0).take_duration(Duration::from_millis(90)).fade_in(Duration::from_millis(8)).amplify(0.18);
    let second = SineWave::new(1320.0).take_duration(Duration::from_millis(420)).fade_in(Duration::from_millis(8)).amplify(0.16);
    sink.append(first);
    sink.append(second);
    sink.detach();
}

/// Whether `path` plays, and how long it is when the file says.
pub fn check(path: &Path) -> Result<Option<Duration>, String> {
    FileSource::open(path).map(|s| s.duration)
}

/// A sound file, or a video's sound, decoded as it plays. Symphonia straight on the file, not
/// rodio's decoder: that one hides the file's length from symphonia (some mp4s then fail) and
/// panics on the seek error it gets.
struct FileSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    /// The last packet's samples, interleaved, and how many of them were played.
    buf: Option<SampleBuffer<f32>>,
    pos: usize,
    channels: u16,
    rate: u32,
    duration: Option<Duration>,
}

impl FileSource {
    fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "it isn't there anymore".to_string(),
            _ => e.to_string(),
        })?;
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let format = symphonia::default::get_probe().format(&hint, stream, &opts, &MetadataOptions::default()).map_err(|_| NO_SOUND)?.format;
        // a video's first track is usually the picture: take the first one with a sound we can decode
        let codecs = symphonia::default::get_codecs();
        let (track, decoder, duration) = format
            .tracks()
            .iter()
            .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .find_map(|t| {
                let decoder = codecs.make(&t.codec_params, &DecoderOptions::default()).ok()?;
                let p = &t.codec_params;
                let duration = p.time_base.zip(p.n_frames).map(|(base, n)| base.calc_time(n)).map(|t| Duration::from_secs_f64(t.seconds as f64 + t.frac));
                Some((t.id, decoder, duration))
            })
            .ok_or(NO_SOUND)?;
        let mut src = Self { format, decoder, track, buf: None, pos: 0, channels: 0, rate: 0, duration };
        // the first samples now, so a file that can't be decoded fails here and not in silence later
        if !src.decode_next() {
            return Err(NO_SOUND.into());
        }
        Ok(src)
    }

    /// Decodes the track's next packet into `buf`; false at the end, or on an error that ends it.
    fn decode_next(&mut self) -> bool {
        let mut damaged = 0;
        loop {
            let Ok(packet) = self.format.next_packet() else { return false };
            if packet.track_id() != self.track {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(audio) => {
                    if audio.frames() == 0 {
                        continue;
                    }
                    let spec = *audio.spec();
                    let need = audio.frames() * spec.channels.count();
                    let fits = self.buf.as_ref().is_some_and(|b| b.capacity() >= need) && self.channels as usize == spec.channels.count() && self.rate == spec.rate;
                    if !fits {
                        self.buf = Some(SampleBuffer::new(audio.capacity() as u64, spec));
                    }
                    let Some(buf) = self.buf.as_mut() else { return false };
                    buf.copy_interleaved_ref(audio);
                    self.channels = spec.channels.count() as u16;
                    self.rate = spec.rate;
                    self.pos = 0;
                    return true;
                }
                // a damaged packet: skip it, but not forever
                Err(Error::DecodeError(_)) if damaged < 8 => damaged += 1,
                Err(_) => return false,
            }
        }
    }

    fn left(&self) -> usize {
        self.buf.as_ref().map_or(0, |b| b.len() - self.pos)
    }
}

impl Iterator for FileSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = *self.buf.as_ref()?.samples().get(self.pos)?;
        self.pos += 1;
        // decode ahead, so `current_frame_len` is only 0 at the very end
        if self.left() == 0 && !self.decode_next() {
            self.buf = None;
        }
        Some(sample)
    }
}

impl Source for FileSource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.left())
    }

    fn channels(&self) -> u16 {
        self.channels.max(1)
    }

    fn sample_rate(&self) -> u32 {
        self.rate.max(1)
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono 16-bit WAV of `n` samples at 8 kHz.
    fn wav(n: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend((36 + n * 2).to_le_bytes());
        v.extend(b"WAVEfmt ");
        for x in [16u32, 1 | 1 << 16, 8000, 16000, 2 | 16 << 16] {
            v.extend(x.to_le_bytes());
        }
        v.extend(b"data");
        v.extend((n * 2).to_le_bytes());
        for i in 0..n {
            v.extend((((i % 100) as i16 - 50) * 300).to_le_bytes());
        }
        v
    }

    /// Plays `src` the way rodio reads it: a frame of `current_frame_len` samples at a time.
    fn drain(mut src: FileSource) -> usize {
        let mut played = 0;
        while let Some(n) = src.current_frame_len().filter(|n| *n > 0) {
            played += src.by_ref().take(n).count();
        }
        assert_eq!(src.next(), None);
        played
    }

    #[test]
    fn a_file_plays_to_the_end() {
        let path = std::env::temp_dir().join(format!("fastbet-sound-{}.wav", std::process::id()));
        std::fs::write(&path, wav(12_000)).unwrap();
        let src = FileSource::open(&path).unwrap();
        assert_eq!((src.channels(), src.sample_rate()), (1, 8000));
        assert_eq!(check(&path), Ok(Some(Duration::from_millis(1500))));
        assert_eq!(drain(src), 12_000);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn says_why_a_file_wont_play() {
        let dir = std::env::temp_dir();
        assert_eq!(check(&dir.join("fastbet-no-such-sound.mp3")), Err("it isn't there anymore".into()));
        let junk = dir.join(format!("fastbet-junk-{}.mp4", std::process::id()));
        std::fs::write(&junk, b"not a video at all").unwrap();
        assert_eq!(check(&junk), Err(NO_SOUND.into()));
        std::fs::remove_file(&junk).unwrap();
    }

    /// A file of your own, decoded to the end without playing it:
    /// `FASTBET_SOUND_FILE=<path> cargo test own_file -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn own_file() {
        let path = PathBuf::from(std::env::var("FASTBET_SOUND_FILE").expect("FASTBET_SOUND_FILE"));
        let src = FileSource::open(&path).unwrap();
        let (ch, rate, len) = (src.channels(), src.sample_rate(), src.duration);
        let n = drain(src);
        println!("{ch} ch, {rate} Hz, {len:?} said, {:.2}s decoded", n as f64 / (ch as f64 * rate as f64));
    }
}
