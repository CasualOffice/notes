//! Monotonic session-timeline mapping with drift estimation (Architecture §5:
//! "monotonic clocks ... drift correction"; HLD media-timing).
//!
//! Each capture track carries its own native timestamps — a sample counter or a
//! device clock — that run at a rate slightly different from wall time and from the
//! other tracks. The pipeline needs one **monotonic session timeline** (milliseconds
//! from session start) so transcript segments from different tracks line up and so
//! `transcript_segment.t_start_ms` (Data Model §8.3) is well defined.
//!
//! # Approach
//!
//! For each track we maintain an online least-squares fit of native-timestamp →
//! session-millisecond observations ([`DriftEstimator`]), which recovers the track's
//! true rate (slope) and offset. Mapping does **not** use the live fit directly:
//! continuously re-warping time would jitter every segment boundary. Instead a
//! *committed* affine mapping is frozen between **safe boundaries** (silence gaps
//! chosen by the caller). At each safe boundary the committed mapping is reconciled
//! toward the latest estimate, but the correction is **clamped** to
//! [`TrackTimeline::max_correction_ms`] so a single step can never yank the timeline.
//! The mapping is additionally forced non-decreasing, so session time is monotonic
//! regardless of noisy input.
//!
//! Every mapped timestamp retains its original native value and the exact adjustment
//! applied ([`MappedTimestamp`]), so provenance is auditable (the data model keeps the
//! native rate on `audio_track`; the adjustment rides alongside the derived segment).

/// An online, numerically-stable least-squares estimator of the affine map
/// `session_ms = slope * native + intercept`.
#[derive(Debug, Clone)]
pub struct DriftEstimator {
    n: u64,
    // Sums maintained in f64 for a closed-form ordinary-least-squares fit.
    sum_x: f64,
    sum_y: f64,
    sum_xx: f64,
    sum_xy: f64,
    slope: f64,
    intercept: f64,
}

impl DriftEstimator {
    /// Create an estimator seeded with a nominal slope (ms per native unit) and a zero
    /// intercept. The nominal slope is used until at least two observations arrive.
    #[must_use]
    pub fn new(nominal_slope: f64) -> Self {
        Self {
            n: 0,
            sum_x: 0.0,
            sum_y: 0.0,
            sum_xx: 0.0,
            sum_xy: 0.0,
            slope: nominal_slope,
            intercept: 0.0,
        }
    }

    /// Fold in one `(native, session_ms)` calibration observation and refit.
    pub fn observe(&mut self, native: i64, session_ms: f64) {
        let x = native as f64;
        let y = session_ms;
        self.n += 1;
        self.sum_x += x;
        self.sum_y += y;
        self.sum_xx += x * x;
        self.sum_xy += x * y;
        if self.n >= 2 {
            let n = self.n as f64;
            let denom = n * self.sum_xx - self.sum_x * self.sum_x;
            if denom.abs() > f64::EPSILON {
                self.slope = (n * self.sum_xy - self.sum_x * self.sum_y) / denom;
                self.intercept = (self.sum_y - self.slope * self.sum_x) / n;
            }
        } else {
            // With a single point, keep the nominal slope and solve the intercept.
            self.intercept = y - self.slope * x;
        }
    }

    /// Current best-fit slope (ms per native unit).
    #[must_use]
    pub fn slope(&self) -> f64 {
        self.slope
    }

    /// Current best-fit intercept (ms).
    #[must_use]
    pub fn intercept(&self) -> f64 {
        self.intercept
    }

    /// Number of observations folded in.
    #[must_use]
    pub fn observations(&self) -> u64 {
        self.n
    }

    /// Predicted session-ms for a native timestamp under the current fit.
    #[must_use]
    pub fn predict(&self, native: i64) -> f64 {
        self.slope * native as f64 + self.intercept
    }
}

/// A native timestamp mapped onto the session timeline, retaining full provenance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MappedTimestamp {
    /// The original per-track native timestamp (sample counter or device clock).
    pub native: i64,
    /// The corrected, monotonic session time in milliseconds.
    pub session_ms: i64,
    /// The raw committed-mapping value before the monotonic clamp (ms).
    pub raw_session_ms: f64,
    /// Total drift adjustment applied to the committed mapping so far, in ms
    /// (accumulated across boundary reconciliations).
    pub cumulative_adjustment_ms: f64,
}

