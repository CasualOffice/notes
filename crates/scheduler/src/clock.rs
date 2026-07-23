//! A tiny clock abstraction so Layer A and the catch-up sweep are testable under a
//! simulated clock (N7 fire-tolerance / de-dup / catch-up unit tests).

use app_domain::Timestamp;

/// A source of "now" in epoch-milliseconds UTC.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// The current instant.
    fn now(&self) -> Timestamp;

    /// Current instant as raw epoch-ms (convenience).
    fn now_ms(&self) -> i64 {
        self.now().as_millis()
    }
}

/// The real wall clock (`app_domain::Timestamp::now`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A manually-advanced clock for deterministic tests.
#[derive(Debug, Clone)]
pub struct SimClock {
    now_ms: std::sync::Arc<std::sync::atomic::AtomicI64>,
}

impl SimClock {
    /// Start at `start_ms`.
    #[must_use]
    pub fn new(start_ms: i64) -> Self {
        Self {
            now_ms: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(start_ms)),
        }
    }

    /// Jump the clock to an absolute epoch-ms instant.
    pub fn set(&self, ms: i64) {
        self.now_ms.store(ms, std::sync::atomic::Ordering::SeqCst);
    }

    /// Advance the clock by `delta_ms`.
    pub fn advance(&self, delta_ms: i64) {
        self.now_ms
            .fetch_add(delta_ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for SimClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.now_ms.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_clock_advances_and_sets() {
        let c = SimClock::new(1_000);
        assert_eq!(c.now_ms(), 1_000);
        c.advance(500);
        assert_eq!(c.now_ms(), 1_500);
        c.set(42);
        assert_eq!(c.now_ms(), 42);
    }
}
