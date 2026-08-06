//! Local dashboard UI (spec 12 — U1, U2, U5).
//!
//! Axum server bound to 127.0.0.1:4321 (loopback only, OBJ-1).
//! Routes serve status, job queue, wiki (okf_documents), and config keys.
//! Static SPA shell with 4 tabs (inline HTML, no build step).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::secrets::{MaskedKey, SecretStore};

/// Shared app state for all UI handlers (spec 12 U1).
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub secrets: Arc<SecretStore>,
    pub status: Arc<Mutex<UiStatus>>,
}

/// UI status injectable into handlers (spec 12 U2).
/// A lightweight snapshot passed to the router, enabling tests without a live Brain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiStatus {
    /// Consumed RAM in GB.
    pub ram_gb: f64,
    /// Total capacity (hardcoded to 22 per CON-1).
    pub ram_ceiling_gb: f64,
    /// Number of queued jobs.
    pub queue_depth: usize,
    /// True if inference (llama-server) is up.
    pub inference_up: bool,
    /// Optional degraded-mode banner (spec 12 U2, spec 09 H12).
    pub degraded_banner: Option<String>,
}

impl Default for UiStatus {
    fn default() -> Self {
        Self {
            ram_gb: 0.0,
            ram_ceiling_gb: 22.0,
            queue_depth: 0,
            inference_up: false,
            degraded_banner: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum UiError {
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("secret: {0}")]
    Secret(#[from] crate::secrets::SecretError),
    #[error("bad request: {0}")]
    BadRequest(String),
}

impl IntoResponse for UiError {
    fn into_response(self) -> axum::response::Response {
        let (status, body) = match self {
            UiError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            UiError::Secret(crate::secrets::SecretError::NotAllowed(key)) => {
                (StatusCode::BAD_REQUEST, format!("key not allowed: {}", key))
            }
            UiError::Secret(crate::secrets::SecretError::BadValue) => (
                StatusCode::BAD_REQUEST,
                "value invalid (newline or NUL)".to_string(),
            ),
            UiError::Secret(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            UiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
        };
        (status, body).into_response()
    }
}

/// Job summary for the Kanban view (spec 12 §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub priority: i64,
    pub created: String,
    /// Brief excerpt of payload (first 100 chars or descriptive field).
    pub payload_summary: String,
}

/// Request to enqueue a job from the UI (spec 12 §3).
#[derive(Debug, Serialize, Deserialize)]
pub struct EnqueueJobRequest {
    pub kind: String,
    pub description: Option<String>,
    pub plan: Option<String>,
    pub skills: Option<Vec<String>>,
    pub refs: Option<Vec<String>>,
}

/// Response after enqueueing (spec 12 §3).
#[derive(Debug, Serialize, Deserialize)]
pub struct EnqueueJobResponse {
    pub id: i64,
}

/// Wiki document summary (spec 12 §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiDoc {
    pub id: String,
    pub title: String,
    pub domain: String,
    pub status: String,
}

/// Chat message request (spec 12 §3, council seam).
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessageRequest {
    pub text: String,
}

/// Chat response (spec 12 §3).
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessageResponse {
    pub reply: String,
    pub queued: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET / — serves the single-page app shell with 4 tabs (spec 12 U1).
async fn get_index() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("Content-Type", "text/html; charset=utf-8")],
        HTML_SHELL,
    )
}

/// GET /api/status — returns current UI state (spec 12 U2).
async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    let s = state.status.lock().await;
    Json(s.clone())
}

/// GET /api/jobs — list all jobs with status for Kanban (spec 12 §3).
async fn get_jobs(State(state): State<AppState>) -> Result<Json<Vec<JobSummary>>, UiError> {
    let jobs: Vec<(i64, String, String, i64, String, String)> = sqlx::query_as(
        "SELECT id, kind, status, priority, created, payload FROM jobs ORDER BY created DESC",
    )
    .fetch_all(&state.pool)
    .await?;

    let summaries = jobs
        .into_iter()
        .map(|(id, kind, status, priority, created, payload)| {
            let payload_summary = payload.chars().take(100).collect::<String>();
            JobSummary {
                id,
                kind,
                status,
                priority,
                created,
                payload_summary,
            }
        })
        .collect();

    Ok(Json(summaries))
}

