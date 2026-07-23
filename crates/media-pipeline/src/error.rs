//! Error taxonomy for the media pipeline. Typed with `thiserror` per the Architecture
//! error-taxonomy convention (no `unwrap()` on fallible paths).

/// Errors surfaced by the DSP pipeline. All variants are deterministic functions of
/// the caller-supplied parameters — there is no IO, so nothing here is transient.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MediaPipelineError {
    /// A sample rate of zero, or an otherwise unusable rate, was supplied.
    #[error("unsupported sample rate: {rate} Hz")]
    UnsupportedRate {
        /// The offending rate in Hz.
        rate: u32,
    },

    /// A channel count of zero, or interleaved data whose length is not a whole
    /// number of frames for the declared channel count.
    #[error(
        "interleaved buffer of {samples} samples is not a whole number of frames for {channels} channel(s)"
    )]
    InvalidChannels {
        /// The declared channel count.
        channels: u16,
        /// The interleaved sample length that could not be divided evenly.
        samples: usize,
    },

    /// A configuration value was outside its permitted range (e.g. overlap larger
    /// than the window, or a zero-length analysis frame).
    #[error("invalid configuration: {reason}")]
    InvalidConfig {
        /// Human-readable description of the constraint that was violated.
        reason: &'static str,
    },
}

/// Convenience alias for pipeline results.
pub type Result<T> = std::result::Result<T, MediaPipelineError>;