/// Per-track monotonic session-timeline mapping with conservative drift correction.
#[derive(Debug, Clone)]
pub struct TrackTimeline {
    estimator: DriftEstimator,
    /// The affine mapping actually used, frozen between safe boundaries.
    committed_slope: f64,
    committed_intercept: f64,
    /// Native timestamp anchoring `session_ms = 0` (the track's first sample).
    native_origin: i64,
    /// Largest correction (ms) a single boundary reconciliation may apply.
    max_correction_ms: f64,
    /// Last session-ms emitted, to enforce monotonicity.
    last_session_ms: i64,
    /// Running total of applied adjustment for provenance.
    cumulative_adjustment_ms: f64,
}

impl TrackTimeline {
    /// Create a timeline for a track.
    ///
    /// * `native_origin` — the native timestamp that corresponds to session time 0
    ///   (typically the track's first captured sample/frame timestamp).
    /// * `nominal_slope` — ms per native unit (e.g. `1000.0 / sample_rate` when native
    ///   timestamps are sample counts, or `1e-6` when they are nanoseconds).
    /// * `max_correction_ms` — the per-boundary correction clamp (conservative, e.g.
    ///   `5.0`).
    #[must_use]
    pub fn new(native_origin: i64, nominal_slope: f64, max_correction_ms: f64) -> Self {
        Self {
            estimator: DriftEstimator::new(nominal_slope),
            committed_slope: nominal_slope,
            committed_intercept: -nominal_slope * native_origin as f64,
            native_origin,
            max_correction_ms: max_correction_ms.abs(),
            last_session_ms: i64::MIN,
            cumulative_adjustment_ms: 0.0,
        }
    }

    /// Feed a calibration observation pairing a native timestamp with an independently
    /// measured session time (e.g. a wall-clock delta from session start). This trains
    /// the drift estimator but does not change the committed mapping until the next
    /// [`reconcile_at_boundary`](Self::reconcile_at_boundary).
    pub fn observe(&mut self, native: i64, session_ms: f64) {
        self.estimator.observe(native, session_ms);
    }

    /// The committed (frozen) mapping value for a native timestamp, before clamping.
    fn committed_raw(&self, native: i64) -> f64 {
        self.committed_slope * native as f64 + self.committed_intercept
    }

    /// Map a native timestamp to a monotonic session timestamp using the currently
    /// committed mapping. Monotonicity is enforced: the result never goes backward.
    pub fn map(&mut self, native: i64) -> MappedTimestamp {
        let raw = self.committed_raw(native);
        let mut session_ms = raw.round() as i64;
        if session_ms < self.last_session_ms {
            session_ms = self.last_session_ms;
        }
        self.last_session_ms = session_ms;
        MappedTimestamp {
            native,
            session_ms,
            raw_session_ms: raw,
            cumulative_adjustment_ms: self.cumulative_adjustment_ms,
        }
    }

    /// At a caller-chosen **safe boundary** (a silence gap where warping time is
    /// inaudible), nudge the committed mapping toward the latest drift estimate. The
    /// applied correction is clamped to `max_correction_ms`. Returns the correction
    /// actually applied, in milliseconds.
    ///
    /// `native_now` is the native timestamp at the boundary; the correction is applied
    /// as an intercept shift so times already emitted are unaffected and future times
    /// converge toward the estimate.
    pub fn reconcile_at_boundary(&mut self, native_now: i64) -> f64 {
        if self.estimator.observations() < 2 {
            return 0.0;
        }
        let target = self.estimator.predict(native_now);
        let current = self.committed_raw(native_now);
        let delta = target - current;
        let applied = delta.clamp(-self.max_correction_ms, self.max_correction_ms);
        // Adopt the estimator's slope but shift the intercept so that at `native_now`
        // the mapping moves by exactly `applied` (not the full delta).
        let new_slope = self.estimator.slope();
        // committed_raw(native_now) after update must equal current + applied.
        let new_intercept = (current + applied) - new_slope * native_now as f64;
        self.committed_slope = new_slope;
        self.committed_intercept = new_intercept;
        self.cumulative_adjustment_ms += applied;
        applied
    }

