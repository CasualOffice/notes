//! # speech-api — two-pass speech-to-text contract
//!
//! Defines the [`SpeechEngine`] trait and its value types, implementing the
//! **HLD §9.2 Speech trait** (two-pass live + final decode, model profiles) and
//! the segment model of **Data Model §8.3 (`transcript_segment`)** — the atomic
//! unit of evidence for meeting artifacts.
//!
//! Two passes: [`SpeechEngine::transcribe_live`] streams low-latency *partial*
//! hypotheses (`pass = live`, ~1-2 s lag, may change), and
//! [`SpeechEngine::transcribe_final`] produces authoritative *final* segments
//! (`pass = final`, time-anchored, the citation target). Live rows are superseded
//! by final rows over the same time window (Data Model §8.3).
//!
//! This crate is the **pure contract layer only**: the whisper.cpp / parakeet
//! backends implement the trait behind FFI. Input PCM is borrowed (`&[f32]`) and
//! **never serialized** — it stays in native memory (CLAUDE.md N13).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use app_domain::{Id, SegmentId, TranscriptPass, TranscriptSegment};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Model profiles
// ---------------------------------------------------------------------------

/// The accuracy/latency trade-off tier a speech model runs at (HLD §9.2
/// `ModelProfile`; the model registry picks the concrete weights).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechModelProfile {
    /// Lowest latency, smallest model — for live streaming on modest hardware.
    Fast,
    /// Default balance of latency and accuracy.
    Balanced,
    /// Highest accuracy — used for the authoritative final pass.
    Quality,
}

impl SpeechModelProfile {
    /// All profiles, in ascending accuracy order.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Fast, Self::Balanced, Self::Quality]
    }
}

// ---------------------------------------------------------------------------
// Audio input (PCM stays native — borrowed, never serialized)
// ---------------------------------------------------------------------------

/// A short slice of normalized PCM for the low-latency live pass.
///
/// Samples are mono float32 (normalized by `media-pipeline`). The buffer is
/// borrowed — PCM never crosses IPC or hits disk as JSON (N13).
#[derive(Debug)]
pub struct AudioChunk<'a> {
    pub samples: &'a [f32],
    pub sample_rate_hz: u32,
    /// Milliseconds from session start at which this chunk begins.
    pub t_start_ms: i64,
}

/// A bounded span of normalized PCM for the authoritative final pass — typically
/// a full utterance / VAD segment with a settled time window.
#[derive(Debug)]
pub struct AudioSpan<'a> {
    pub samples: &'a [f32],
    pub sample_rate_hz: u32,
    pub t_start_ms: i64,
    pub t_end_ms: i64,
}

// ---------------------------------------------------------------------------
// Output: partial (live) vs final segments
// ---------------------------------------------------------------------------

/// A *partial* hypothesis from the live pass (`pass = live`). It is provisional:
/// a later live or final result may revise the same time window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialHypothesis {
    pub text: String,
    pub t_start_ms: i64,
    pub t_end_ms: i64,
    /// How likely this hypothesis is to survive unchanged, in `0.0..=1.0`
    /// (1.0 = effectively settled). Purely a UI/stream hint.
    pub stability: f32,
    /// Acoustic/decoder confidence in `0.0..=1.0`, when the backend exposes one.
    pub confidence: Option<f32>,
}

impl PartialHypothesis {
    /// The pass that produced this hypothesis (always [`TranscriptPass::Live`]).
    #[must_use]
    pub const fn pass(&self) -> TranscriptPass {
        TranscriptPass::Live
    }
}

/// An authoritative *final* segment (`pass = final`) — the atomic unit of
/// evidence (Data Model §8.3). Its `segment_id` is what `evidence_segment_ids`
/// and citations resolve to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinalSegment {
    pub segment_id: SegmentId,
    pub t_start_ms: i64,
    pub t_end_ms: i64,
    /// Speaker-turn label from diarization, when available.
    pub speaker: Option<String>,
    pub text: String,
    /// Decoder confidence in `0.0..=1.0`, when the backend exposes one.
    pub confidence: Option<f32>,
    /// Detected language for this segment, when known.
    pub language: Option<LanguageTag>,
}

impl FinalSegment {
    /// The pass that produced this segment (always [`TranscriptPass::Final`]).
    #[must_use]
    pub const fn pass(&self) -> TranscriptPass {
        TranscriptPass::Final
    }

    /// Project into the shared [`TranscriptSegment`] the Core persists / pushes
    /// on `LiveTranscript` (Data Model §8.3). `person_id` resolution and `seq`
    /// assignment happen downstream in `app-service`.
    #[must_use]
    pub fn to_transcript_segment(&self) -> TranscriptSegment {
        TranscriptSegment {
            segment_id: self.segment_id,
            t_start_ms: self.t_start_ms,
            t_end_ms: self.t_end_ms,
            speaker: self.speaker.clone(),
            text: self.text.clone(),
            pass: TranscriptPass::Final,
            confidence: self.confidence,
        }
    }
}

