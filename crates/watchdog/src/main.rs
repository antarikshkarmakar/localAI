//! localai-watchdog binary: tiny supervisor for Brain process.
//!
//! Reads heartbeat file, applies policy, restarts process on hang/crash.
//! All decisions are in the lib; this is just I/O and process control.

use localai_watchdog::{Action, PolicyConfig, WatchdogPolicy};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

/// Environment variable names.
const ENV_CMD: &str = "LOCALAI_WATCHDOG_CMD";
const ENV_HEARTBEAT_PATH: &str = "LOCALAI_WATCHDOG_HEARTBEAT_PATH";
const ENV_POLL_INTERVAL_MS: &str = "LOCALAI_WATCHDOG_POLL_INTERVAL_MS";
const ENV_MAX_MISSED: &str = "LOCALAI_WATCHDOG_MAX_MISSED";
const ENV_BACKOFF_INITIAL_MS: &str = "LOCALAI_WATCHDOG_BACKOFF_INITIAL_MS";
const ENV_BACKOFF_MAX_MS: &str = "LOCALAI_WATCHDOG_BACKOFF_MAX_MS";

/// Configuration loaded from environment.
struct WatchdogConfig {
    /// Command to spawn the process.
    cmd: String,
    /// Path to heartbeat file.
    heartbeat_path: PathBuf,
    /// Poll interval in milliseconds.
    poll_interval_ms: u64,
    /// Policy config.
    policy_config: PolicyConfig,
}

impl WatchdogConfig {
    /// Load from environment, with defaults.
    fn from_env() -> Result<Self, String> {
        let cmd = env::var(ENV_CMD).map_err(|_| {
            format!(
                "missing required env var {}: should be the command to spawn Brain",
                ENV_CMD
            )
        })?;

        let heartbeat_path = env::var(ENV_HEARTBEAT_PATH)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".localai-heartbeat"));

        let poll_interval_ms: u64 = env::var(ENV_POLL_INTERVAL_MS)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000);

        let max_missed: usize = env::var(ENV_MAX_MISSED)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);

        let backoff_initial_ms: u64 = env::var(ENV_BACKOFF_INITIAL_MS)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);

        let backoff_max_ms: u64 = env::var(ENV_BACKOFF_MAX_MS)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30_000);

        let policy_config = PolicyConfig {
            max_missed_polls: max_missed,
            backoff_initial_ms,
            backoff_max_ms,
            backoff_multiplier: 2.0,
        };

        Ok(WatchdogConfig {
            cmd,
            heartbeat_path,
            poll_interval_ms,
            policy_config,
        })
    }
}

/// Read the heartbeat counter from the file.
/// Returns None if the file doesn't exist or is unreadable.
fn read_heartbeat(path: &PathBuf) -> Option<u64> {
    match fs::read_to_string(path) {
        Ok(content) => content.trim().parse::<u64>().ok().or_else(|| {
            warn!("heartbeat file contains non-numeric value: {}", content);
            None
        }),
        Err(_) => {
            // File doesn't exist or is unreadable; treated as a miss.
            None
        }
    }
}

/// Kill the process with the given PID.
/// First tries SIGTERM, then waits a grace period, then SIGKILL (Unix only).
#[cfg(unix)]
fn kill_process(pid: u32, grace_ms: u64) -> Result<(), String> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let nix_pid = Pid::from_raw(pid as i32);

    // Try SIGTERM first
    if let Err(e) = kill(nix_pid, Signal::SIGTERM) {
        warn!("failed to send SIGTERM to {}: {}", pid, e);
        // Process may already be dead; continue.
    } else {
        info!("sent SIGTERM to process {}", pid);
    }

    // Wait for grace period
    thread::sleep(Duration::from_millis(grace_ms));

    // If still alive, send SIGKILL
    if let Err(e) = kill(nix_pid, Signal::SIGKILL) {
        // Might already be dead
        info!("SIGKILL {} (already dead?): {}", pid, e);
    } else {
        info!("sent SIGKILL to process {}", pid);
    }

    Ok(())
}

/// Kill the process (non-Unix fallback: placeholder).
#[cfg(not(unix))]
fn kill_process(pid: u32, _grace_ms: u64) -> Result<(), String> {
    // On Windows, would use taskkill or Windows process APIs.
    // For now, a placeholder.
    warn!(
        "kill_process not fully implemented on this platform; would terminate pid {}",
        pid
    );
    Ok(())
}

/// Spawn a new child process with the given command.
/// Returns the child's PID.
fn spawn_process(cmd: &str) -> Result<u32, String> {
    // Simple shell invocation. In production, parse the command more carefully.
    let child = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", cmd])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else {
        Command::new("bash")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };

    match child {
        Ok(c) => {
            let pid = c.id();
            info!("spawned process: cmd='{}', pid={}", cmd, pid);
            Ok(pid)
        }
        Err(e) => Err(format!("failed to spawn process: {}", e)),
    }
}

/// Main watchdog loop.
fn run_watchdog(config: WatchdogConfig) -> Result<(), String> {
    info!(
        "starting watchdog: cmd='{}', heartbeat='{}'",
        config.cmd,
        config.heartbeat_path.display()
    );

    let mut policy = WatchdogPolicy::new(config.policy_config);
    let mut health_counter = 0u32;

    // Spawn the initial process.
    let mut current_pid: Option<u32> = Some(spawn_process(&config.cmd)?);

    loop {
        // Sleep for the poll interval
        thread::sleep(Duration::from_millis(config.poll_interval_ms));

        // Read the heartbeat
        let counter = read_heartbeat(&config.heartbeat_path);

        // Feed the policy
        let action = policy.observe(counter);

        match action {
            Action::None => {
                health_counter = health_counter.saturating_add(1);
                // Every 10 healthy polls, acknowledge sustained health
                if health_counter >= 10 {
                    policy.acknowledge_health();
                    health_counter = 0;
                }
            }
            Action::Restart => {
                health_counter = 0;
                warn!(
                    "watchdog: restarting process (backoff={}ms, consecutive_restarts={})",
                    policy.current_backoff_ms(),
                    policy.consecutive_restarts()
                );

                // Kill the old process
                if let Some(pid) = current_pid {
                    if let Err(e) = kill_process(pid, 5000) {
                        error!("failed to kill process {}: {}", pid, e);
                    }
                }

                // Apply the backoff delay before spawning
                thread::sleep(Duration::from_millis(policy.current_backoff_ms()));

                // Spawn the new process
                match spawn_process(&config.cmd) {
                    Ok(new_pid) => {
                        current_pid = Some(new_pid);
                        policy.acknowledge_restart();
                    }
                    Err(e) => {
                        error!("failed to spawn new process: {}", e);
                        // Continue the loop; next iteration will try again
                    }
                }
            }
        }
    }
}

fn main() {
    // Initialize tracing (basic setup; production would use a full subscriber)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let config = match WatchdogConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!("watchdog config error: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = run_watchdog(config) {
        error!("watchdog error: {}", e);
        std::process::exit(1);
    }
}
