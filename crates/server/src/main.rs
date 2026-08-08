//! `localai-brain` — the Brain binary (composition root, spec 01).
//!
//! The one place wall-clock reads are allowed (G-09 forbids duration math
//! across sleep/resume, not a boot timestamp). Wires: config → boot() →
//! supervisor idle loop → SIGTERM/Ctrl-C → graceful shutdown.
//!
//! Seams still open (documented in startup.rs): step 5 llama-server spawn
//! (inference::launch is built, not yet wired here — needs a model path),
//! and the real per-kind JobRunner (Phase 3). Until a runner lands, jobs of
//! any kind fail as `bug`-class; a fresh DB has none, so the Brain idles.

use chrono::Utc;
use localai_core::config::Config;
use localai_server::process_runner::ProcessRunner;
use localai_server::queue::JobQueue;
use localai_server::scheduler::{default_jobs, Scheduler};
use localai_server::secrets::SecretStore;
use localai_server::startup::boot;
use localai_server::supervisor::{JobRunner, Supervisor};
use localai_server::ui::{self, UiStatus};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Locate the `localai-worker` binary: LOCALAI_WORKER_BIN env wins, else a
/// sibling of the running `localai-brain` executable (cargo puts workspace
/// bins in the same target dir).
fn worker_bin_path() -> PathBuf {
    if let Ok(p) = std::env::var("LOCALAI_WORKER_BIN") {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("localai-worker")))
        .unwrap_or_else(|| PathBuf::from("localai-worker"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load operator API keys from the local secret store into the process env
    // BEFORE config load, so an explicit shell env var still wins (CON-9).
    let secrets = std::sync::Arc::new(SecretStore::new(secrets_dir()));
    match secrets.load_into_env() {
        Ok(n) => tracing::info!(keys = n, "loaded API keys from secret store"),
        Err(e) => tracing::warn!(error = %e, "secret store load skipped"),
    }

    // Config: config.toml (if present) < LOCALAI_* env (spec 01 §6).
    let toml_str = std::fs::read_to_string("config.toml").unwrap_or_default();
    let config = Config::load(&toml_str, std::env::vars())?;

    let cwd = std::env::current_dir()?;
    let heartbeat_path = data_dir(&config).join("localai.heartbeat");
    let heartbeat_every = Duration::from_secs(1); // well under watchdog threshold (R16)

    // Boot (steps 1–4, 6-partial, 7). `now` from wall clock — allowed here.
    let (brain, report) = boot(
        &config,
        &cwd,
        &Utc::now().to_rfc3339(),
        heartbeat_path,
        heartbeat_every,
    )
    .await?;

    let inference_state = if brain.inference.is_some() {
        "up"
    } else {
        "disabled (no model_path — degraded/model-down, H12)"
    };
    tracing::info!(
        db = %brain.db_path.display(),
        spill_reconciled = report.spill_reconciled,
        orphans_requeued = report.orphans.requeued,
        orphans_quarantined = report.orphans.quarantined,
        inference = inference_state,
        "Brain booted"
    );

    // Supervisor dispatching to real worker child processes (spec 04 O8).
    // Job wall-clock timeout = the lease: a job must finish inside its lease
    // or it's presumed dead anyway (O3/O10 aligned).
    let worker_bin = worker_bin_path();
    if !worker_bin.exists() {
        tracing::warn!(bin = %worker_bin.display(),
            "localai-worker binary not found — jobs will fail until it is built \
             (cargo build --bin localai-worker) or LOCALAI_WORKER_BIN is set");
    }
    let queue = Arc::new(JobQueue::new(brain.pool.clone()));
    let runner: Arc<dyn JobRunner> = Arc::new(ProcessRunner::new(
        worker_bin,
        Duration::from_secs(config.queue.lease_secs),
    ));
    let supervisor = Supervisor::new(brain.pool.clone(), queue, runner, &config.queue);
    let lease_secs = config.queue.lease_secs as i64;

    // Local dashboard (spec 12) — loopback only. Seed status once; live
    // MemoryGuard→UiStatus refresh is a follow-on (status starts static).
    let ui_status = std::sync::Arc::new(tokio::sync::Mutex::new(UiStatus {
        ram_gb: 0.0,
        ram_ceiling_gb: config.mem.ceiling_gb,
        queue_depth: 0,
        inference_up: brain.inference.is_some(),
        degraded_banner: brain
            .inference
            .is_none()
            .then(|| "model-down (no model_path) — H12".to_string()),
    }));
    let ui_router = ui::router(brain.pool.clone(), secrets.clone(), ui_status);
    let ui_port: u16 = 4321; // spec 12 U1 default
    let ui_task = tokio::spawn(async move {
        if let Err(e) = ui::serve(ui_router, ui_port).await {
            tracing::error!(error = %e, "dashboard server exited");
        }
    });
    tracing::info!(url = %format!("http://127.0.0.1:{ui_port}"), "dashboard up");

    // Recurring work (spec 04 O15/O15b) — this is what makes the Brain
    // self-directed rather than waiting for the operator to enqueue.
    // Ticking hourly is plenty: the dedup key is period-bucketed, so extra
    // ticks are no-ops and a missed hour still fires later the same day.
    let scheduler = Scheduler::new(
        brain.pool.clone(),
        default_jobs(&config.research.arxiv_categories, &config.research.sources),
    );
    let sched_task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            match scheduler.tick(&Utc::now().to_rfc3339()).await {
                Ok(r) if !r.enqueued.is_empty() => {
                    tracing::info!(jobs = ?r.enqueued, "scheduler enqueued recurring work")
                }
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "scheduler tick failed"),
            }
        }
    });

    run_until_signal(&supervisor, lease_secs).await;
    ui_task.abort();
    sched_task.abort();

    tracing::info!("shutdown signal received — flushing and exiting");
    brain.shutdown().await;
    Ok(())
}