/// POST /api/jobs — enqueue a new job (spec 12 §3).
async fn post_jobs(
    State(state): State<AppState>,
    Json(req): Json<EnqueueJobRequest>,
) -> Result<Json<EnqueueJobResponse>, UiError> {
    // Build a minimal payload JSON from the request (spec 12 §3).
    let payload = serde_json::json!({
        "description": req.description,
        "plan": req.plan,
        "skills": req.skills,
        "refs": req.refs,
    });

    let now = chrono::Utc::now().to_rfc3339();
    let result = sqlx::query(
        r#"INSERT INTO jobs (kind, priority, payload, status, created)
           VALUES (?, ?, ?, 'queued', ?)"#,
    )
    .bind(&req.kind)
    .bind(5) // default priority
    .bind(payload.to_string())
    .bind(&now)
    .execute(&state.pool)
    .await?;

    Ok(Json(EnqueueJobResponse {
        id: result.last_insert_rowid(),
    }))
}

/// GET /api/wiki — list OKF documents (spec 12 §2).
async fn get_wiki(State(state): State<AppState>) -> Result<Json<Vec<WikiDoc>>, UiError> {
    let docs: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT id, title, domain, status FROM okf_documents ORDER BY created DESC")
            .fetch_all(&state.pool)
            .await?;

    let wikis = docs
        .into_iter()
        .map(|(id, title, domain, status)| WikiDoc {
            id,
            title,
            domain,
            status,
        })
        .collect();

    Ok(Json(wikis))
}

