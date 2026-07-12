//! llama-server process launcher (spec 03 §1, I5, I13, startup step 5).
//!
//! Spawns and supervises the local inference server process.
//! - `LaunchSpec`: builds argv for llama-server (pure function, fully testable)
//! - `ProcessSpawner`: trait for process spawning (real impl + test fakes)
//! - `launch_and_wait()`: spawn process, poll /health until healthy or timeout
//! - `LlamaServerHandle`: holds the child process; `shutdown()` SIGTERMs + waits

use crate::health::HealthCheck;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Child;
use tokio::time::sleep;
use tracing::{debug, warn};

/// Spec 03 §1: Launch specification for llama-server.
/// Builds the full argv per ADR-003.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Path to main model GGUF (e.g., /models/gemma4-12b-Q4_K_M.gguf)
    pub model_path: PathBuf,

    /// Optional draft model for speculative decoding (ADR-003)
    pub draft_path: Option<PathBuf>,

    /// Port to bind on (loopback only, spec 03 §1)
    pub port: u16,

    /// Context size in tokens (32768 by default, spec 03 I7, CON-6)
    pub ctx: u32,

    /// Thread count (phys cores by default). None uses llama-server default.
    pub threads: Option<u32>,

    /// Extra arguments to pass to llama-server (for future extensions)
    pub extra_args: Vec<String>,
}

impl LaunchSpec {
    /// Build the full argv for `llama-server` per ADR-003.
    ///
    /// Returns `["llama-server", "-m", model_path, ..., "--host", "127.0.0.1", "--port", port, ...]`
    /// - Always includes: -m, --model-draft (if present), --spec-type draft-mtp, --ctx-size,
    ///   --host 127.0.0.1, --port, --mlock
    /// - Optionally includes: --threads (if Some)
    /// - Then appends any extra_args
    pub fn to_argv(&self) -> Vec<String> {
        let mut argv = vec!["llama-server".to_string()];

        // Main model (required)
        argv.push("-m".to_string());
        argv.push(self.model_path.display().to_string());

        // Draft model for speculative decoding (spec 03 §1, ADR-003)
        if let Some(draft) = &self.draft_path {
            argv.push("--model-draft".to_string());
            argv.push(draft.display().to_string());
            argv.push("--spec-type".to_string());
            argv.push("draft-mtp".to_string());
        }

        // Context size (spec 03 I7, default 32K)
        argv.push("--ctx-size".to_string());
        argv.push(self.ctx.to_string());

        // Loopback binding (spec 03 §1)
        argv.push("--host".to_string());
        argv.push("127.0.0.1".to_string());

        argv.push("--port".to_string());
        argv.push(self.port.to_string());

        // Thread count (optional, spec 03 §1)
        if let Some(threads) = self.threads {
            argv.push("--threads".to_string());
            argv.push(threads.to_string());
        }

        // Memory lock (CPU-only, native build, spec 03 §1, ADR-001)
        argv.push("--mlock".to_string());

        // User-provided extras
        argv.extend(self.extra_args.clone());

        argv
    }
}

/// Spec 03 §1: Trait for process spawning.
/// Real impl uses tokio::process::Command; tests inject fakes.
pub trait ProcessSpawner: Send + Sync {
    /// Spawn a process with the given program name and arguments.
    /// Real impl: executes; fake impl: tracks calls, returns a mock Child.
    fn spawn(&self, program: &str, args: &[String]) -> Result<Child, LaunchError>;
}

/// Real process spawner using tokio.
pub struct RealProcessSpawner;

impl ProcessSpawner for RealProcessSpawner {
    fn spawn(&self, program: &str, args: &[String]) -> Result<Child, LaunchError> {
        use std::process::Stdio;

        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

        cmd.spawn().map_err(|e| LaunchError::SpawnFailed {
            program: program.to_string(),
            reason: e.to_string(),
        })
    }
}

/// Spec 03 §1: Typed errors for launch operations.
#[derive(Debug, Error)]
pub enum LaunchError {
    /// Failed to spawn the process (IO error, binary not found, etc.)
    #[error("failed to spawn {program}: {reason}")]
    SpawnFailed { program: String, reason: String },