/// Dispatch loop: one `run_once` per tick until a shutdown signal. Concurrent
/// dispatch up to the O4 semaphore is a Phase-3 refinement; a serial tick is
/// correct and simplest for the first bootable Brain.
async fn run_until_signal(supervisor: &Supervisor, lease_secs: i64) {
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let mut shutdown = ShutdownSignal::new();

    loop {
        tokio::select! {
            _ = shutdown.recv() => return,
            _ = tick.tick() => {
                let now = Utc::now().to_rfc3339();
                let lease = (Utc::now() + chrono::Duration::seconds(lease_secs)).to_rfc3339();
                match supervisor.run_once(&now, &lease).await {
                    Ok(Some(id)) => tracing::debug!(job = id, "job dispatched"),
                    Ok(None) => {}
                    Err(e) => tracing::error!(error = %e, "dispatch error"),
                }
            }
        }
    }
}

/// SIGTERM (watchdog/systemd) + Ctrl-C (interactive), unified. If SIGTERM
/// registration fails, Ctrl-C still shuts down cleanly (no panic).
struct ShutdownSignal {
    #[cfg(unix)]
    sigterm: Option<tokio::signal::unix::Signal>,
}

impl ShutdownSignal {
    fn new() -> Self {
        #[cfg(unix)]
        {
            let sigterm =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        tracing::warn!(error = %e, "no SIGTERM handler; Ctrl-C only");
                        None
                    }
                };
            Self { sigterm }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        if let Some(sigterm) = self.sigterm.as_mut() {
            tokio::select! {
                _ = sigterm.recv() => return,
                _ = tokio::signal::ctrl_c() => return,
            }
        }
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Directory holding runtime data — parent of the configured db file.
fn data_dir(config: &Config) -> PathBuf {
    PathBuf::from(&config.paths.db_path)
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Secret store location: `$HOME/.localai` (outside the repo, gitignored by
/// location — CON-13 softened for the single-user config UI). Falls back to
/// `./.localai` if HOME is unset.
fn secrets_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".localai")
}
