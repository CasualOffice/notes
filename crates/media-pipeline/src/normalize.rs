//! Level conditioning (Architecture §5: "clipping protection, DC removal, conservative
//! gain normalization").
//!
//! Order of operations, applied in [`Normalizer::process`]:
//! 1. **DC removal** — a streaming one-pole high-pass ([`DcBlocker`]) strips any DC
//!    offset introduced by the capture path without a look-ahead delay.
//! 2. **Conservative gain** — a slow-moving makeup gain nudges the running peak toward
//!    a headroom target, but is clamped to `max_gain` so quiet passages / room noise
//!    are never blown up. Gain is smoothed across blocks to avoid pumping.
//! 3. **Clipping protection** — a `tanh` soft-clip tames the rare inter-sample
//!    overshoot, then a hard clamp guarantees the STT front-end never sees a value
//!    outside `[-1, 1]`.
//!
//! Every step is deterministic and allocation-free over a caller-owned buffer.

/// A one-pole DC-blocking high-pass filter: `y[n] = x[n] - x[n-1] + R * y[n-1]`.
///
/// `R` near 1.0 gives a very low corner frequency (a few Hz at 16 kHz), removing DC
/// and sub-audible rumble while leaving speech untouched.
#[derive(Debug, Clone)]
pub struct DcBlocker {
    r: f32,
    prev_in: f32,
    prev_out: f32,
}

impl Default for DcBlocker {
    fn default() -> Self {
        // R = 0.999 -> corner ~2.5 Hz at 16 kHz.
        Self::new(0.999)
    }
}

impl DcBlocker {
    /// Create a DC blocker with pole coefficient `r` (clamped to `[0, 1)`).
    #[must_use]
    pub fn new(r: f32) -> Self {
        Self {
            r: r.clamp(0.0, 0.999_999),
            prev_in: 0.0,
            prev_out: 0.0,
        }
    }

    /// Filter `buf` in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            let x = *s;
            let y = x - self.prev_in + self.r * self.prev_out;
            self.prev_in = x;
            self.prev_out = y;
            *s = y;
        }
    }
}

/// Remove DC from a standalone block by subtracting its mean (non-streaming helper).
pub fn remove_dc_mean(buf: &mut [f32]) {
    if buf.is_empty() {
        return;
    }
    let mean = (buf.iter().map(|&s| f64::from(s)).sum::<f64>() / buf.len() as f64) as f32;
    for s in buf.iter_mut() {
        *s -= mean;
    }
}

/// Soft-clip a single sample with `tanh`, blending toward the hard limit only as the
/// input approaches full scale, then hard-clamp to `[-1, 1]`.
#[must_use]
pub fn soft_clip(x: f32) -> f32 {
    // Below the knee the response is linear; above it, tanh compresses the overshoot
    // asymptotically toward full scale, preserving sign.
    const KNEE: f32 = 0.9;
    let y = if x.abs() <= KNEE {
        x
    } else {
        let over = x.abs() - KNEE;
        let compressed = KNEE + (1.0 - KNEE) * (over / (1.0 - KNEE)).tanh();
        x.signum() * compressed
    };
    // Hard clamp is a belt-and-suspenders guarantee for the STT front-end.
    y.clamp(-1.0, 1.0)
}

/// Statistics reported by [`Normalizer::process`] for one block. Useful as a health
/// signal (e.g. persistent clipping means the source is too hot).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizeStats {
    /// Peak absolute sample value observed in the block *before* clipping protection.
    pub input_peak: f32,
    /// The makeup gain actually applied this block.
    pub applied_gain: f32,
    /// Number of samples that required hard-clamping after soft-clip.
    pub clipped: usize,
}

/// Tunables for [`Normalizer`].
#[derive(Debug, Clone, Copy)]
pub struct NormalizeConfig {
    /// DC-blocker pole coefficient.
    pub dc_pole: f32,
    /// Target peak (headroom) the makeup gain aims for, e.g. `0.89`.
    pub target_peak: f32,
    /// Upper bound on makeup gain — conservative, so noise is never boosted to speech
    /// level. `1.0` disables boosting entirely.
    pub max_gain: f32,
    /// Only apply makeup gain once the block peak exceeds this floor; below it the
    /// block is treated as silence/noise and left at unity.
    pub noise_floor: f32,
    /// Per-block smoothing factor for the gain (`0` = frozen, `1` = instant).
    pub gain_smoothing: f32,
}

impl Default for NormalizeConfig {
    fn default() -> Self {
        Self {
            dc_pole: 0.999,
            target_peak: 0.89,
            max_gain: 4.0,
            noise_floor: 0.02,
            gain_smoothing: 0.2,
        }
    }
}