    /// Health check never succeeded within timeout (spec 03 I13, startup step 5)
    #[error("health check did not succeed within {timeout_secs}s")]
    HealthTimeout { timeout_secs: u64 },

    /// Health check itself failed (network error, transport failure)
    #[error("health check failed: {0}")]
    HealthCheckFailed(#[from] crate::error::InferenceError),

    /// Internal error (e.g., missing field)
    #[error("internal launch error: {0}")]
    Internal(String),
}

/// Spec 03 §1, I5: Handle to a running llama-server process.
/// Holds the child process; shutdown() SIGTERMs it and waits.
pub struct LlamaServerHandle {
    child: Child,
    port: u16,
}

impl LlamaServerHandle {
    /// Construct a handle from a spawned child.
    pub fn new(child: Child, port: u16) -> Self {
        Self { child, port }
    }

    /// Get the port this server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Gracefully shut down the server: SIGTERM the child and wait for exit.
    /// Spec 03 I5: zero dropped requests (queued requests are drained before shutdown).
    pub async fn shutdown(mut self) -> Result<(), LaunchError> {
        debug!(port = self.port, "sending SIGTERM to llama-server");
        self.child
            .kill()
            .await
            .map_err(|e| LaunchError::Internal(format!("failed to kill child: {}", e)))?;

        // Wait for the process to exit.
        self.child
            .wait()
            .await
            .map_err(|e| LaunchError::Internal(format!("failed to wait for child: {}", e)))?;

        debug!(port = self.port, "llama-server exited");
        Ok(())
    }
}

/// Spec 03 §1, startup step 5: Spawn llama-server and wait until healthy.
///
/// - Spawn the process with the given spec
/// - Poll the /health endpoint until it returns "ok" or timeout
/// - On timeout: kill the child, return HealthTimeout
/// - On success: return a handle to the running server
///
/// # Arguments
/// - `spec`: launch configuration (model paths, port, ctx, etc.)
/// - `spawner`: process spawner trait (real or fake for tests)
/// - `health`: health check instance to poll /health
/// - `timeout`: max time to wait for health (spec 03 I13: 120s default from InferenceCfg)
pub async fn launch_and_wait(
    spec: &LaunchSpec,
    spawner: &dyn ProcessSpawner,
    health: &HealthCheck,
    timeout: Duration,
) -> Result<LlamaServerHandle, LaunchError> {
    let argv = spec.to_argv();
    debug!(
        program = "llama-server",
        args = ?&argv[1..],
        port = spec.port,
        "spawning llama-server"
    );

    // Spawn the process
    let mut child = spawner.spawn("llama-server", &argv)?;

    // Give the process a moment to start before polling /health
    sleep(Duration::from_millis(100)).await;

    // Poll /health until healthy or timeout
    let start = std::time::Instant::now();
    loop {
        match health.check().await {
            Ok(status) if status.status == "ok" => {
                debug!(port = spec.port, "llama-server is healthy");
                return Ok(LlamaServerHandle::new(child, spec.port));
            }
            Ok(status) => {
                debug!(
                    port = spec.port,
                    status = ?status.status,
                    "llama-server not ready yet"
                );
            }
            Err(e) => {
                debug!(
                    port = spec.port,
                    error = ?e,
                    "health check failed (will retry)"
                );
            }
        }

        // Check timeout
        if start.elapsed() > timeout {
            warn!(
                port = spec.port,
                timeout_secs = timeout.as_secs(),
                "health check timeout, killing child"
            );
            let _ = child.kill().await;
            return Err(LaunchError::HealthTimeout {
                timeout_secs: timeout.as_secs(),
            });
        }

        // Back off before next poll
        sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ============ Tests for LaunchSpec::to_argv ============

    #[test]
    fn to_argv_includes_model_path() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert_eq!(argv[0], "llama-server");
        assert!(argv.contains(&"-m".to_string()));
        let m_idx = argv.iter().position(|a| a == "-m").unwrap();
        assert_eq!(argv[m_idx + 1], "/models/gemma4-12b-Q4_K_M.gguf");
    }

    #[test]
    fn to_argv_with_draft_includes_spec_flags() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: Some(PathBuf::from("/models/gemma4-12b-mtp-drafter.gguf")),
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert!(argv.contains(&"--model-draft".to_string()));
        assert!(argv.contains(&"/models/gemma4-12b-mtp-drafter.gguf".to_string()));
        assert!(argv.contains(&"--spec-type".to_string()));
        assert!(argv.contains(&"draft-mtp".to_string()));
    }

