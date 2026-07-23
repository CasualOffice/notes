//! Voice-activity detection and overlapping utterance windowing (Architecture §5:
//! "VAD maintaining overlapping utterance windows (live window 6-12 s, overlap
//! 1-2 s, configurable)").
//!
//! Two cooperating pieces, both operating on 16 kHz mono `f32`:
//!
//! * [`EnergyVad`] — a per-frame classifier combining short-time **RMS energy** with
//!   the **zero-crossing rate** (ZCR). High energy with a bounded ZCR marks voiced /
//!   most speech; high-ZCR hiss and low-energy silence are rejected. Onset and
//!   hangover counters debounce the decision so single-frame dropouts inside speech
//!   do not fragment an utterance.
//! * [`UtteranceWindower`] — accumulates the mono stream and emits overlapping
//!   windows of `min..=max` seconds. A window is cut at a silence boundary once it is
//!   at least `min` long, or force-cut at `max`. The trailing `overlap` seconds are
//!   carried into the next window so an utterance straddling a cut is never lost.

/// Result of classifying one analysis frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadFrame {
    /// Debounced speech decision for this frame.
    pub is_speech: bool,
    /// Short-time RMS energy of the frame.
    pub rms: f32,
    /// Zero-crossing rate (crossings / sample) of the frame.
    pub zcr: f32,
}

/// Tunables for [`EnergyVad`]. Durations are expressed in analysis frames.
#[derive(Debug, Clone, Copy)]
pub struct VadConfig {
    /// Working sample rate (16 kHz downstream).
    pub sample_rate: u32,
    /// Samples per analysis frame (e.g. 20 ms -> 320 samples at 16 kHz).
    pub frame_len: usize,
    /// RMS above which a frame is a speech candidate.
    pub rms_threshold: f32,
    /// Reject frames whose ZCR exceeds this (fricative hiss / high-frequency noise).
    pub zcr_max: f32,
    /// Consecutive candidate frames required to switch into the speech state.
    pub onset_frames: usize,
    /// Frames of grace kept as speech after activity drops (utterance tail).
    pub hangover_frames: usize,
}

impl VadConfig {
    /// Sensible defaults for 16 kHz speech: 20 ms frames, ~0.02 RMS gate.
    #[must_use]
    pub fn for_16k() -> Self {
        Self {
            sample_rate: 16_000,
            frame_len: 320, // 20 ms
            rms_threshold: 0.02,
            zcr_max: 0.35,
            onset_frames: 2,
            hangover_frames: 12, // ~240 ms tail
        }
    }
}

/// Debounced energy + zero-crossing voice-activity detector.
#[derive(Debug, Clone)]
pub struct EnergyVad {
    cfg: VadConfig,
    active: bool,
    onset_count: usize,
    hangover_count: usize,
}

impl EnergyVad {
    /// Create a detector with the given configuration.
    #[must_use]
    pub fn new(cfg: VadConfig) -> Self {
        Self {
            cfg,
            active: false,
            onset_count: 0,
            hangover_count: 0,
        }
    }

    /// The configured samples-per-frame.
    #[must_use]
    pub fn frame_len(&self) -> usize {
        self.cfg.frame_len
    }

    /// Classify one frame of exactly [`frame_len`](Self::frame_len) samples (a shorter
    /// trailing frame is still accepted). Returns the debounced decision plus the raw
    /// features that drove it.
    pub fn classify(&mut self, frame: &[f32]) -> VadFrame {
        let (rms, zcr) = frame_features(frame);
        let candidate = rms >= self.cfg.rms_threshold && zcr <= self.cfg.zcr_max;

        if candidate {
            self.onset_count = self.onset_count.saturating_add(1);
            self.hangover_count = self.cfg.hangover_frames;
            if self.onset_count >= self.cfg.onset_frames {
                self.active = true;
            }
        } else {
            self.onset_count = 0;
            if self.hangover_count > 0 {
                self.hangover_count -= 1;
            } else {
                self.active = false;
            }
        }

        VadFrame {
            is_speech: self.active,
            rms,
            zcr,
        }
    }

    /// Reset the detector state (e.g. at a new session).
    pub fn reset(&mut self) {
        self.active = false;
        self.onset_count = 0;
        self.hangover_count = 0;
    }
}

