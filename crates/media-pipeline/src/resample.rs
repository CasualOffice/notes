//! Resampling to the 16 kHz mono `f32` STT input rate (Architecture §5).
//!
//! # Method — windowed-sinc polyphase interpolation
//!
//! This is a self-contained, documented band-limited resampler (no external DSP
//! crate). For each output sample at continuous input position `t = n * in/out`, the
//! output is a weighted sum of nearby input samples using a **windowed-sinc** kernel:
//!
//! ```text
//!   h(d) = 2*fc * sinc(2*fc*d) * blackman(d / support)
//! ```
//!
//! where `d` is the distance (in input samples) from `t`, `fc` is the normalized
//! low-pass cutoff and `support` is the kernel half-width. When downsampling
//! (`out < in`) the cutoff is lowered to `0.5 * out/in` so the sinc doubles as the
//! anti-aliasing filter; when upsampling it stays at the input Nyquist (`0.5`). The
//! kernel spans [`ZERO_CROSSINGS`] sinc lobes on each side. Weights are normalized to
//! unit sum so DC gain is exactly 1 (amplitude-preserving), which the tests rely on.
//!
//! The [`Resampler`] is **streaming**: it retains just enough input history to supply
//! the left/right context of the next output sample, so it can be driven with
//! arbitrary-sized capture blocks. [`Resampler::flush`] drains the tail (treating
//! past-the-end input as silence) at end-of-stream. [`resample`] is a one-shot helper.

use crate::error::{MediaPipelineError, Result};

/// The canonical STT input rate. All downstream stages assume 16 kHz mono `f32`.
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// Number of sinc zero-crossings (lobes) retained on each side of the kernel center.
/// 16 lobes give roughly -60 dB stopband with the Blackman window — good quality at
/// a modest cost, and deterministic.
pub const ZERO_CROSSINGS: usize = 16;

use std::f64::consts::PI;

#[inline]
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = PI * x;
        px.sin() / px
    }
}

/// Blackman window on the normalized interval `t ∈ [-1, 1]`; zero at the endpoints.
#[inline]
fn blackman(t: f64) -> f64 {
    if t.abs() >= 1.0 {
        0.0
    } else {
        0.42 + 0.5 * (PI * t).cos() + 0.08 * (2.0 * PI * t).cos()
    }
}

/// A streaming windowed-sinc resampler converting one mono `f32` stream to a fixed
/// output rate.
#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: f64,
    out_rate: f64,
    /// Input samples advanced per output sample (`in/out`).
    step: f64,
    /// Normalized low-pass cutoff (cycles per input sample).
    fc: f64,
    /// Kernel half-width in input samples.
    support: f64,
    /// Retained input history.
    history: Vec<f32>,
    /// Absolute input-sample index of `history[0]`.
    base_index: u64,
    /// Index of the next output sample to emit.
    next_out: u64,
    /// Set by [`Resampler::flush`]: emit remaining outputs treating missing input as 0.
    draining: bool,
}

impl Resampler {
    /// Create a resampler from `in_rate` to `out_rate` (both in Hz).
    ///
    /// # Errors
    /// Returns [`MediaPipelineError::UnsupportedRate`] if either rate is zero.
    pub fn new(in_rate: u32, out_rate: u32) -> Result<Self> {
        if in_rate == 0 {
            return Err(MediaPipelineError::UnsupportedRate { rate: in_rate });
        }
        if out_rate == 0 {
            return Err(MediaPipelineError::UnsupportedRate { rate: out_rate });
        }
        let inr = f64::from(in_rate);
        let outr = f64::from(out_rate);
        let step = inr / outr;
        // Anti-aliasing cutoff: input Nyquist when upsampling, output Nyquist when down.
        let fc = 0.5 * (outr / inr).min(1.0);
        let support = ZERO_CROSSINGS as f64 / (2.0 * fc);
        Ok(Self {
            in_rate: inr,
            out_rate: outr,
            step,
            fc,
            support,
            history: Vec::new(),
            base_index: 0,
            next_out: 0,
            draining: false,
        })
    }

    /// Create a resampler targeting [`TARGET_SAMPLE_RATE_HZ`].
    ///
    /// # Errors
    /// Returns [`MediaPipelineError::UnsupportedRate`] if `in_rate` is zero.
    pub fn to_target(in_rate: u32) -> Result<Self> {
        Self::new(in_rate, TARGET_SAMPLE_RATE_HZ)
    }

    /// The input sample rate in Hz.
    #[must_use]
    pub fn input_rate(&self) -> u32 {
        self.in_rate as u32
    }

    /// The output sample rate in Hz.
    #[must_use]
    pub fn output_rate(&self) -> u32 {
        self.out_rate as u32
    }

