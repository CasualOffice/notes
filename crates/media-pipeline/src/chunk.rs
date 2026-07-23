//! Chunking, the bounded capture ring, and health signalling (Architecture §5;
//! HLD §8.4: "downmix/resample ... VAD, chunk ... overlapping chunks ... pass-1 live";
//! §9.1: the capture ring "never allocates, locks, or blocks" and degrades honestly).
//!
//! This module owns the two capture-side back-pressure guarantees:
//!
//! * [`AudioRing`] — a bounded sample ring between the native capture callback and the
//!   DSP thread. When the consumer falls behind, the producer **drops the oldest
//!   not-yet-persisted samples and emits a [`RingOverflow`] health event** rather than
//!   blocking or unbounded-growing (N13 / the "no RT-callback sin" invariant). The ring
//!   is a plain `VecDeque`; the real lock-free SPSC implementation lives in the native
//!   `capture-*` crates — this is the deterministic model the DSP layer is tested against.
//! * [`Chunker`] — turns cut [`UtteranceWindow`](crate::vad::UtteranceWindow)s into
//!   recoverable [`Chunk`]s (time-anchored, carrying their own samples so a crash mid-
//!   session can replay them) plus the [`InferenceJob`]s the STT scheduler consumes.

use app_domain::ChunkId;

use crate::vad::UtteranceWindow;

/// Emitted when the ring drops samples because the consumer fell behind. Surfaced as a
/// capture health event so the UI can report degradation honestly (never silent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingOverflow {
    /// Samples discarded by this push.
    pub dropped_samples: u64,
    /// Ring capacity in samples (for context in the health event).
    pub capacity: usize,
    /// Cumulative samples dropped over the ring's lifetime.
    pub total_dropped: u64,
}

/// A bounded, drop-oldest ring of mono `f32` samples.
#[derive(Debug, Clone)]
pub struct AudioRing {
    buf: std::collections::VecDeque<f32>,
    capacity: usize,
    total_dropped: u64,
}

impl AudioRing {
    /// Create a ring holding at most `capacity` samples.
    ///
    /// # Panics
    /// Panics if `capacity` is zero — a zero-length ring can never make progress and
    /// indicates a configuration bug, not a runtime condition.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "AudioRing capacity must be non-zero");
        Self {
            buf: std::collections::VecDeque::with_capacity(capacity),
            capacity,
            total_dropped: 0,
        }
    }

    /// Create a ring sized to hold `seconds` of mono audio at `sample_rate`.
    #[must_use]
    pub fn with_seconds(sample_rate: u32, seconds: f32) -> Self {
        let cap = (sample_rate as f32 * seconds.max(0.0)).round() as usize;
        Self::new(cap.max(1))
    }

    /// Number of samples currently buffered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the ring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Ring capacity in samples.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Push samples. If the incoming block would exceed capacity, the oldest samples
    /// are dropped to make room and a [`RingOverflow`] is returned. Never blocks.
    ///
    /// A block larger than the whole ring keeps only its most-recent `capacity`
    /// samples (the freshest audio wins).
    pub fn push_slice(&mut self, samples: &[f32]) -> Option<RingOverflow> {
        if samples.is_empty() {
            return None;
        }
        let incoming = samples.len();
        // How many existing+incoming samples exceed capacity.
        let projected = self.buf.len() + incoming;
        let mut dropped = 0u64;
        if projected > self.capacity {
            let overflow = projected - self.capacity;
            // Drop from the front (oldest), but never more than we currently hold.
            let drop_existing = overflow.min(self.buf.len());
            for _ in 0..drop_existing {
                self.buf.pop_front();
            }
            dropped += drop_existing as u64;
            // If the incoming block alone exceeds capacity, keep only its tail.
            let keep = incoming.min(self.capacity);
            let start = incoming - keep;
            dropped += (incoming - keep) as u64;
            self.buf.extend(samples[start..].iter().copied());
        } else {
            self.buf.extend(samples.iter().copied());
        }

        if dropped > 0 {
            self.total_dropped += dropped;
            Some(RingOverflow {
                dropped_samples: dropped,
                capacity: self.capacity,
                total_dropped: self.total_dropped,
            })
        } else {
            None
        }
    }

    /// Drain up to `max` samples from the front (oldest first) for the DSP stage.
    pub fn drain(&mut self, max: usize) -> Vec<f32> {
        let n = max.min(self.buf.len());
        self.buf.drain(0..n).collect()
    }

    /// Drain all buffered samples.
    pub fn drain_all(&mut self) -> Vec<f32> {
        self.buf.drain(..).collect()
    }
}

/// Which STT pass a job feeds. Live is the low-latency pass-1 over each chunk; the
/// authoritative final pass runs session-wide at end-of-capture (out of scope here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Pass-1 live transcription over a single chunk (HLD §8.4).
    Live,
    /// Pass-2 final transcription (session-level; modelled for completeness).
    Final,
}

