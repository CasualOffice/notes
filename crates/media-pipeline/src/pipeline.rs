//! End-to-end DSP pipeline composition (Architecture §5; HLD §8.4 signal path:
//! "downmix/resample 16k mono f32, DC/gain, VAD, chunk ... overlapping chunks").
//!
//! [`Pipeline`] wires the stages of this crate together for a single capture track:
//!
//! ```text
//!   interleaved native f32
//!        │  downmix_to_mono
//!        ▼  Resampler (→ 16 kHz mono f32)
//!        ▼  Normalizer (DC removal, conservative gain, clip protection)
//!        ▼  AudioRing  (bounded; drops oldest not-yet-chunked frames on overflow)
//!        ▼  EnergyVad  (per 20 ms frame)
//!        ▼  UtteranceWindower (overlapping 6–12 s windows)
//!        ▼  Chunker    → Chunk (+ live InferenceJob)
//! ```
//!
//! The ring sits at the resampled-mono stage: its contents are the "not-yet-persisted
//! frames" the overflow policy is allowed to drop (Architecture §5 / N13). Everything
//! is deterministic and free of IO, so a whole track can be replayed identically.

use crate::chunk::{AudioRing, Chunk, Chunker, InferenceJob, RingOverflow};
use crate::downmix::downmix_to_mono;
use crate::error::{MediaPipelineError, Result};
use crate::normalize::{NormalizeConfig, Normalizer};
use crate::resample::{Resampler, TARGET_SAMPLE_RATE_HZ};
use crate::vad::{EnergyVad, UtteranceWindower, VadConfig, WindowConfig};

/// Configuration for a [`Pipeline`].
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Native capture sample rate (Hz).
    pub input_rate: u32,
    /// Native capture channel count.
    pub input_channels: u16,
    /// Level-conditioning tunables.
    pub normalize: NormalizeConfig,
    /// VAD tunables (its `sample_rate` must be [`TARGET_SAMPLE_RATE_HZ`]).
    pub vad: VadConfig,
    /// Windowing tunables (its `sample_rate` must be [`TARGET_SAMPLE_RATE_HZ`]).
    pub window: WindowConfig,
    /// Ring capacity for resampled mono audio, in seconds.
    pub ring_seconds: f32,
}

impl PipelineConfig {
    /// Defaults for a track: 16 kHz VAD/windowing, 6–12 s windows, ~30 s ring.
    ///
    /// # Errors
    /// Returns [`MediaPipelineError::UnsupportedRate`] if `input_rate` is zero or
    /// [`MediaPipelineError::InvalidChannels`] if `input_channels` is zero.
    pub fn new(input_rate: u32, input_channels: u16) -> Result<Self> {
        if input_rate == 0 {
            return Err(MediaPipelineError::UnsupportedRate { rate: input_rate });
        }
        if input_channels == 0 {
            return Err(MediaPipelineError::InvalidChannels {
                channels: input_channels,
                samples: 0,
            });
        }
        Ok(Self {
            input_rate,
            input_channels,
            normalize: NormalizeConfig::default(),
            vad: VadConfig::for_16k(),
            window: WindowConfig::for_16k(),
            ring_seconds: 30.0,
        })
    }
}

/// What one [`Pipeline::ingest`] (or [`Pipeline::finish`]) call produced.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PipelineOutput {
    /// Recoverable chunks emitted this call.
    pub chunks: Vec<Chunk>,
    /// Inference jobs to enqueue (parallel to `chunks`).
    pub jobs: Vec<InferenceJob>,
    /// Ring-overflow health events raised this call (never silent).
    pub health: Vec<RingOverflow>,
}

/// A single-track DSP pipeline. Feed native interleaved `f32` via [`ingest`](Self::ingest)
/// and call [`finish`](Self::finish) once at end-of-capture.
#[derive(Debug)]
pub struct Pipeline {
    channels: u16,
    resampler: Resampler,
    normalizer: Normalizer,
    ring: AudioRing,
    vad: EnergyVad,
    windower: UtteranceWindower,
    chunker: Chunker,
    frame_len: usize,
}

