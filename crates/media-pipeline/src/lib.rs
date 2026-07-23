//! # media-pipeline
//!
//! Deterministic, pure-Rust audio DSP for Casual Note's meeting-capture path,
//! implementing the signal chain of **Architecture §5** ("Cross-Platform Capture
//! Architecture": *"the `media-pipeline` normalizes identically: monotonic clocks,
//! channel downmix, resample to 16 kHz mono float32, DC removal/gain, VAD, overlapping
//! chunking, drift correction — feeding the STT and LLM schedulers"*) and the media
//! signal path of **HLD §8.4** (*"downmix/resample 16k mono f32, DC/gain, VAD, chunk …
//! overlapping chunks → pass-1 live"*).
//!
//! This crate is the **pure-DSP layer**: it contains no IO, no async, no OS or model
//! FFI. The native lock-free capture ring and STT/LLM backends live in the `capture-*`,
//! `speech-*`, and `llm-*` crates; here the ring is modelled deterministically so the
//! DSP is fully unit-testable against synthetic signals. Every stage is a deterministic
//! function of its input, so a captured track replays bit-identically.
//!
//! ## Signal chain
//!
//! | Stage | Module | Responsibility |
//! |-------|--------|----------------|
//! | Timeline | [`timeline`] | Per-track native timestamps → one monotonic session timeline, with online drift estimation and conservative, clamped correction at safe (silence) boundaries; original timestamp + adjustment retained. |
//! | Downmix | [`downmix`] | Interleaved multi-channel → mono by channel averaging. |
//! | Resample | [`resample`] | Windowed-sinc polyphase resample to 16 kHz mono `f32` (self-contained; anti-aliasing on downsample). |
//! | Normalize | [`normalize`] | DC removal, conservative bounded makeup gain, soft-clip + hard-clamp clipping protection. |
//! | VAD | [`vad`] | Energy + zero-crossing voice-activity detection and overlapping utterance windows (6–12 s window, 1–2 s overlap, configurable). |
//! | Chunk | [`chunk`] | Bounded drop-oldest [`AudioRing`](chunk::AudioRing) with explicit overflow health events, plus [`Chunker`](chunk::Chunker) emitting recoverable [`Chunk`](chunk::Chunk)s + [`InferenceJob`](chunk::InferenceJob)s. |
//! | Pipeline | [`pipeline`] | Composes all of the above for a single capture track. |
//!
//! ## Invariants honored
//!
//! * **No RT-callback sin** (CLAUDE.md / N13): the drop-oldest [`AudioRing`](chunk::AudioRing)
//!   never blocks or grows unbounded; overflow is reported, never silent.
//! * **Capability honesty**: ring overflow surfaces a [`RingOverflow`](chunk::RingOverflow)
//!   health event rather than degrading quietly.
//! * **Deterministic & testable**: no IO/clock reads on the signal path; the timeline
//!   takes its calibration observations from the caller.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod chunk;
pub mod downmix;
pub mod error;
pub mod normalize;
pub mod pipeline;
pub mod resample;
pub mod timeline;
pub mod vad;

// --- Flat re-exports of the most-used items ---
pub use chunk::{AudioRing, Chunk, ChunkOutput, Chunker, InferenceJob, JobKind, RingOverflow};
pub use downmix::downmix_to_mono;
pub use error::{MediaPipelineError, Result};
pub use normalize::{
    remove_dc_mean, soft_clip, DcBlocker, NormalizeConfig, NormalizeStats, Normalizer,
};
pub use pipeline::{Pipeline, PipelineConfig, PipelineOutput};
pub use resample::{
    resample, resample_to_target, Resampler, TARGET_SAMPLE_RATE_HZ, ZERO_CROSSINGS,
};
pub use timeline::{DriftEstimator, MappedTimestamp, TrackTimeline};
pub use vad::{
    frame_features, EnergyVad, UtteranceWindow, UtteranceWindower, VadConfig, VadFrame,
    WindowConfig,
};