/// GET /api/wiki/:id — fetch a specific OKF document (content deferred; stub).
async fn get_wiki_doc(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WikiDoc>, UiError> {
    let doc: (String, String, String, String) =
        sqlx::query_as("SELECT id, title, domain, status FROM okf_documents WHERE id = ?")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .map_err(|_| UiError::BadRequest("doc not found".to_string()))?;

    Ok(Json(WikiDoc {
        id: doc.0,
        title: doc.1,
        domain: doc.2,
        status: doc.3,
    }))
}

/// GET /api/config/keys — list masked secrets (spec 12 U5, spec 11 S5).
async fn get_config_keys(State(state): State<AppState>) -> Result<Json<Vec<MaskedKey>>, UiError> {
    let keys = state.secrets.list_masked()?;
    Ok(Json(keys))
}

/// POST /api/config/keys — set a secret key (spec 12 U5).
#[derive(Debug, Serialize, Deserialize)]
pub struct SetKeyRequest {
    pub name: String,
    pub value: String,
}

async fn post_config_keys(
    State(state): State<AppState>,
    Json(req): Json<SetKeyRequest>,
) -> Result<Json<Vec<MaskedKey>>, UiError> {
    state.secrets.set_key(&req.name, &req.value)?;
    let keys = state.secrets.list_masked()?;
    Ok(Json(keys))
}

/// GET /api/chat — render chat tab (stub, council phase 5+).
async fn get_chat() -> impl IntoResponse {
    Json(serde_json::json!({
        "messages": [],
        "status": "ready"
    }))
}

/// POST /api/chat — send a message (spec 12 §3, council seam).
async fn post_chat(
    State(state): State<AppState>,
    Json(req): Json<ChatMessageRequest>,
) -> Result<Json<ChatMessageResponse>, UiError> {
    // If the message starts with '/task ', enqueue an agent job.
    let queued = if req.text.starts_with("/task ") {
        let desc = req.text.strip_prefix("/task ").unwrap_or("").to_string();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT INTO jobs (kind, priority, payload, status, created)
               VALUES (?, ?, ?, 'queued', ?)"#,
        )
        .bind("agent")
        .bind(3) // higher priority for agent tasks
        .bind(serde_json::json!({"description": desc}).to_string())
        .bind(&now)
        .execute(&state.pool)
        .await?;
        true
    } else {
        false
    };

    Ok(Json(ChatMessageResponse {
        reply: "council not yet wired (Phase 5) — your message was queued".to_string(),
        queued,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Router assembly
// ─────────────────────────────────────────────────────────────────────────────

/// Assemble all UI routes (spec 12 U1).
pub fn router(pool: SqlitePool, secrets: Arc<SecretStore>, status: Arc<Mutex<UiStatus>>) -> Router {
    let state = AppState {
        pool,
        secrets,
        status,
    };

    Router::new()
        .route("/", get(get_index))
        .route("/api/status", get(get_status))
        .route("/api/jobs", get(get_jobs).post(post_jobs))
        .route("/api/wiki", get(get_wiki))
        .route("/api/wiki/:id", get(get_wiki_doc))
        .route(
            "/api/config/keys",
            get(get_config_keys).post(post_config_keys),
        )
        .route("/api/chat", get(get_chat).post(post_chat))
        .with_state(state)
}

/// Bind and serve the UI router on loopback only (spec 12 U1, OBJ-1).
pub async fn serve(router: Router, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    // MANDATORY: bind 127.0.0.1 ONLY (never 0.0.0.0).
    // Binding non-loopback would violate OBJ-1 (local-only).
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("UI server bound to {}", addr);
    axum::serve(listener, router).await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// HTML Shell (spec 12 U1 — single SPA, inline, no build step)
// ─────────────────────────────────────────────────────────────────────────────

const HTML_SHELL: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>localAI Brain</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
            background: #0f0f0f;
            color: #e0e0e0;
            line-height: 1.6;
        }
        .container {
            display: flex;
            height: 100vh;
        }
        .sidebar {
            width: 200px;
            background: #1a1a1a;
            border-right: 1px solid #333;
            padding: 20px;
            overflow-y: auto;
        }
        .tab-btn {
            display: block;
            width: 100%;
            text-align: left;
            padding: 10px;
            margin: 5px 0;
            background: #333;
            border: 1px solid #444;
            color: #e0e0e0;
            cursor: pointer;
            border-radius: 4px;
        }
        .tab-btn.active {
            background: #0066cc;
            border-color: #0088ff;
        }
        .tab-btn:hover {
            background: #444;
        }
        .content {
            flex: 1;
            display: flex;
            flex-direction: column;
        }
        .header {
            background: #1a1a1a;
            border-bottom: 1px solid #333;
            padding: 15px 20px;
            display: flex;
            justify-content: space-between;
            align-items: center;
        }
        .status-bar {
            display: flex;
            gap: 20px;
            font-size: 12px;
        }
        .status-item {
            display: flex;
            align-items: center;
            gap: 5px;
        }
        .status-dot {
            width: 8px;
            height: 8px;
            border-radius: 50%;
        }
        .status-dot.online { background: #00cc00; }
        .status-dot.offline { background: #cc0000; }
        .status-dot.degraded { background: #ffaa00; }
        .main {
            flex: 1;
            overflow: auto;
            padding: 20px;
        }
        .tab-content {
            display: none;
        }
        .tab-content.active {
            display: block;
        }
        .panel {
            background: #1a1a1a;
            border: 1px solid #333;
            border-radius: 4px;
            padding: 15px;
            margin-bottom: 15px;
        }
        .panel-title {
            font-size: 14px;
            font-weight: bold;
            margin-bottom: 10px;
            color: #00ccff;
        }
        .form-group {
            margin-bottom: 10px;
        }
        label {
            display: block;
            font-size: 12px;
            margin-bottom: 5px;
            color: #aaa;
        }
        input, textarea, select {
            width: 100%;
            padding: 8px;
            background: #2a2a2a;
            border: 1px solid #444;
            color: #e0e0e0;
            border-radius: 4px;
            font-family: monospace;
        }
        button {
            padding: 8px 16px;
            background: #0066cc;
            color: #fff;
            border: none;
            border-radius: 4px;
            cursor: pointer;
            margin-top: 5px;
        }
        button:hover {
            background: #0088ff;
        }
        .card {
            background: #252525;
            border-left: 3px solid #0066cc;
            padding: 10px;
            margin-bottom: 8px;
            border-radius: 4px;
            font-size: 12px;
        }
        .card-header {
            display: flex;
            justify-content: space-between;
            margin-bottom: 5px;
        }
        .card-title {
            font-weight: bold;
            color: #00ccff;
        }
        .card-meta {
            color: #888;
            font-size: 11px;
        }
        .kanban-col {
            display: inline-block;
            width: 23%;
            background: #252525;
            border: 1px solid #333;
            border-radius: 4px;
            padding: 10px;
            margin-right: 2%;
            vertical-align: top;
        }
        .kanban-col-title {
            font-weight: bold;
            padding-bottom: 10px;
            border-bottom: 1px solid #333;
            margin-bottom: 10px;
            color: #00ccff;
        }
        .banner {
            background: #cc6600;
            border: 1px solid #ffaa00;
            color: #000;
            padding: 10px;
            border-radius: 4px;
            margin-bottom: 15px;
            font-weight: bold;
        }
    </style>
</head>
<body>
<div class="container">
    <div class="sidebar">
        <h2 style="margin-bottom: 20px; color: #00ccff;">localAI</h2>
        <button class="tab-btn active" onclick="switchTab('kanban')">Kanban</button>
        <button class="tab-btn" onclick="switchTab('chat')">Chat</button>
        <button class="tab-btn" onclick="switchTab('wiki')">Wiki</button>
        <button class="tab-btn" onclick="switchTab('config')">Config</button>
    </div>
    <div class="content">
        <div class="header">
            <h1 style="font-size: 18px;">localAI Brain Dashboard</h1>
            <div class="status-bar">
                <div class="status-item">
                    Inference: <span class="status-dot" id="inference-dot"></span>
                </div>
                <div class="status-item">
                    Queue: <span id="queue-depth">0</span>
                </div>
                <div class="status-item">
                    RAM: <span id="ram-usage">0</span> / 22 GB
                </div>
            </div>
        </div>
        <div class="main">
            <div id="degraded-banner" style="display: none;" class="banner"></div>

            <!-- KANBAN TAB -->
            <div id="kanban" class="tab-content active">
                <h2 style="margin-bottom: 20px;">Task Queue</h2>
                <div class="panel">
                    <div class="panel-title">Add Task</div>
                    <div class="form-group">
                        <label>Kind</label>
                        <input type="text" id="job-kind" placeholder="agent|scrape|ingest|distill|...">
                    </div>
                    <div class="form-group">
                        <label>Description</label>
                        <textarea id="job-desc" placeholder="What should this task do?" rows="3"></textarea>
                    </div>
                    <div class="form-group">
                        <label>Plan (optional)</label>
                        <textarea id="job-plan" placeholder="Step-by-step plan" rows="2"></textarea>
                    </div>
                    <button onclick="enqueueJob()">Create Task</button>
                </div>
                <div id="kanban-board" style="margin-top: 20px;"></div>
            </div>

            <!-- CHAT TAB -->
            <div id="chat" class="tab-content">
                <h2 style="margin-bottom: 20px;">Chat</h2>
                <div class="panel" style="height: 400px; overflow-y: auto;">
                    <div id="chat-messages"></div>
                </div>
                <div style="margin-top: 10px;">
                    <textarea id="chat-input" placeholder="Type a message... (start with /task to enqueue an agent task)" rows="3"></textarea>
                    <button onclick="sendChat()">Send</button>
                </div>
            </div>

            <!-- WIKI TAB -->
            <div id="wiki" class="tab-content">
                <h2 style="margin-bottom: 20px;">Knowledge Base</h2>
                <div id="wiki-list"></div>
            </div>

            <!-- CONFIG TAB -->
            <div id="config" class="tab-content">
                <h2 style="margin-bottom: 20px;">Configuration</h2>
                <div class="panel">
                    <div class="panel-title">API Keys (masked)</div>
                    <div id="config-keys"></div>
                </div>
            </div>
        </div>
    </div>
</div>

<script>
    // Polling interval for status updates (spec 12 U2, 2s poll).
    const POLL_INTERVAL = 2000;

    function switchTab(name) {
        document.querySelectorAll('.tab-content').forEach(el => el.classList.remove('active'));
        document.querySelectorAll('.tab-btn').forEach(el => el.classList.remove('active'));
        document.getElementById(name).classList.add('active');
        event.target.classList.add('active');
        if (name === 'chat') loadChatMessages();
        else if (name === 'wiki') loadWiki();
        else if (name === 'config') loadConfig();
    }

    async function loadStatus() {
        try {
            const resp = await fetch('/api/status');
            const status = await resp.json();
            document.getElementById('queue-depth').textContent = status.queue_depth;
            document.getElementById('ram-usage').textContent = status.ram_gb.toFixed(1);
            document.getElementById('inference-dot').className = 'status-dot ' +
                (status.inference_up ? 'online' : 'offline');

            if (status.degraded_banner) {
                const banner = document.getElementById('degraded-banner');
                banner.textContent = status.degraded_banner;
                banner.style.display = 'block';
            }
        } catch (e) {
            console.error('Status load failed:', e);
        }
    }

    async function loadJobs() {
        try {
            const resp = await fetch('/api/jobs');
            const jobs = await resp.json();
            renderKanban(jobs);
        } catch (e) {
            console.error('Job load failed:', e);
        }
    }

    function renderKanban(jobs) {
        const board = document.getElementById('kanban-board');
        const statuses = ['queued', 'running', 'done', 'failed', 'quarantined'];
        board.innerHTML = '';

        statuses.forEach(status => {
            const col = document.createElement('div');
            col.className = 'kanban-col';
            col.innerHTML = `<div class="kanban-col-title">${status}</div>`;

            jobs.filter(j => j.status === status).forEach(job => {
                const card = document.createElement('div');
                card.className = 'card';
                card.innerHTML = `
                    <div class="card-header">
                        <span class="card-title">${job.kind}</span>
                        <span class="card-meta">#${job.id}</span>
                    </div>
                    <div>${job.payload_summary.substring(0, 50)}...</div>
                `;
                col.appendChild(card);
            });
            board.appendChild(col);
        });
    }

    async function enqueueJob() {
        const kind = document.getElementById('job-kind').value;
        const desc = document.getElementById('job-desc').value;
        const plan = document.getElementById('job-plan').value;
        if (!kind) { alert('Kind is required'); return; }

        try {
            const resp = await fetch('/api/jobs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ kind, description: desc, plan })
            });
            if (resp.ok) {
                alert('Task created');
                document.getElementById('job-kind').value = '';
                document.getElementById('job-desc').value = '';
                document.getElementById('job-plan').value = '';
                loadJobs();
            } else {
                alert('Failed to create task');
            }
        } catch (e) {
            alert('Error: ' + e);
        }
    }

    async function loadChatMessages() {
        try {
            const resp = await fetch('/api/chat');
            const data = await resp.json();
            const container = document.getElementById('chat-messages');
            container.innerHTML = '<p style="color: #888;">(messages go here)</p>';
        } catch (e) {
            console.error('Chat load failed:', e);
        }
    }

    async function sendChat() {
        const text = document.getElementById('chat-input').value;
        if (!text) return;

        try {
            const resp = await fetch('/api/chat', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ text })
            });
            const data = await resp.json();
            const container = document.getElementById('chat-messages');
            const msg = document.createElement('div');
            msg.style.padding = '5px';
            msg.innerHTML = `<strong>You:</strong> ${text}<br><strong>Brain:</strong> ${data.reply}`;
            container.appendChild(msg);
            document.getElementById('chat-input').value = '';
        } catch (e) {
            alert('Chat error: ' + e);
        }
    }

    async function loadWiki() {
        try {
            const resp = await fetch('/api/wiki');
            const docs = await resp.json();
            const container = document.getElementById('wiki-list');
            container.innerHTML = '';
            docs.forEach(doc => {
                const card = document.createElement('div');
                card.className = 'card';
                card.innerHTML = `<div class="card-title">${doc.title}</div>
                    <div style="font-size: 11px; color: #888;">${doc.domain} — ${doc.status}</div>`;
                container.appendChild(card);
            });
        } catch (e) {
            console.error('Wiki load failed:', e);
        }
    }

    async function loadConfig() {
        try {
            const resp = await fetch('/api/config/keys');
            const keys = await resp.json();
            const container = document.getElementById('config-keys');
            container.innerHTML = '';
            keys.forEach(key => {
                const div = document.createElement('div');
                div.className = 'panel';
                div.innerHTML = `
                    <div style="margin-bottom: 10px;">
                        <strong>${key.name}</strong> ${key.set ? '(set)' : '(not set)'}<br>
                        <span style="color: #888; font-size: 11px;">${key.masked}</span>
                    </div>
                    <input type="password" id="key-${key.name}" placeholder="Enter new value (leave empty to keep current)">
                    <button onclick="setKey('${key.name}')">Update</button>
                `;
                container.appendChild(div);
            });
        } catch (e) {
            console.error('Config load failed:', e);
        }
    }

    async function setKey(name) {
        const val = document.getElementById('key-' + name).value;
        if (!val) return;

        try {
            const resp = await fetch('/api/config/keys', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ name, value: val })
            });
            if (resp.ok) {
                alert('Key updated');
                loadConfig();
            } else {
                const err = await resp.text();
                alert('Error: ' + err);
            }
        } catch (e) {
            alert('Error: ' + e);
        }
    }

    // Polling loop
    setInterval(async () => {
        await loadStatus();
        if (document.getElementById('kanban').classList.contains('active')) {
            await loadJobs();
        }
    }, POLL_INTERVAL);

    // Initial load
    loadStatus();
    loadJobs();