/// Streaming level conditioner: DC removal, conservative gain, clip protection.
#[derive(Debug, Clone)]
pub struct Normalizer {
    cfg: NormalizeConfig,
    dc: DcBlocker,
    /// Smoothed makeup gain carried across blocks.
    gain: f32,
}

impl Default for Normalizer {
    fn default() -> Self {
        Self::new(NormalizeConfig::default())
    }
}

impl Normalizer {
    /// Create a conditioner with the given configuration.
    #[must_use]
    pub fn new(cfg: NormalizeConfig) -> Self {
        Self {
            dc: DcBlocker::new(cfg.dc_pole),
            gain: 1.0,
            cfg,
        }
    }

    /// Condition `buf` in place and return per-block [`NormalizeStats`].
    pub fn process(&mut self, buf: &mut [f32]) -> NormalizeStats {
        // 1. DC removal.
        self.dc.process(buf);

        // 2. Peak-driven conservative makeup gain.
        let input_peak = buf.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let desired = if input_peak > self.cfg.noise_floor {
            (self.cfg.target_peak / input_peak).clamp(1.0, self.cfg.max_gain)
        } else {
            1.0
        };
        // Smooth toward the desired gain to avoid pumping between blocks.
        let a = self.cfg.gain_smoothing.clamp(0.0, 1.0);
        self.gain += a * (desired - self.gain);
        let applied_gain = self.gain;

        // 3. Apply gain, soft-clip, hard-clamp. `clipped` counts samples that would
        //    have exceeded full scale without protection — a useful "source too hot"
        //    health signal.
        let mut clipped = 0usize;
        for s in buf.iter_mut() {
            let g = *s * applied_gain;
            if g.abs() > 1.0 {
                clipped += 1;
            }
            *s = soft_clip(g);
        }

        NormalizeStats {
            input_peak,
            applied_gain,
            clipped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn dc_blocker_removes_offset() {
        // Use a faster pole so the exponential DC transient fully settles within the
        // test buffer; the filter still demonstrably rejects the constant offset.
        let mut x = vec![0.5f32; 4_000]; // pure DC
        let mut dc = DcBlocker::new(0.99);
        dc.process(&mut x);
        let tail = &x[2_000..];
        let tail_mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(tail_mean.abs() < 1e-3, "tail_mean {tail_mean}");
    }

    #[test]
    fn remove_dc_mean_zeroes_average() {
        let mut x: Vec<f32> = (0..1000).map(|i| 0.3 + 0.1 * (i as f32).sin()).collect();
        remove_dc_mean(&mut x);
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        assert!(mean.abs() < 1e-5);
    }

    #[test]
    fn soft_clip_never_exceeds_unity() {
        for i in -300..=300 {
            let x = i as f32 / 100.0; // -3.0 ..= 3.0
            let y = soft_clip(x);
            assert!((-1.0..=1.0).contains(&y), "x={x} y={y}");
        }
    }

    #[test]
    fn soft_clip_linear_below_knee() {
        assert!((soft_clip(0.5) - 0.5).abs() < 1e-6);
        assert!((soft_clip(-0.3) + 0.3).abs() < 1e-6);
    }

    #[test]
    fn conservative_gain_does_not_boost_noise() {
        // Very quiet block below the noise floor stays at unity gain.
        let mut noise = vec![0.005f32; 1600];
        let mut n = Normalizer::default();
        let stats = n.process(&mut noise);
        assert!(
            stats.applied_gain <= 1.0 + 1e-6,
            "gain {}",
            stats.applied_gain
        );
    }

    #[test]
    fn gain_is_bounded_by_max_gain() {
        let cfg = NormalizeConfig {
            max_gain: 3.0,
            gain_smoothing: 1.0,
            ..NormalizeConfig::default()
        };
        let mut n = Normalizer::new(cfg);
        // Quiet but above the noise floor: 0.1 peak -> target 0.89 would want 8.9x,
        // but must be clamped to 3.0.
        let mut buf: Vec<f32> = (0..1600)
            .map(|i| 0.1 * (2.0 * PI * 300.0 * i as f64 / 16_000.0).sin() as f32)
            .collect();
        let stats = n.process(&mut buf);
        assert!(
            stats.applied_gain <= 3.0 + 1e-6,
            "gain {}",
            stats.applied_gain
        );
        assert!(stats.applied_gain > 1.0);
    }

    #[test]
    fn output_never_clips() {
        let mut hot: Vec<f32> = (0..1600)
            .map(|i| 1.8 * (2.0 * PI * 440.0 * i as f64 / 16_000.0).sin() as f32)
            .collect();
        let mut n = Normalizer::default();
        n.process(&mut hot);
        assert!(hot.iter().all(|&s| (-1.0..=1.0).contains(&s)));
    }
}