/// Compute (RMS, zero-crossing rate) for a frame. ZCR is crossings divided by the
/// number of adjacent-sample transitions, so it is in `[0, 1]`.
#[must_use]
pub fn frame_features(frame: &[f32]) -> (f32, f32) {
    if frame.is_empty() {
        return (0.0, 0.0);
    }
    // Branchless sign in {-1, 0, 1} without float equality (avoids float_cmp) and
    // without an if/else-if chain (avoids comparison_chain).
    let sign_of = |x: f32| -> i8 { i8::from(x > 0.0) - i8::from(x < 0.0) };
    let mut sumsq = 0.0f64;
    let mut crossings = 0usize;
    let mut prev_sign = sign_of(frame[0]);
    for &s in frame {
        sumsq += f64::from(s) * f64::from(s);
        let sign = sign_of(s);
        // A zero sample is not a crossing on its own; carry the previous sign forward.
        if sign != 0 {
            if sign != prev_sign && prev_sign != 0 {
                crossings += 1;
            }
            prev_sign = sign;
        }
    }
    let rms = (sumsq / frame.len() as f64).sqrt() as f32;
    let denom = (frame.len() - 1).max(1) as f32;
    (rms, crossings as f32 / denom)
}

/// A cut, time-anchored utterance window ready for STT.
#[derive(Debug, Clone, PartialEq)]
pub struct UtteranceWindow {
    /// Monotonic window sequence number within the session (0-based).
    pub seq: u64,
    /// Session-relative start time in milliseconds.
    pub t_start_ms: i64,
    /// Session-relative end time in milliseconds (exclusive).
    pub t_end_ms: i64,
    /// Mono 16 kHz samples spanning `[t_start_ms, t_end_ms)`, including the leading
    /// overlap carried from the previous window.
    pub samples: Vec<f32>,
    /// Whether the window ended at a detected silence boundary (`true`) or was
    /// force-cut at the maximum length (`false`).
    pub cut_at_silence: bool,
}

/// Tunables for [`UtteranceWindower`].
#[derive(Debug, Clone, Copy)]
pub struct WindowConfig {
    /// Working sample rate.
    pub sample_rate: u32,
    /// Minimum window length in samples before a silence boundary may cut it.
    pub min_window_samples: usize,
    /// Maximum window length in samples; a window is force-cut here.
    pub max_window_samples: usize,
    /// Trailing samples carried into the next window as leading overlap.
    pub overlap_samples: usize,
}