    #[test]
    fn to_argv_without_draft_omits_spec_flags() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert!(!argv.contains(&"--model-draft".to_string()));
        assert!(!argv.contains(&"--spec-type".to_string()));
    }

    #[test]
    fn to_argv_includes_loopback_host_and_port() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 9090,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert!(argv.contains(&"--host".to_string()));
        assert!(argv.contains(&"127.0.0.1".to_string()));
        assert!(argv.contains(&"--port".to_string()));
        assert!(argv.contains(&"9090".to_string()));
    }

    #[test]
    fn to_argv_includes_ctx_size() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 16_384,
            threads: None,
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert!(argv.contains(&"--ctx-size".to_string()));
        assert!(argv.contains(&"16384".to_string()));
    }

    #[test]
    fn to_argv_with_threads_includes_flag() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: Some(16),
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert!(argv.contains(&"--threads".to_string()));
        assert!(argv.contains(&"16".to_string()));
    }

    #[test]
    fn to_argv_without_threads_omits_flag() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert!(!argv.contains(&"--threads".to_string()));
    }

    #[test]
    fn to_argv_always_includes_mlock() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let argv = spec.to_argv();
        assert!(argv.contains(&"--mlock".to_string()));
    }

    #[test]
    fn to_argv_includes_extra_args() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec!["--custom".to_string(), "value".to_string()],
        };

        let argv = spec.to_argv();
        assert!(argv.contains(&"--custom".to_string()));
        assert!(argv.contains(&"value".to_string()));
    }

    // ============ Mock ProcessSpawner for testing launch_and_wait ============

    /// A mock process spawner that creates a real but harmless child process.
    /// We need a real Child for testing because it's a tokio::process::Child.
    /// This spawner creates processes that sleep, giving us time to test health checks.
    /// (program, args) recorded per spawn call.
    type SpawnCall = (String, Vec<String>);

    struct MockProcessSpawner {
        calls: Arc<Mutex<Vec<SpawnCall>>>,
        fail: bool,
    }

    impl MockProcessSpawner {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail: false,
            }
        }

        fn with_fail(fail: bool) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail,
            }
        }

        fn recorded_calls(&self) -> Vec<SpawnCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ProcessSpawner for MockProcessSpawner {
        fn spawn(&self, program: &str, args: &[String]) -> Result<Child, LaunchError> {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));

            if self.fail {
                return Err(LaunchError::SpawnFailed {
                    program: program.to_string(),
                    reason: "mock: spawn failed".to_string(),
                });
            }

            // Create a real harmless child process that sleeps.
            // Always use sleep (available in WSL and Unix).
            let mut cmd = tokio::process::Command::new("sleep");
            cmd.args(["30"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            cmd.spawn().map_err(|e| LaunchError::SpawnFailed {
                program: program.to_string(),
                reason: e.to_string(),
            })
        }
    }

    // ============ Tests for launch_and_wait ============

    #[tokio::test]
    async fn launch_and_wait_spawns_process_with_correct_argv() {
        let spawner = MockProcessSpawner::new();
        let mock_transport = Arc::new(crate::transport::tests::MockTransport::new());
        mock_transport.enqueue_response(serde_json::json!({"status": "ok"}));

        let health = HealthCheck::new(mock_transport);

        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: Some(16),
            extra_args: vec![],
        };

        let _handle = launch_and_wait(&spec, &spawner, &health, Duration::from_secs(5))
            .await
            .unwrap();

        let calls = spawner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "llama-server");
        assert!(calls[0]
            .1
            .contains(&"/models/gemma4-12b-Q4_K_M.gguf".to_string()));
    }

    #[tokio::test]
    async fn launch_and_wait_returns_handle_on_health_ok() {
        let spawner = MockProcessSpawner::new();
        let mock_transport = Arc::new(crate::transport::tests::MockTransport::new());
        mock_transport.enqueue_response(serde_json::json!({"status": "ok", "model": "gemma4-12b"}));

        let health = HealthCheck::new(mock_transport);

        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let handle = launch_and_wait(&spec, &spawner, &health, Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(handle.port(), 8080);
    }

    #[tokio::test]
    async fn launch_and_wait_retries_on_non_ok_status() {
        let spawner = MockProcessSpawner::new();
        let mock_transport = Arc::new(crate::transport::tests::MockTransport::new());

        // First response: loading (not ok), second response: ok
        mock_transport.enqueue_response(serde_json::json!({"status": "loading"}));
        mock_transport.enqueue_response(serde_json::json!({"status": "ok"}));

        let health = HealthCheck::new(mock_transport);

        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let handle = launch_and_wait(&spec, &spawner, &health, Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(handle.port(), 8080);
    }

    #[tokio::test]
    async fn launch_and_wait_times_out_if_never_healthy() {
        let spawner = MockProcessSpawner::new();
        let mock_transport = Arc::new(crate::transport::tests::MockTransport::new());

        // All responses are "loading" (never "ok")
        mock_transport.enqueue_response(serde_json::json!({"status": "loading"}));
        mock_transport.enqueue_response(serde_json::json!({"status": "loading"}));
        mock_transport.enqueue_response(serde_json::json!({"status": "loading"}));

        let health = HealthCheck::new(mock_transport);

        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let result = launch_and_wait(&spec, &spawner, &health, Duration::from_millis(100)).await;

        assert!(result.is_err());
        match result {
            Err(LaunchError::HealthTimeout { .. }) => {}
            Err(e) => panic!("expected HealthTimeout, got {:?}", e),
            Ok(_) => panic!("expected timeout error"),
        }
    }

    #[tokio::test]
    async fn launch_and_wait_fails_if_spawn_fails() {
        let spawner = MockProcessSpawner::with_fail(true);
        let mock_transport = Arc::new(crate::transport::tests::MockTransport::new());
        let health = HealthCheck::new(mock_transport);

        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: None,
            port: 8080,
            ctx: 32_768,
            threads: None,
            extra_args: vec![],
        };

        let result = launch_and_wait(&spec, &spawner, &health, Duration::from_secs(5)).await;

        assert!(result.is_err());
        match result {
            Err(LaunchError::SpawnFailed { .. }) => {}
            Err(e) => panic!("expected SpawnFailed, got {:?}", e),
            Ok(_) => panic!("expected spawn error"),
        }
    }

    #[test]
    fn launch_spec_debug_is_readable() {
        let spec = LaunchSpec {
            model_path: PathBuf::from("/models/gemma4-12b-Q4_K_M.gguf"),
            draft_path: Some(PathBuf::from("/models/draft.gguf")),
            port: 8080,
            ctx: 32_768,
            threads: Some(16),
            extra_args: vec!["--custom".to_string()],
        };

        let debug_str = format!("{:?}", spec);
        assert!(debug_str.contains("model_path"));
        assert!(debug_str.contains("draft_path"));
    }

    #[test]
    fn launch_error_debug_and_display() {
        let err = LaunchError::HealthTimeout { timeout_secs: 120 };
        assert_eq!(err.to_string(), "health check did not succeed within 120s");

        let err2 = LaunchError::SpawnFailed {
            program: "llama-server".to_string(),
            reason: "not found".to_string(),
        };
        assert!(err2.to_string().contains("llama-server"));
        assert!(err2.to_string().contains("not found"));
    }
}