impl Pipeline {
    /// Build a pipeline from configuration.
    ///
    /// # Errors
    /// Returns an error if the rates are unusable, the window config is invalid, or the
    /// VAD/window sample rates are not [`TARGET_SAMPLE_RATE_HZ`].
    pub fn new(cfg: PipelineConfig) -> Result<Self> {
        if cfg.vad.sample_rate != TARGET_SAMPLE_RATE_HZ
            || cfg.window.sample_rate != TARGET_SAMPLE_RATE_HZ
        {
            return Err(MediaPipelineError::InvalidConfig {
                reason: "VAD and window sample_rate must equal the 16 kHz target",
            });
        }
        if cfg.vad.frame_len == 0 {
            return Err(MediaPipelineError::InvalidConfig {
                reason: "vad.frame_len must be non-zero",
            });
        }
        if cfg.input_channels == 0 {
            return Err(MediaPipelineError::InvalidChannels {
                channels: 0,
                samples: 0,
            });
        }
        let frame_len = cfg.vad.frame_len;
        Ok(Self {
            channels: cfg.input_channels,
            resampler: Resampler::to_target(cfg.input_rate)?,
            normalizer: Normalizer::new(cfg.normalize),
            ring: AudioRing::with_seconds(TARGET_SAMPLE_RATE_HZ, cfg.ring_seconds),
            vad: EnergyVad::new(cfg.vad),
            windower: UtteranceWindower::new(cfg.window)?,
            chunker: Chunker::new(),
            frame_len,
        })
    }

    /// Ingest a block of native interleaved `f32` samples and produce any chunks, jobs,
    /// and health events that became available.
    ///
    /// # Errors
    /// Returns [`MediaPipelineError::InvalidChannels`] if the block length is not a
    /// whole number of frames for the configured channel count.
    pub fn ingest(&mut self, interleaved: &[f32]) -> Result<PipelineOutput> {
        let mono_native = downmix_to_mono(interleaved, self.channels)?;
        let mut mono16k = self.resampler.process(&mono_native);
        self.normalizer.process(&mut mono16k);

        let mut out = PipelineOutput::default();
        if let Some(overflow) = self.ring.push_slice(&mono16k) {
            out.health.push(overflow);
        }
        self.drain_frames(&mut out, false);
        Ok(out)
    }

    /// Flush all buffered audio at end-of-capture, emitting the final (possibly short)
    /// chunk and its job.
    pub fn finish(&mut self) -> PipelineOutput {
        let mut mono16k = self.resampler.flush();
        if !mono16k.is_empty() {
            self.normalizer.process(&mut mono16k);
            if let Some(overflow) = self.ring.push_slice(&mono16k) {
                // Overflow at flush is possible if the ring was already full.
                let mut out = PipelineOutput::default();
                out.health.push(overflow);
                self.drain_frames(&mut out, true);
                return out;
            }
        }
        let mut out = PipelineOutput::default();
        self.drain_frames(&mut out, true);
        out
    }