    /// The native timestamp anchoring session time zero.
    #[must_use]
    pub fn native_origin(&self) -> i64 {
        self.native_origin
    }

    /// Total drift adjustment applied across all boundary reconciliations (ms).
    #[must_use]
    pub fn cumulative_adjustment_ms(&self) -> f64 {
        self.cumulative_adjustment_ms
    }

    /// The current committed slope (ms per native unit).
    #[must_use]
    pub fn committed_slope(&self) -> f64 {
        self.committed_slope
    }

    /// The current committed intercept (ms). Together with
    /// [`committed_slope`](Self::committed_slope) this fully describes the frozen
    /// affine mapping, without triggering the monotonic clamp of [`map`](Self::map).
    #[must_use]
    pub fn committed_intercept(&self) -> f64 {
        self.committed_intercept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimator_recovers_known_line() {
        // session_ms = 0.02 * native + 5  (e.g. native = samples at 50 kHz-ish)
        let mut est = DriftEstimator::new(0.02);
        for k in 0..100 {
            let native = k * 1000;
            est.observe(native, 0.02 * native as f64 + 5.0);
        }
        assert!((est.slope() - 0.02).abs() < 1e-9, "slope {}", est.slope());
        assert!(
            (est.intercept() - 5.0).abs() < 1e-6,
            "intercept {}",
            est.intercept()
        );
    }

    #[test]
    fn nominal_mapping_before_calibration() {
        // native are sample counts at 16 kHz -> slope 1000/16000 = 0.0625 ms/sample.
        let slope = 1000.0 / 16_000.0;
        let mut tl = TrackTimeline::new(0, slope, 5.0);
        // 16000 samples in = 1000 ms.
        let m = tl.map(16_000);
        assert_eq!(m.session_ms, 1000);
        assert_eq!(m.native, 16_000);
    }

    #[test]
    fn origin_maps_to_zero() {
        let slope = 1000.0 / 48_000.0;
        let mut tl = TrackTimeline::new(4242, slope, 5.0);
        let m = tl.map(4242);
        assert_eq!(m.session_ms, 0);
    }

    #[test]
    fn mapping_is_monotonic_under_backward_input() {
        let slope = 1000.0 / 16_000.0;
        let mut tl = TrackTimeline::new(0, slope, 5.0);
        let a = tl.map(16_000).session_ms; // 1000
                                           // A native timestamp that goes backward must not produce a smaller session ms.
        let b = tl.map(8_000).session_ms;
        assert!(b >= a, "expected monotonic, got {a} then {b}");
    }

    #[test]
    fn boundary_correction_is_clamped() {
        let slope = 1000.0 / 16_000.0;
        let mut tl = TrackTimeline::new(0, slope, 5.0);
        // Train the estimator to imply a big offset (+100 ms) vs the committed mapping.
        for k in 1..50 {
            let native = k * 16_000; // one second steps
                                     // True session time is 100 ms ahead of the nominal mapping.
            tl.observe(native, slope * native as f64 + 100.0);
        }
        let applied = tl.reconcile_at_boundary(50 * 16_000);
        // The full delta is ~100 ms but the clamp caps a single step at 5 ms.
        assert!((applied - 5.0).abs() < 1e-6, "applied {applied}");
        assert!((tl.cumulative_adjustment_ms() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn repeated_boundaries_converge_within_clamp() {
        let slope = 1000.0 / 16_000.0;
        let mut tl = TrackTimeline::new(0, slope, 5.0);
        for k in 1..50 {
            let native = k * 16_000;
            tl.observe(native, slope * native as f64 + 100.0);
        }
        let mut total = 0.0;
        // Each boundary can add at most 5 ms; 30 boundaries move up to 150 ms > 100.
        for i in 1..=30 {
            total += tl.reconcile_at_boundary((50 + i) * 16_000).abs();
        }
        // Converged: committed mapping now within a step of the target.
        let native_probe = 80 * 16_000;
        let committed = tl.committed_slope() * native_probe as f64 + tl.committed_intercept();
        let expected = slope * native_probe as f64 + 100.0;
        let residual = (committed - expected).abs();
        assert!(residual <= 5.0 + 1e-6, "residual {residual}");
        assert!(total > 0.0);
    }
}