impl WindowConfig {
    /// Build a config from second-valued live-window parameters.
    ///
    /// # Errors
    /// Returns [`InvalidConfig`](crate::error::MediaPipelineError::InvalidConfig) if
    /// `min > max`, `max == 0`, or `overlap >= max`.
    pub fn from_seconds(
        sample_rate: u32,
        min_s: f32,
        max_s: f32,
        overlap_s: f32,
    ) -> crate::error::Result<Self> {
        let s = |secs: f32| (secs.max(0.0) * sample_rate as f32).round() as usize;
        let cfg = Self {
            sample_rate,
            min_window_samples: s(min_s),
            max_window_samples: s(max_s),
            overlap_samples: s(overlap_s),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// The Architecture default: 6 s min, 12 s max, 1.5 s overlap at 16 kHz.
    #[must_use]
    pub fn for_16k() -> Self {
        Self {
            sample_rate: 16_000,
            min_window_samples: 6 * 16_000,
            max_window_samples: 12 * 16_000,
            overlap_samples: (1.5 * 16_000.0) as usize,
        }
    }

    fn validate(&self) -> crate::error::Result<()> {
        use crate::error::MediaPipelineError::InvalidConfig;
        if self.max_window_samples == 0 {
            return Err(InvalidConfig {
                reason: "max_window_samples must be non-zero",
            });
        }
        if self.min_window_samples > self.max_window_samples {
            return Err(InvalidConfig {
                reason: "min_window_samples must not exceed max_window_samples",
            });
        }
        if self.overlap_samples >= self.max_window_samples {
            return Err(InvalidConfig {
                reason: "overlap_samples must be smaller than max_window_samples",
            });
        }
        Ok(())
    }
}

/// Accumulates the classified mono stream and emits overlapping utterance windows.
#[derive(Debug, Clone)]
pub struct UtteranceWindower {
    cfg: WindowConfig,
    /// Pending samples for the current window (may already include carried overlap).
    buf: Vec<f32>,
    /// Session-relative sample index at which `buf[0]` sits.
    buf_start_sample: u64,
    /// Whether the most-recent frame classified as speech.
    in_speech: bool,
    seq: u64,
}

impl UtteranceWindower {
    /// Create a windower.
    ///
    /// # Errors
    /// Propagates [`WindowConfig::validate`] failures.
    pub fn new(cfg: WindowConfig) -> crate::error::Result<Self> {
        cfg.validate()?;
        Ok(Self {
            cfg,
            buf: Vec::new(),
            buf_start_sample: 0,
            in_speech: false,
            seq: 0,
        })
    }

    fn sample_to_ms(&self, sample: u64) -> i64 {
        // ms = sample * 1000 / rate, computed in i128 to avoid overflow on long runs.
        ((i128::from(sample) * 1000) / i128::from(self.cfg.sample_rate)) as i64
    }

    /// Push one classified frame's samples. Returns a window if the append completed
    /// one (at most one per call). `is_speech` is the debounced decision from
    /// [`EnergyVad::classify`] for the same frame.
    pub fn push_frame(&mut self, samples: &[f32], is_speech: bool) -> Option<UtteranceWindow> {
        self.buf.extend_from_slice(samples);
        let was_speech = self.in_speech;
        self.in_speech = is_speech;

        // Force-cut at the hard maximum regardless of activity.
        if self.buf.len() >= self.cfg.max_window_samples {
            return Some(self.cut(false));
        }
        // Otherwise cut on a falling edge (speech -> silence) once long enough.
        let silence_boundary = was_speech && !is_speech;
        if silence_boundary && self.buf.len() >= self.cfg.min_window_samples {
            return Some(self.cut(true));
        }
        None
    }

    /// Emit any remaining buffered audio as a final (possibly short) window at
    /// end-of-stream. Unlike a mid-stream cut, no overlap is carried — the buffer is
    /// fully drained. Returns `None` if nothing is buffered.
    pub fn flush(&mut self) -> Option<UtteranceWindow> {
        if self.buf.is_empty() {
            return None;
        }
        let start_sample = self.buf_start_sample;
        let end_sample = start_sample + self.buf.len() as u64;
        let samples = std::mem::take(&mut self.buf);
        self.buf_start_sample = end_sample;
        let window = UtteranceWindow {
            seq: self.seq,
            t_start_ms: self.sample_to_ms(start_sample),
            t_end_ms: self.sample_to_ms(end_sample),
            samples,
            cut_at_silence: true,
        };
        self.seq += 1;
        Some(window)
    }

    /// Cut the current buffer into a window, retaining the trailing overlap as the
    /// seed of the next window.
    fn cut(&mut self, cut_at_silence: bool) -> UtteranceWindow {
        let start_sample = self.buf_start_sample;
        let end_sample = start_sample + self.buf.len() as u64;
        let overlap = self.cfg.overlap_samples.min(self.buf.len());
        let tail_start = self.buf.len() - overlap;

        let samples = self.buf.clone();
        // Retain the last `overlap` samples for the next window.
        let carried: Vec<f32> = self.buf[tail_start..].to_vec();
        self.buf = carried;
        // The carried overlap's session-relative start.
        self.buf_start_sample = end_sample - overlap as u64;

        let window = UtteranceWindow {
            seq: self.seq,
            t_start_ms: self.sample_to_ms(start_sample),
            t_end_ms: self.sample_to_ms(end_sample),
            samples,
            cut_at_silence,
        };
        self.seq += 1;
        window
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn tone(freq: f64, amp: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    #[test]
    fn features_of_silence_and_tone() {
        let (rms_s, _) = frame_features(&vec![0.0f32; 320]);
        assert!(rms_s < 1e-6);
        let (rms_t, zcr_t) = frame_features(&tone(200.0, 0.5, 16_000, 320));
        assert!(rms_t > 0.3); // 0.5 amplitude sine -> ~0.354 RMS
        assert!(zcr_t > 0.0 && zcr_t < 0.1); // low-frequency tone -> low ZCR
    }

    #[test]
    fn vad_flags_speech_not_silence() {
        let mut vad = EnergyVad::new(VadConfig::for_16k());
        // Silence stays non-speech.
        for _ in 0..10 {
            assert!(!vad.classify(&vec![0.0f32; 320]).is_speech);
        }
        // A loud tone crosses onset after `onset_frames` and latches speech.
        let sp = tone(220.0, 0.6, 16_000, 320);
        let mut became = false;
        for _ in 0..5 {
            if vad.classify(&sp).is_speech {
                became = true;
            }
        }
        assert!(became, "VAD never detected the tone as speech");
    }

    #[test]
    fn vad_rejects_high_zcr_noise() {
        let mut vad = EnergyVad::new(VadConfig::for_16k());
        // Alternating +/- full scale -> maximal ZCR, energetic but not speech-like.
        let hiss: Vec<f32> = (0..320)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let mut any = false;
        for _ in 0..10 {
            if vad.classify(&hiss).is_speech {
                any = true;
            }
        }
        assert!(!any, "high-ZCR noise should be rejected");
    }

    #[test]
    fn window_force_cut_at_max() {
        // min 6s, max 12s, overlap 1.5s at 16k.
        let cfg = WindowConfig::for_16k();
        let mut w = UtteranceWindower::new(cfg).unwrap();
        // Feed 12s of continuous speech in 20ms frames without a silence boundary.
        let frame = vec![0.3f32; 320];
        let mut emitted = None;
        // 12s / 20ms = 600 frames reaches the max.
        for _ in 0..600 {
            if let Some(win) = w.push_frame(&frame, true) {
                emitted = Some(win);
                break;
            }
        }
        let win = emitted.expect("a window should be force-cut at max length");
        assert!(!win.cut_at_silence);
        assert_eq!(win.samples.len(), cfg.max_window_samples);
        assert_eq!(win.t_start_ms, 0);
        assert_eq!(win.t_end_ms, 12_000);
    }

    #[test]
    fn window_cut_at_silence_after_min() {
        let cfg = WindowConfig::from_seconds(16_000, 6.0, 12.0, 1.5).unwrap();
        let mut w = UtteranceWindower::new(cfg).unwrap();
        let frame = vec![0.3f32; 320];
        // 7s of speech (350 frames), then one silent frame -> falling edge.
        for _ in 0..350 {
            assert!(w.push_frame(&frame, true).is_none());
        }
        let silence = vec![0.0f32; 320];
        let win = w
            .push_frame(&silence, false)
            .expect("silence boundary after min length should cut");
        assert!(win.cut_at_silence);
        // 350 speech frames + 1 silence frame = 351 * 320 samples.
        assert_eq!(win.samples.len(), 351 * 320);
        assert_eq!(win.t_start_ms, 0);
    }

    #[test]
    fn windows_overlap_by_configured_amount() {
        let cfg = WindowConfig::from_seconds(16_000, 1.0, 2.0, 0.5).unwrap();
        let mut w = UtteranceWindower::new(cfg).unwrap();
        let frame = vec![0.3f32; 320];
        // Drive two force-cuts (2s each = 100 frames).
        let mut windows = Vec::new();
        for _ in 0..250 {
            if let Some(win) = w.push_frame(&frame, true) {
                windows.push(win);
            }
        }
        assert!(windows.len() >= 2);
        // The second window starts `overlap` (0.5s = 8000 samples) before the first
        // window's end.
        let first = &windows[0];
        let second = &windows[1];
        let overlap_ms = first.t_end_ms - second.t_start_ms;
        assert_eq!(overlap_ms, 500);
        assert_eq!(second.seq, first.seq + 1);
    }

    #[test]
    fn flush_emits_trailing_audio() {
        let cfg = WindowConfig::from_seconds(16_000, 1.0, 2.0, 0.5).unwrap();
        let mut w = UtteranceWindower::new(cfg).unwrap();
        w.push_frame(&vec![0.2f32; 320], true);
        let win = w.flush().expect("trailing buffer should flush");
        assert_eq!(win.samples.len(), 320);
        assert!(w.flush().is_none());
    }

    #[test]
    fn invalid_window_config_rejected() {
        assert!(WindowConfig::from_seconds(16_000, 5.0, 2.0, 0.5).is_err()); // min>max
        assert!(WindowConfig::from_seconds(16_000, 1.0, 2.0, 2.0).is_err()); // overlap>=max
    }
}
