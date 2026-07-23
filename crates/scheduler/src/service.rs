//! The async **Layer-A driver**: one Tokio task owning the [`TimerWheel`], sleeping
//! until the next `fire_at`, and delivering fired reminders through the state-gated
//! [`FireGate`] so a reminder fires **exactly once across both layers** (HLD §8.3).
//!
//! The task is command-driven over an mpsc channel ([`SchedulerHandle`]). Wiring
//! order for a mutation follows Architecture §6.1 invariant 1: the caller writes
//! SQLite first, then [`SchedulerHandle::arm`]/[`disarm`](SchedulerHandle::disarm)
//! patches Layer A, then [`crate::horizon::reconcile`] re-syncs Layer B.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use app_domain::{EntityRef, ReminderId};
use tokio::sync::mpsc;

use crate::capability::ScheduledReminder;
use crate::clock::Clock;
use crate::error::SchedulerError;
use crate::gate::FireGate;
use crate::wheel::TimerWheel;

/// Idle poll interval when nothing is armed (bounds worst-case wake latency).
const IDLE_POLL: Duration = Duration::from_secs(3600);

/// A reminder the driver decided to deliver (after winning the de-dup claim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiredReminder {
    pub reminder_id: ReminderId,
    pub target: Option<EntityRef>,
    /// `true` only for the coalesced catch-up notification (HLD §8.3).
    pub grouped: bool,
}

/// Where delivered reminders go. The app-service implementation emits
/// `AppEvent::ReminderFired` and shows the native notification; kept a trait so the
/// scheduler stays free of Tauri/UI concerns.
pub trait DeliverySink: Send + Sync {
    fn deliver(&self, fired: FiredReminder);
}

/// Everything the driver task needs.
pub struct SchedulerConfig {
    pub clock: Arc<dyn Clock>,
    pub gate: Arc<dyn FireGate>,
    pub sink: Arc<dyn DeliverySink>,
}

impl std::fmt::Debug for SchedulerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulerConfig").finish_non_exhaustive()
    }
}

enum Command {
    Arm(ScheduledReminder),
    Disarm(ReminderId),
    Rebuild(Vec<ScheduledReminder>),
    Shutdown,
}

/// A handle to the running Layer-A driver task.
#[derive(Debug, Clone)]
pub struct SchedulerHandle {
    tx: mpsc::Sender<Command>,
}

impl SchedulerHandle {
    /// Patch Layer A: arm (or re-arm) a reminder at its effective fire instant.
    pub async fn arm(&self, r: ScheduledReminder) -> Result<(), SchedulerError> {
        self.send(Command::Arm(r)).await
    }

    /// Patch Layer A: remove a reminder (edit/complete/cancel).
    pub async fn disarm(&self, id: ReminderId) -> Result<(), SchedulerError> {
        self.send(Command::Disarm(id)).await
    }

    /// Rebuild Layer A from SQLite on boot / wake.
    pub async fn rebuild(&self, active: Vec<ScheduledReminder>) -> Result<(), SchedulerError> {
        self.send(Command::Rebuild(active)).await
    }

    /// Stop the driver task.
    pub async fn shutdown(&self) -> Result<(), SchedulerError> {
        self.send(Command::Shutdown).await
    }

    async fn send(&self, cmd: Command) -> Result<(), SchedulerError> {
        self.tx
            .send(cmd)
            .await
            .map_err(|_| SchedulerError::NotRunning)
    }
}

/// Spawn the Layer-A driver task, returning a handle to command it.
#[must_use]
pub fn spawn(config: SchedulerConfig) -> SchedulerHandle {
    let (tx, rx) = mpsc::channel(128);
    tokio::spawn(run_loop(config, rx));
    SchedulerHandle { tx }
}