    /// Push a block of input samples and return the output samples that became fully
    /// determined by the newly-available context. Call [`flush`](Self::flush) once at
    /// end-of-stream to drain the remainder.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.history.extend_from_slice(input);
        let mut out = Vec::new();
        self.produce(&mut out);
        self.trim();
        out
    }

    /// Emit all remaining outputs, treating input past the buffered tail as silence.
    /// After flushing, the resampler must not be reused.
    pub fn flush(&mut self) -> Vec<f32> {
        self.draining = true;
        let mut out = Vec::new();
        self.produce(&mut out);
        out
    }

    /// The absolute-input-index (exclusive) up to which history is populated.
    fn available_end(&self) -> u64 {
        self.base_index + self.history.len() as u64
    }

    /// Produce as many output samples as the current buffer allows into `out`.
    fn produce(&mut self, out: &mut Vec<f32>) {
        let available_end = self.available_end();
        if self.history.is_empty() {
            return;
        }
        loop {
            let center = self.next_out as f64 * self.step;
            let hi_ideal = (center + self.support).floor() as i64;
            if !self.draining && hi_ideal as u64 >= available_end {
                // Need more right-side context before this output is final.
                break;
            }
            if self.draining && center > (available_end as f64) - 1.0 {
                // No input remains under the kernel center.
                break;
            }
            let hi = hi_ideal.min(available_end as i64 - 1);
            let lo = ((center - self.support).ceil() as i64).max(self.base_index as i64);
            let mut acc = 0.0f64;
            let mut wsum = 0.0f64;
            let mut j = lo;
            while j <= hi {
                let d = center - j as f64;
                let w = 2.0 * self.fc * sinc(2.0 * self.fc * d) * blackman(d / self.support);
                let x = f64::from(self.history[(j - self.base_index as i64) as usize]);
                acc += w * x;
                wsum += w;
                j += 1;
            }
            let y = if wsum.abs() > 1e-12 { acc / wsum } else { 0.0 };
            out.push(y as f32);
            self.next_out += 1;
        }
    }

    /// Drop history samples that no future output can reference.
    fn trim(&mut self) {
        let next_center = self.next_out as f64 * self.step;
        let keep_from = ((next_center - self.support).floor() as i64).max(self.base_index as i64);
        let drop = (keep_from - self.base_index as i64).max(0) as usize;
        if drop > 0 && drop <= self.history.len() {
            self.history.drain(0..drop);
            self.base_index += drop as u64;
        }
    }
}

/// One-shot resample of a whole buffer from `in_rate` to `out_rate`.
///
/// Equivalent to constructing a [`Resampler`], calling `process` once, then `flush`.
///
/// # Errors
/// Returns [`MediaPipelineError::UnsupportedRate`] if either rate is zero.
pub fn resample(input: &[f32], in_rate: u32, out_rate: u32) -> Result<Vec<f32>> {
    let mut r = Resampler::new(in_rate, out_rate)?;
    let mut out = r.process(input);
    out.extend(r.flush());
    Ok(out)
}

/// One-shot resample of a whole buffer to [`TARGET_SAMPLE_RATE_HZ`].
///
/// # Errors
/// Returns [`MediaPipelineError::UnsupportedRate`] if `in_rate` is zero.
pub fn resample_to_target(input: &[f32], in_rate: u32) -> Result<Vec<f32>> {
    resample(input, in_rate, TARGET_SAMPLE_RATE_HZ)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine(freq: f64, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    #[test]
    fn rejects_zero_rates() {
        assert!(Resampler::new(0, 16_000).is_err());
        assert!(Resampler::new(48_000, 0).is_err());
    }

    #[test]
    fn output_length_matches_ratio_downsample() {
        // 48k -> 16k, ratio 1/3. 48000 input samples -> ~16000 output.
        let input = sine(440.0, 48_000, 48_000);
        let out = resample(&input, 48_000, 16_000).unwrap();
        let expected = 16_000.0;
        let err = (out.len() as f64 - expected).abs();
        assert!(err <= 2.0, "len {} vs {}", out.len(), expected);
    }

    #[test]
    fn output_length_matches_ratio_upsample() {
        // 8k -> 16k, ratio 2. 8000 input -> ~16000 output.
        let input = sine(300.0, 8_000, 8_000);
        let out = resample(&input, 8_000, 16_000).unwrap();
        let err = (out.len() as f64 - 16_000.0).abs();
        assert!(err <= 2.0, "len {}", out.len());
    }

    #[test]
    fn identity_rate_preserves_signal_midbuffer() {
        // in==out: the resampler is a Nyquist low-pass ~ identity in the interior.
        let input = sine(1_000.0, 16_000, 4_000);
        let out = resample(&input, 16_000, 16_000).unwrap();
        assert!((out.len() as i64 - input.len() as i64).abs() <= 2);
        // Compare a mid region away from edge transients.
        let max_err = out[1_500..2_500]
            .iter()
            .zip(&input[1_500..2_500])
            .map(|(o, i)| (o - i).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 0.02, "max_err {max_err}");
    }

    #[test]
    fn preserves_tone_after_downsample() {
        // A 1 kHz tone survives 48k->16k (well below the 8k output Nyquist) with the
        // right amplitude and frequency. Check peak amplitude ~ 1.0 in the interior.
        let input = sine(1_000.0, 48_000, 48_000);
        let out = resample(&input, 48_000, 16_000).unwrap();
        let interior = &out[2_000..14_000];
        let peak = interior.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!((peak - 1.0).abs() < 0.05, "peak {peak}");
    }

    #[test]
    fn streaming_matches_one_shot() {
        let input = sine(500.0, 32_000, 6_400);
        let one_shot = resample(&input, 32_000, 16_000).unwrap();

        let mut r = Resampler::new(32_000, 16_000).unwrap();
        let mut streamed = Vec::new();
        for block in input.chunks(97) {
            streamed.extend(r.process(block));
        }
        streamed.extend(r.flush());

        assert_eq!(streamed.len(), one_shot.len());
        for (a, b) in streamed.iter().zip(one_shot.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn silence_stays_silent() {
        let input = vec![0.0f32; 9_600];
        let out = resample(&input, 48_000, 16_000).unwrap();
        assert!(out.iter().all(|&s| s.abs() < 1e-6));
    }
}