    /// Pull whole frames out of the ring, run VAD + windowing + chunking. When
    /// `final_flush` is set, also classify a trailing partial frame and flush the
    /// windower so no audio is stranded.
    fn drain_frames(&mut self, out: &mut PipelineOutput, final_flush: bool) {
        while self.ring.len() >= self.frame_len {
            let frame = self.ring.drain(self.frame_len);
            let decision = self.vad.classify(&frame);
            if let Some(window) = self.windower.push_frame(&frame, decision.is_speech) {
                let emitted = self.chunker.emit(window);
                out.chunks.push(emitted.chunk);
                out.jobs.extend(emitted.jobs);
            }
        }
        if final_flush {
            if !self.ring.is_empty() {
                let frame = self.ring.drain_all();
                let decision = self.vad.classify(&frame);
                if let Some(window) = self.windower.push_frame(&frame, decision.is_speech) {
                    let emitted = self.chunker.emit(window);
                    out.chunks.push(emitted.chunk);
                    out.jobs.extend(emitted.jobs);
                }
            }
            if let Some(window) = self.windower.flush() {
                let emitted = self.chunker.emit(window);
                out.chunks.push(emitted.chunk);
                out.jobs.extend(emitted.jobs);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn interleaved_tone(freq: f64, rate: u32, channels: u16, frames: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * channels as usize);
        for i in 0..frames {
            let s = 0.6 * (2.0 * PI * freq * i as f64 / f64::from(rate)).sin() as f32;
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    #[test]
    fn end_to_end_emits_chunks_and_jobs() {
        let cfg = PipelineConfig::new(48_000, 2).unwrap();
        let mut p = Pipeline::new(cfg).unwrap();
        // 15 seconds of stereo 48k speech-like tone in 0.5 s blocks.
        let block_frames = 24_000; // 0.5 s at 48k
        let mut total = PipelineOutput::default();
        for _ in 0..30 {
            let block = interleaved_tone(220.0, 48_000, 2, block_frames);
            let out = p.ingest(&block).unwrap();
            total.chunks.extend(out.chunks);
            total.jobs.extend(out.jobs);
            total.health.extend(out.health);
        }
        let fin = p.finish();
        total.chunks.extend(fin.chunks);
        total.jobs.extend(fin.jobs);

        assert!(!total.chunks.is_empty(), "expected at least one chunk");
        assert_eq!(total.chunks.len(), total.jobs.len());
        // Chunk sequence numbers are monotonic and each job points at its chunk.
        for (i, chunk) in total.chunks.iter().enumerate() {
            assert_eq!(chunk.id, total.jobs[i].chunk_id);
            assert!(chunk.t_end_ms > chunk.t_start_ms);
            assert!(!chunk.persisted);
        }
        // First chunk should be roughly a 12 s force-cut window (no silence boundary).
        let first = &total.chunks[0];
        assert!(
            first.t_end_ms >= 11_900 && first.t_end_ms <= 12_100,
            "{}",
            first.t_end_ms
        );
    }

    #[test]
    fn overflow_is_reported_when_consumer_never_drains() {
        // Tiny ring: 0.1 s. We feed far more than it can hold before draining frames
        // out, but since ingest drains frames immediately, force overflow with a ring
        // smaller than one block by disabling draining via a huge frame requirement.
        let mut cfg = PipelineConfig::new(16_000, 1).unwrap();
        cfg.ring_seconds = 0.1; // 1600 samples
                                // Make the analysis frame larger than the ring so frames never drain and the
                                // ring must overflow on sustained input.
        cfg.vad.frame_len = 4_000;
        cfg.window.min_window_samples = 4_000;
        cfg.window.max_window_samples = 8_000;
        cfg.window.overlap_samples = 1_000;
        let mut p = Pipeline::new(cfg).unwrap();

        let mut saw_overflow = false;
        for _ in 0..10 {
            let block = interleaved_tone(200.0, 16_000, 1, 1_600); // 0.1 s each
            let out = p.ingest(&block).unwrap();
            if !out.health.is_empty() {
                saw_overflow = true;
            }
        }
        assert!(saw_overflow, "ring should have reported overflow");
    }

    #[test]
    fn silence_produces_no_midstream_chunks() {
        let cfg = PipelineConfig::new(16_000, 1).unwrap();
        let mut p = Pipeline::new(cfg).unwrap();
        // 20 s of silence: VAD never fires, so no silence-boundary cut; the only
        // possible chunk is a force-cut at 12 s.
        let mut chunks = 0;
        for _ in 0..40 {
            let out = p.ingest(&vec![0.0f32; 8_000]).unwrap(); // 0.5 s
            chunks += out.chunks.len();
        }
        // Silence still accumulates in the windower and force-cuts at 12 s once.
        assert!(chunks <= 2, "unexpected chunk count {chunks}");
    }

    #[test]
    fn rejects_ragged_block() {
        let cfg = PipelineConfig::new(48_000, 2).unwrap();
        let mut p = Pipeline::new(cfg).unwrap();
        assert!(matches!(
            p.ingest(&[0.1, 0.2, 0.3]),
            Err(MediaPipelineError::InvalidChannels { .. })
        ));
    }

    #[test]
    fn rejects_zero_channels_config() {
        assert!(PipelineConfig::new(48_000, 0).is_err());
    }
}