async fn run_loop(config: SchedulerConfig, mut rx: mpsc::Receiver<Command>) {
    let mut wheel = TimerWheel::new();
    // id -> descriptor, so delivery can carry the target for deep-linking.
    let mut meta: HashMap<ReminderId, ScheduledReminder> = HashMap::new();

    loop {
        // 1. Deliver anything already due (state-gated de-dup).
        let now = config.clock.now_ms();
        for id in wheel.pop_due(now) {
            deliver(&config, &meta, id, false);
            meta.remove(&id);
        }

        // 2. Sleep until the next deadline or a command, whichever comes first.
        let next = wheel.next_fire_at();
        let sleep_for = match next {
            Some(t) => Duration::from_millis((t - config.clock.now_ms()).max(0) as u64),
            None => IDLE_POLL,
        };

        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => { /* re-loop: pop_due handles it */ }
            cmd = rx.recv() => match cmd {
                Some(Command::Arm(r)) => {
                    wheel.arm(r.reminder_id, r.fire_at.as_millis());
                    meta.insert(r.reminder_id, r);
                }
                Some(Command::Disarm(id)) => {
                    wheel.disarm(id);
                    meta.remove(&id);
                }
                Some(Command::Rebuild(active)) => {
                    meta = active.iter().map(|r| (r.reminder_id, r.clone())).collect();
                    wheel.rebuild(active.into_iter().map(|r| (r.reminder_id, r.fire_at.as_millis())));
                }
                Some(Command::Shutdown) | None => break,
            }
        }
    }
}

fn deliver(
    config: &SchedulerConfig,
    meta: &HashMap<ReminderId, ScheduledReminder>,
    id: ReminderId,
    grouped: bool,
) {
    match config.gate.try_claim(id) {
        Ok(true) => {
            let target = meta.get(&id).and_then(|m| m.target);
            config.sink.deliver(FiredReminder {
                reminder_id: id,
                target,
                grouped,
            });
        }
        // Already claimed by Layer B (or terminal) — de-dup no-op.
        Ok(false) => tracing::debug!(reminder = %id, "layer A fire de-duped (already claimed)"),
        Err(e) => tracing::warn!(reminder = %id, error = %e, "fire-gate claim failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;
    use crate::gate::InMemoryGate;
    use app_domain::{Id, Timestamp};
    use reminders::ReminderState;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        fired: Mutex<Vec<FiredReminder>>,
    }
    impl DeliverySink for RecordingSink {
        fn deliver(&self, fired: FiredReminder) {
            self.fired.lock().unwrap().push(fired);
        }
    }

    fn descriptor(id: ReminderId, fire_ms: i64) -> ScheduledReminder {
        ScheduledReminder {
            reminder_id: id,
            fire_at: Timestamp::from_millis(fire_ms),
            tz: "UTC".into(),
            body: None,
            target: None,
        }
    }

    // Real (short) timing: the deterministic guarantees are proven in wheel/gate/
    // catchup unit tests; this proves the async wiring delivers within tolerance.
    #[tokio::test]
    async fn driver_fires_due_reminder_once() {
        let clock = Arc::new(SystemClock);
        let gate = Arc::new(InMemoryGate::new());
        let sink = Arc::new(RecordingSink::default());

        let id = Id::new();
        gate.set_state(id, ReminderState::Pending);

        let handle = spawn(SchedulerConfig {
            clock: clock.clone(),
            gate: gate.clone(),
            sink: sink.clone(),
        });

        // Fire ~40ms from now.
        let fire_at = clock.now_ms() + 40;
        handle.arm(descriptor(id, fire_at)).await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown().await.ok();

        let fired = sink.fired.lock().unwrap();
        assert_eq!(fired.len(), 1, "fired exactly once");
        assert_eq!(fired[0].reminder_id, id);
        assert_eq!(gate.state(id), Some(ReminderState::Fired));
    }

    #[tokio::test]
    async fn layer_b_preclaim_suppresses_layer_a() {
        let clock = Arc::new(SystemClock);
        let gate = Arc::new(InMemoryGate::new());
        let sink = Arc::new(RecordingSink::default());

        let id = Id::new();
        gate.set_state(id, ReminderState::Pending);
        // Simulate Layer B already delivering via the deep-link path.
        assert!(gate.try_claim(id).unwrap());

        let handle = spawn(SchedulerConfig {
            clock: clock.clone(),
            gate: gate.clone(),
            sink: sink.clone(),
        });
        handle
            .arm(descriptor(id, clock.now_ms() + 20))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        handle.shutdown().await.ok();

        assert!(
            sink.fired.lock().unwrap().is_empty(),
            "Layer A de-duped against Layer B"
        );
    }
}