/// An inference job referencing the chunk it should transcribe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceJob {
    /// The chunk this job transcribes.
    pub chunk_id: ChunkId,
    /// Which pass this job feeds.
    pub kind: JobKind,
}

/// A recoverable, time-anchored chunk. It carries its own samples so an in-flight
/// chunk can be replayed from the session journal after a crash, and its time range
/// anchors resulting transcript segments back to the session timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Stable chunk id (spine `chunk.id`, Data Model §9.2).
    pub id: ChunkId,
    /// Monotonic sequence within the session.
    pub seq: u64,
    /// Session-relative start time in milliseconds.
    pub t_start_ms: i64,
    /// Session-relative end time in milliseconds (exclusive).
    pub t_end_ms: i64,
    /// Mono 16 kHz samples for this chunk (includes leading overlap).
    pub samples: Vec<f32>,
    /// Whether the chunk's audio has been persisted to the session journal yet. The
    /// ring only ever drops frames that are **not** yet part of a persisted chunk.
    pub persisted: bool,
}

/// A chunk plus the inference jobs it spawned.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkOutput {
    /// The emitted chunk.
    pub chunk: Chunk,
    /// Jobs to enqueue for the STT scheduler.
    pub jobs: Vec<InferenceJob>,
}

/// Converts cut utterance windows into recoverable chunks and inference jobs.
#[derive(Debug, Default)]
pub struct Chunker {
    emitted: u64,
}

impl Chunker {
    /// Create a fresh chunker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Total chunks emitted so far.
    #[must_use]
    pub fn chunks_emitted(&self) -> u64 {
        self.emitted
    }

    /// Wrap a cut [`UtteranceWindow`] into a [`Chunk`] and its live inference job.
    ///
    /// The chunk id is a fresh spine [`ChunkId`]; the window's own `seq` is preserved.
    pub fn emit(&mut self, window: UtteranceWindow) -> ChunkOutput {
        let id = ChunkId::new();
        let chunk = Chunk {
            id,
            seq: window.seq,
            t_start_ms: window.t_start_ms,
            t_end_ms: window.t_end_ms,
            samples: window.samples,
            persisted: false,
        };
        self.emitted += 1;
        ChunkOutput {
            chunk,
            jobs: vec![InferenceJob {
                chunk_id: id,
                kind: JobKind::Live,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_accepts_within_capacity() {
        let mut ring = AudioRing::new(8);
        assert!(ring.push_slice(&[1.0, 2.0, 3.0]).is_none());
        assert_eq!(ring.len(), 3);
        assert!(ring.push_slice(&[4.0, 5.0]).is_none());
        assert_eq!(ring.len(), 5);
    }

    #[test]
    fn ring_drops_oldest_on_overflow() {
        let mut ring = AudioRing::new(4);
        assert!(ring.push_slice(&[1.0, 2.0, 3.0, 4.0]).is_none());
        // Pushing 2 more must drop the 2 oldest and report it.
        let ov = ring.push_slice(&[5.0, 6.0]).expect("overflow expected");
        assert_eq!(ov.dropped_samples, 2);
        assert_eq!(ov.total_dropped, 2);
        assert_eq!(ring.capacity, 4);
        // Newest-4 survive: 3,4,5,6.
        assert_eq!(ring.drain_all(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn ring_block_larger_than_capacity_keeps_tail() {
        let mut ring = AudioRing::new(3);
        let ov = ring
            .push_slice(&[1.0, 2.0, 3.0, 4.0, 5.0])
            .expect("overflow expected");
        assert_eq!(ov.dropped_samples, 2);
        assert_eq!(ring.drain_all(), vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn ring_never_blocks_and_accumulates_total() {
        let mut ring = AudioRing::new(2);
        ring.push_slice(&[1.0, 2.0]);
        let ov1 = ring.push_slice(&[3.0]).unwrap();
        assert_eq!(ov1.total_dropped, 1);
        let ov2 = ring.push_slice(&[4.0]).unwrap();
        assert_eq!(ov2.total_dropped, 2);
    }

    #[test]
    fn ring_drain_takes_oldest_first() {
        let mut ring = AudioRing::new(8);
        ring.push_slice(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(ring.drain(2), vec![1.0, 2.0]);
        assert_eq!(ring.drain(10), vec![3.0, 4.0]);
        assert!(ring.is_empty());
    }

    #[test]
    fn chunker_emits_chunk_and_live_job() {
        let mut c = Chunker::new();
        let window = UtteranceWindow {
            seq: 7,
            t_start_ms: 1000,
            t_end_ms: 7000,
            samples: vec![0.1; 320],
            cut_at_silence: true,
        };
        let out = c.emit(window);
        assert_eq!(out.chunk.seq, 7);
        assert_eq!(out.chunk.t_start_ms, 1000);
        assert_eq!(out.chunk.t_end_ms, 7000);
        assert!(!out.chunk.persisted);
        assert_eq!(out.jobs.len(), 1);
        assert_eq!(out.jobs[0].kind, JobKind::Live);
        assert_eq!(out.jobs[0].chunk_id, out.chunk.id);
    }
}
