//! Liveness heartbeat (spec 01 R16, spec 09 H9).
//!
//! A dedicated timer task writes a monotonically increasing counter to a file;
//! the watchdog reads it and restarts the Brain if it stops advancing. R16 is
//! the load-bearing rule: the heartbeat is written HERE, on its own interval —
//! never from the dispatch loop or a request path — so a legitimate 80-second
//! generation (RV-03) is never mistaken for a hang.
//!
//! Write is atomic (temp file + rename) so the watchdog never reads a torn
//! value. Failure to write is logged, not fatal — a transient FS hiccup should
//! not kill the Brain; a *persistent* one correctly reads as a hang and the
//! watchdog restarts us, which is the intended safety behavior.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Shared beat counter. The heartbeat task owns the increment; other code may
/// read it for diagnostics.
#[derive(Clone, Default)]
pub struct Heartbeat {
    counter: Arc<AtomicU64>,
}

impl Heartbeat {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
    }

    /// Advance and persist one beat. Exposed for tests; the running system
    /// calls it from [`spawn`] on a timer.
    pub fn beat(&self, path: &Path) -> std::io::Result<u64> {
        let next = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        write_atomic(path, next.to_string().as_bytes())?;
        Ok(next)
    }

    /// Spawn the dedicated heartbeat timer (R16). Returns the task handle; it
    /// runs until aborted (shutdown). Interval must be well under the
    /// watchdog's `poll_interval × max_missed` threshold.
    pub fn spawn(&self, path: PathBuf, interval: Duration) -> tokio::task::JoinHandle<()> {
        let hb = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = hb.beat(&path) {
                    tracing::warn!(error = %e, "heartbeat write failed (transient FS?); \
                        persistent failure will correctly trip the watchdog");
                }
            }
        })
    }
}

/// Read the current heartbeat counter (watchdog side / diagnostics).
/// Missing or unparseable file → `None` (the watchdog treats that as a miss).
pub fn read_beat(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Atomic write: temp sibling + fsync + rename. The reader sees either the old
/// value or the new one, never a partial line.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_data()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_increments_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hb");
        let hb = Heartbeat::new();

        assert_eq!(hb.beat(&path).unwrap(), 1);
        assert_eq!(hb.beat(&path).unwrap(), 2);
        assert_eq!(hb.current(), 2);
        assert_eq!(read_beat(&path), Some(2));
    }

    #[test]
    fn read_missing_file_is_none() {
        assert_eq!(read_beat(Path::new("/nonexistent/hb")), None);
    }

    #[test]
    fn read_garbage_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hb");
        std::fs::write(&path, "not-a-number").unwrap();
        assert_eq!(read_beat(&path), None);
    }

    // R16 behaviour proxy: the counter advances on its own timer, independent
    // of any request work. Real time (tokio's test-util paused-clock isn't in
    // the "full" feature set); short interval keeps it fast.
    #[tokio::test]
    async fn spawned_task_advances_counter_on_timer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hb");
        let hb = Heartbeat::new();
        let handle = hb.spawn(path.clone(), Duration::from_millis(20));

        tokio::time::sleep(Duration::from_millis(120)).await;

        assert!(
            hb.current() >= 3,
            "counter should have ticked several times, got {}",
            hb.current()
        );
        // Persisted value tracks the counter.
        assert_eq!(read_beat(&path), Some(hb.current()));
        handle.abort();
    }
}
