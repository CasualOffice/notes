//! Channel downmix to mono (Architecture §5: "channel downmix").
//!
//! Native capture delivers interleaved multi-channel `f32` PCM. STT consumes mono,
//! so we average all channels of each frame. Averaging (rather than summing) keeps
//! the signal inside `[-1, 1]` regardless of channel count, deferring any level
//! decisions to the [`normalize`](crate::normalize) stage.

use crate::error::{MediaPipelineError, Result};

/// Downmix interleaved `channels`-channel PCM to a mono buffer by averaging channels.
///
/// `interleaved.len()` must be an exact multiple of `channels`.
///
/// # Errors
/// Returns [`MediaPipelineError::InvalidChannels`] if `channels` is zero or the
/// buffer length is not a whole number of frames.
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Result<Vec<f32>> {
    if channels == 0 {
        return Err(MediaPipelineError::InvalidChannels {
            channels,
            samples: interleaved.len(),
        });
    }
    let ch = channels as usize;
    if !interleaved.len().is_multiple_of(ch) {
        return Err(MediaPipelineError::InvalidChannels {
            channels,
            samples: interleaved.len(),
        });
    }
    if ch == 1 {
        return Ok(interleaved.to_vec());
    }
    let frames = interleaved.len() / ch;
    let mut out = Vec::with_capacity(frames);
    let inv = 1.0f32 / ch as f32;
    for frame in interleaved.chunks_exact(ch) {
        // Sum in f64 to avoid catastrophic cancellation with many channels.
        let sum: f64 = frame.iter().map(|&s| s as f64).sum();
        out.push((sum * inv as f64) as f32);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_is_passthrough() {
        let x = [0.1, -0.2, 0.3];
        assert_eq!(downmix_to_mono(&x, 1).unwrap(), x);
    }

    #[test]
    fn stereo_averages_channels() {
        // frames: (1.0, -1.0) -> 0.0 ; (0.5, 0.5) -> 0.5
        let x = [1.0, -1.0, 0.5, 0.5];
        let mono = downmix_to_mono(&x, 2).unwrap();
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn averaging_stays_in_range() {
        let x = [1.0, 1.0, 1.0, 1.0]; // two frames of full-scale stereo
        let mono = downmix_to_mono(&x, 2).unwrap();
        assert!(mono.iter().all(|&s| (-1.0..=1.0).contains(&s)));
        assert_eq!(mono, vec![1.0, 1.0]);
    }

    #[test]
    fn rejects_zero_channels() {
        assert!(matches!(
            downmix_to_mono(&[0.0, 0.0], 0),
            Err(MediaPipelineError::InvalidChannels { .. })
        ));
    }

    #[test]
    fn rejects_ragged_buffer() {
        // 3 samples cannot be split into stereo frames.
        assert!(matches!(
            downmix_to_mono(&[0.0, 0.0, 0.0], 2),
            Err(MediaPipelineError::InvalidChannels { .. })
        ));
    }

    #[test]
    fn empty_is_ok() {
        assert_eq!(downmix_to_mono(&[], 2).unwrap(), Vec::<f32>::new());
    }
}