/// A detected language, as a BCP-47 / ISO-639-1 code plus a confidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LanguageTag {
    /// e.g. `"en"`, `"es"`, `"pt-BR"`.
    pub code: String,
    /// Detection confidence in `0.0..=1.0`.
    pub confidence: f32,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failures raised across the speech contract (`thiserror`, typed per taxonomy).
#[derive(Debug, Error)]
pub enum SpeechError {
    /// The model weights for the active profile are not resident yet.
    #[error("speech model not loaded for profile {0:?}")]
    ModelNotLoaded(SpeechModelProfile),

    /// The requested profile is not offered by this engine.
    #[error("unsupported speech model profile: {0:?}")]
    UnsupportedProfile(SpeechModelProfile),

    /// The input sample rate is not one this engine accepts.
    #[error("unsupported sample rate {got} Hz (expected {expected} Hz)")]
    UnsupportedSampleRate { got: u32, expected: u32 },

    /// The audio span was empty or malformed.
    #[error("invalid audio input: {0}")]
    InvalidAudio(String),

    /// The decoder failed on this input.
    #[error("decode failed: {0}")]
    DecodeFailed(String),

    /// Catch-all for a native/backend failure.
    #[error("speech backend error: {0}")]
    Backend(String),
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// A two-pass speech-to-text engine (HLD §9.2).
///
/// Implemented by swappable backends (`speech-whisper`, `speech-parakeet`). The
/// live pass is a hot, allocation-light path called on streamed chunks; the final
/// pass is the heavier, authoritative decode. Neither runs on an OS RT audio
/// callback (N13) — the caller feeds already-buffered PCM.
pub trait SpeechEngine: Send {
    /// The profiles this engine can serve.
    fn profiles(&self) -> Vec<SpeechModelProfile>;

    /// The currently active profile.
    fn active_profile(&self) -> SpeechModelProfile;

    /// Switch the active profile (may load different weights).
    ///
    /// # Errors
    /// Returns [`SpeechError::UnsupportedProfile`] if the engine cannot serve it.
    fn set_profile(&mut self, profile: SpeechModelProfile) -> Result<(), SpeechError>;

    /// Detect the spoken language from a chunk, if determinable.
    ///
    /// # Errors
    /// Propagates decoder/backend failures.
    fn detect_language(
        &mut self,
        chunk: &AudioChunk<'_>,
    ) -> Result<Option<LanguageTag>, SpeechError>;

    /// Pass 1 — low-latency live decode. Returns provisional partial
    /// hypotheses (`pass = live`) that a later pass may revise. Infallible by
    /// design so a glitch never stalls the live stream; degraded output is empty.
    fn transcribe_live(&mut self, chunk: &AudioChunk<'_>) -> Vec<PartialHypothesis>;

    /// Pass 2 — authoritative final decode over a settled span. The returned
    /// [`FinalSegment`]s carry the `segment_id`s that evidence citations resolve
    /// to (Data Model §8.3).
    ///
    /// # Errors
    /// Returns a [`SpeechError`] on decode/backend failure; the caller keeps the
    /// captured audio and may retry (the LLM/STT never owns recording state).
    fn transcribe_final(&mut self, span: &AudioSpan<'_>) -> Result<Vec<FinalSegment>, SpeechError>;
}

// ---------------------------------------------------------------------------
// Deterministic test double
// ---------------------------------------------------------------------------

/// A deterministic [`SpeechEngine`] for tests: identical input always yields
/// identical output (no RNG, no clock). Segment ids are derived from the span's
/// time window so they are stable and reproducible.
#[derive(Clone, Debug)]
pub struct MockSpeechEngine {
    profile: SpeechModelProfile,
    /// Language the mock always "detects".
    language: LanguageTag,
}

impl Default for MockSpeechEngine {
    fn default() -> Self {
        Self {
            profile: SpeechModelProfile::Balanced,
            language: LanguageTag {
                code: "en".into(),
                confidence: 0.99,
            },
        }
    }
}

impl MockSpeechEngine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Derive a stable [`SegmentId`] from a time window (no randomness).
    fn stable_id(t_start_ms: i64, t_end_ms: i64, idx: u32) -> SegmentId {
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&t_start_ms.to_be_bytes());
        bytes[8..16].copy_from_slice(&(t_end_ms ^ i64::from(idx)).to_be_bytes());
        Id::from_bytes(bytes)
    }
}

impl SpeechEngine for MockSpeechEngine {
    fn profiles(&self) -> Vec<SpeechModelProfile> {
        SpeechModelProfile::all().to_vec()
    }