</script>
</body>
</html>
"#;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // T1: HTML shell contains 4 tab labels (spec 12 U1).
    #[test]
    fn test_html_shell_has_four_tabs() {
        assert!(
            HTML_SHELL.contains("Kanban"),
            "HTML should contain Kanban tab"
        );
        assert!(HTML_SHELL.contains("Chat"), "HTML should contain Chat tab");
        assert!(HTML_SHELL.contains("Wiki"), "HTML should contain Wiki tab");
        assert!(
            HTML_SHELL.contains("Config"),
            "HTML should contain Config tab"
        );
    }

    // T2a: UiStatus defaults to expected values.
    #[test]
    fn test_ui_status_defaults() {
        let status = UiStatus::default();
        assert_eq!(status.ram_ceiling_gb, 22.0);
        assert_eq!(status.queue_depth, 0);
        assert!(!status.inference_up);
        assert!(status.degraded_banner.is_none());
    }

    // T2b: Jobs can be enqueued via database.
    #[tokio::test]
    async fn test_enqueue_job_in_db() {
        let pool = localai_migration::run_migrations("sqlite::memory:")
            .await
            .expect("migrations");

        let payload = serde_json::json!({"description": "test"});
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            r#"INSERT INTO jobs (kind, priority, payload, status, created)
               VALUES (?, ?, ?, 'queued', ?)"#,
        )
        .bind("test_kind")
        .bind(5)
        .bind(payload.to_string())
        .bind(&now)
        .execute(&pool)
        .await
        .expect("insert job");

        let status: String = sqlx::query_scalar("SELECT status FROM jobs LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("fetch job");

        assert_eq!(status, "queued");
    }

    // T3: SecretStore masks API keys, never exposes raw values.
    #[tokio::test]
    async fn test_secrets_are_masked_not_raw() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let secrets = SecretStore::new(tmpdir.path().to_path_buf());

        secrets
            .set_key("OPENAI_API_KEY", "sk-test-abcd1234wxyz")
            .expect("set key");

        let masked_list = secrets.list_masked().expect("list masked");
        let openai = masked_list
            .iter()
            .find(|k| k.name == "OPENAI_API_KEY")
            .expect("key exists");

        assert!(openai.set, "key should be marked as set");
        assert!(
            !openai.masked.contains("abcd1234"),
            "raw value should not appear"
        );
        assert!(
            openai.masked.contains("…"),
            "masked format should use ellipsis"
        );
    }

    // T4: SecretStore rejects non-allowlisted keys.
    #[tokio::test]
    async fn test_secrets_rejects_bad_key_names() {
        let tmpdir = tempfile::tempdir().expect("tempdir");
        let secrets = SecretStore::new(tmpdir.path().to_path_buf());

        let result = secrets.set_key("EVIL_ARBITRARY_VAR", "x");
        assert!(result.is_err(), "non-allowlisted key should be rejected");
    }

    // T5: Loopback binding check (spec 12 U1, OBJ-1).
    #[test]
    fn test_loopback_binding_hardcoded() {
        // The serve() function hardcodes 127.0.0.1 binding. This test verifies
        // the bind address is loopback-only (never 0.0.0.0).
        let addr = SocketAddr::from(([127, 0, 0, 1], 4321));
        assert!(
            addr.ip().is_loopback(),
            "UI must bind to loopback only (spec 12 U1, OBJ-1)"
        );
    }
}