    fn active_profile(&self) -> SpeechModelProfile {
        self.profile
    }

    fn set_profile(&mut self, profile: SpeechModelProfile) -> Result<(), SpeechError> {
        self.profile = profile;
        Ok(())
    }

    fn detect_language(
        &mut self,
        chunk: &AudioChunk<'_>,
    ) -> Result<Option<LanguageTag>, SpeechError> {
        if chunk.samples.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.language.clone()))
    }

    fn transcribe_live(&mut self, chunk: &AudioChunk<'_>) -> Vec<PartialHypothesis> {
        if chunk.samples.is_empty() {
            return Vec::new();
        }
        // Deterministic: text keyed on the chunk's start offset and length.
        vec![PartialHypothesis {
            text: format!(
                "live segment at {}ms ({} samples)",
                chunk.t_start_ms,
                chunk.samples.len()
            ),
            t_start_ms: chunk.t_start_ms,
            t_end_ms: chunk.t_start_ms + 1000,
            stability: 0.5,
            confidence: Some(0.8),
        }]
    }

    fn transcribe_final(&mut self, span: &AudioSpan<'_>) -> Result<Vec<FinalSegment>, SpeechError> {
        if span.samples.is_empty() {
            return Err(SpeechError::InvalidAudio("empty span".into()));
        }
        Ok(vec![FinalSegment {
            segment_id: Self::stable_id(span.t_start_ms, span.t_end_ms, 0),
            t_start_ms: span.t_start_ms,
            t_end_ms: span.t_end_ms,
            speaker: Some("speaker-1".into()),
            text: format!("final segment [{}..{}]ms", span.t_start_ms, span.t_end_ms),
            confidence: Some(0.95),
            language: Some(self.language.clone()),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(samples: &[f32], t: i64) -> AudioChunk<'_> {
        AudioChunk {
            samples,
            sample_rate_hz: 16_000,
            t_start_ms: t,
        }
    }

    #[test]
    fn mock_is_deterministic() {
        let pcm = vec![0.1_f32; 320];
        let mut a = MockSpeechEngine::new();
        let mut b = MockSpeechEngine::new();

        assert_eq!(
            a.transcribe_live(&chunk(&pcm, 500)),
            b.transcribe_live(&chunk(&pcm, 500))
        );

        let span = AudioSpan {
            samples: &pcm,
            sample_rate_hz: 16_000,
            t_start_ms: 1000,
            t_end_ms: 4000,
        };
        let s1 = a.transcribe_final(&span).unwrap();
        let s2 = b.transcribe_final(&span).unwrap();
        assert_eq!(s1, s2);
        assert_eq!(s1[0].pass(), TranscriptPass::Final);
        assert!(!s1[0].text.is_empty());
        assert!(s1[0].confidence.is_some());
    }

    #[test]
    fn empty_input_is_handled() {
        let mut e = MockSpeechEngine::new();
        assert!(e.transcribe_live(&chunk(&[], 0)).is_empty());
        assert_eq!(e.detect_language(&chunk(&[], 0)).unwrap(), None);
        let span = AudioSpan {
            samples: &[],
            sample_rate_hz: 16_000,
            t_start_ms: 0,
            t_end_ms: 0,
        };
        assert!(matches!(
            e.transcribe_final(&span),
            Err(SpeechError::InvalidAudio(_))
        ));
    }

    #[test]
    fn final_segment_projects_to_transcript_segment() {
        let pcm = vec![0.2_f32; 160];
        let mut e = MockSpeechEngine::new();
        let span = AudioSpan {
            samples: &pcm,
            sample_rate_hz: 16_000,
            t_start_ms: 0,
            t_end_ms: 2000,
        };
        let seg = &e.transcribe_final(&span).unwrap()[0];
        let ts = seg.to_transcript_segment();
        assert_eq!(ts.segment_id, seg.segment_id);
        assert_eq!(ts.pass, TranscriptPass::Final);
        assert_eq!(ts.text, seg.text);
    }

    #[test]
    fn profile_switch_reports_active() {
        let mut e = MockSpeechEngine::new();
        assert_eq!(e.active_profile(), SpeechModelProfile::Balanced);
        e.set_profile(SpeechModelProfile::Quality).unwrap();
        assert_eq!(e.active_profile(), SpeechModelProfile::Quality);
        assert_eq!(e.profiles().len(), 3);
    }

    #[test]
    fn profile_serialises_snake_case() {
        let v = serde_json::to_value(SpeechModelProfile::Fast).unwrap();
        assert_eq!(v, serde_json::json!("fast"));
    }
}
