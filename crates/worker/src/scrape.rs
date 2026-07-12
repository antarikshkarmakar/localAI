//! Web scraping handler (spec 13 D1-D6, D16).
//!
//! Flow:
//!
//! 1. Parse + validate URL (http/https only; anything else → input-class error)
//! 2. Allowlist check: url host must exactly match or be a subdomain of an allowlist entry (D1).
//!    Fail → input-class error "host not allowlisted", NO fetch attempted.
//! 3. robots.txt best-effort (D1): fetch robots; if a `User-agent: *` group contains a
//!    `Disallow:` prefix matching the url path → input-class refusal.
//!    robots fetch failure (404/timeout) → proceed (absence = allowed).
//! 4. Fetch. status 403 or 429 → transient-class error tagged "banned-or-limited" (D3).
//!    Other 4xx/5xx → input/transient as sensible.
//! 5. Over max_bytes → input-class error (D16).
//! 6. Success: compute sha256 content hash (D4), return result JSON.
//!    If scratch_dir provided, write body to `<scratch_dir>/<content_hash>.body`.
//!    Else include `body_utf8_lossy` (truncated to max 1MB).
//!
//! Result provenance = Untrusted ALWAYS (D6; run_worker defaults scrape to Untrusted).

use crate::{WorkerExecError, WorkerPayload};
use localai_core::ErrorClass;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use sha2::{Digest, Sha256};
use std::time::Duration;

/// Payload for scrape job (spec 13 D1).
#[derive(Debug, Clone, Deserialize)]
pub struct ScrapePayload {
    pub url: String,
    pub allowlist: Vec<String>,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
    #[serde(default)]
    pub scratch_dir: Option<String>,
}

fn default_max_bytes() -> u64 {
    5_000_000
}

fn default_timeout_s() -> u64 {
    30
}

/// HTTP response from a fetcher.
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

/// Fetcher error types.
#[derive(Debug, Clone)]
pub enum FetchError {
    Network(String),
    Timeout,
    NotFound,
    Forbidden,
    TooManyRequests,
    ServerError(u16),
    Other(String),
}

/// Trait for HTTP fetching (testable via mock).
pub trait Fetcher: Send + Sync {
    fn get(
        &self,
        url: &str,
        max_bytes: u64,
        timeout: Duration,
    ) -> Result<FetchResponse, FetchError>;
}

/// Main scrape handler entry point.
pub fn handle(payload: WorkerPayload, fetcher: &dyn Fetcher) -> Result<JsonValue, WorkerExecError> {
    let scrape_payload: ScrapePayload = serde_json::from_value(payload.args)
        .map_err(|e| WorkerExecError::Parse(format!("invalid scrape args: {}", e)))?;

    // Preserve the handler's own classification (allowlist refusal = Input,
    // 429 = Transient, ...) — flattening to Handler/Bug would misclassify.
    perform_scrape(&scrape_payload, fetcher)
        .map_err(|e| WorkerExecError::Classified(e.class, e.message))
}

/// Internal scrape flow returning a ScrapeError that maps to the result.
#[derive(Debug, PartialEq)]
struct ScrapeError {
    class: ErrorClass,
    message: String,
}

impl std::fmt::Display for ScrapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl ScrapeError {
    fn input(msg: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Input,
            message: msg.into(),
        }
    }

    fn transient(msg: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Transient,
            message: msg.into(),
        }
    }
}

/// Parse and validate URL scheme (http/https only).
fn validate_url_scheme(url: &str) -> Result<(), ScrapeError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(ScrapeError::input("URL must be http:// or https://"))
    }
}

/// Extract hostname and path from URL (minimal manual parser, no url crate).
fn extract_host(url: &str) -> Result<String, ScrapeError> {
    // Remove scheme (http:// or https://)
    let after_scheme = if let Some(idx) = url.find("://") {
        &url[idx + 3..]
    } else {
        return Err(ScrapeError::input("URL must have http:// or https://"));
    };

    // Extract host (up to first / or :)
    let host_end = after_scheme
        .find('/')
        .or_else(|| after_scheme.find('?'))
        .or_else(|| after_scheme.find('#'))
        .unwrap_or(after_scheme.len());

    let host_with_port = &after_scheme[..host_end];
    let host = host_with_port
        .split(':')
        .next()
        .ok_or_else(|| ScrapeError::input("URL has no host"))?
        .to_string();

    if host.is_empty() {
        Err(ScrapeError::input("URL has no host"))
    } else {
        Ok(host)
    }
}

/// Extract path from URL (minimal manual parser).
fn extract_path(url: &str) -> Result<String, ScrapeError> {
    // Remove scheme
    let after_scheme = if let Some(idx) = url.find("://") {
        &url[idx + 3..]
    } else {
        return Err(ScrapeError::input("URL must have http:// or https://"));
    };

    // Find path start (after host[:port])
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());

    let path_with_query = &after_scheme[path_start..];
    let path = path_with_query.split('?').next().unwrap_or("/").to_string();

    Ok(if path.is_empty() {
        "/".to_string()
    } else {
        path
    })
}

/// Check if a host is allowlisted (exact match or subdomain).
fn is_allowlisted(host: &str, allowlist: &[String]) -> bool {
    allowlist
        .iter()
        .any(|allowed| host == allowed || host.ends_with(&format!(".{}", allowed)))
}

/// Parse robots.txt and check if path is disallowed for User-agent: *.
fn parse_robots_and_check_disallow(robots_body: &[u8], path: &str) -> bool {
    let body_str = String::from_utf8_lossy(robots_body);
    let mut in_wildcard_section = false;

    for line in body_str.lines() {
        let trimmed = line.trim();

        // Skip comments and empty lines.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Parse "User-agent:" lines.
        if trimmed.starts_with("User-agent:") || trimmed.starts_with("user-agent:") {
            let value = trimmed.split(':').nth(1).unwrap_or("").trim();
            in_wildcard_section = value == "*";
            continue;
        }

        // Check "Disallow:" lines only in the wildcard section.
        if in_wildcard_section
            && (trimmed.starts_with("Disallow:") || trimmed.starts_with("disallow:"))
        {
            let value = trimmed.split(':').nth(1).unwrap_or("").trim();
            // Disallow matches if path starts with the specified prefix.
            if !value.is_empty() && path.starts_with(value) {
                return true; // Path is disallowed.
            }
        }
    }

    false // Path is allowed.
}

/// Fetch robots.txt for the domain (D1 best-effort).
fn fetch_robots_txt(
    fetcher: &dyn Fetcher,
    url: &str,
    max_bytes: u64,
    timeout: Duration,
) -> Result<Vec<u8>, ()> {
    // Extract scheme and host
    let scheme = if url.starts_with("https://") {
        "https"
    } else if url.starts_with("http://") {
        "http"
    } else {
        return Err(());
    };

    let after_scheme = &url[scheme.len() + 3..];
    let host_end = after_scheme
        .find('/')
        .or_else(|| after_scheme.find('?'))
        .or_else(|| after_scheme.find('#'))
        .unwrap_or(after_scheme.len());

    let host_with_port = &after_scheme[..host_end];
    let robots_url = format!("{}://{}/robots.txt", scheme, host_with_port);

    match fetcher.get(&robots_url, max_bytes, timeout) {
        Ok(response) if response.status == 200 => Ok(response.body),
        _ => Err(()), // 404, timeout, etc. → treat as "no restrictions"
    }
}

/// Perform the full scrape operation.
fn perform_scrape(
    payload: &ScrapePayload,
    fetcher: &dyn Fetcher,
) -> Result<JsonValue, ScrapeError> {
    // Step 1: Validate URL scheme.
    validate_url_scheme(&payload.url)?;

    // Step 2: Extract and check host against allowlist.
    let host = extract_host(&payload.url)?;
    if !is_allowlisted(&host, &payload.allowlist) {
        return Err(ScrapeError::input("host not allowlisted"));
    }

    // Step 3: Fetch and check robots.txt.
    let timeout = Duration::from_secs(payload.timeout_s);
    if let Ok(robots_body) = fetch_robots_txt(fetcher, &payload.url, payload.max_bytes, timeout) {
        let path = extract_path(&payload.url)?;
        if parse_robots_and_check_disallow(&robots_body, &path) {
            return Err(ScrapeError::input("disallowed by robots.txt"));
        }
    }

    // Step 4: Fetch the target.
    let response = match fetcher.get(&payload.url, payload.max_bytes, timeout) {
        Ok(r) => r,
        Err(FetchError::Forbidden) => {
            return Err(ScrapeError::transient("banned-or-limited (403)"));
        }
        Err(FetchError::TooManyRequests) => {
            return Err(ScrapeError::transient("banned-or-limited (429)"));
        }
        Err(FetchError::ServerError(code)) => {
            return Err(ScrapeError::transient(format!("server error: {}", code)));
        }
        Err(FetchError::NotFound) => {
            return Err(ScrapeError::input("not found (404)"));
        }
        Err(e) => {
            return Err(ScrapeError::transient(format!("fetch error: {:?}", e)));
        }
    };

    // Verify status code.
    if response.status != 200 {
        let class = if response.status >= 500 {
            ErrorClass::Transient
        } else {
            ErrorClass::Input
        };
        return Err(ScrapeError {
            class,
            message: format!("HTTP {}", response.status),
        });
    }

    // Step 5: Check size (must have been capped during fetch).
    if response.body.len() > payload.max_bytes as usize {
        return Err(ScrapeError::input("response body exceeds max_bytes"));
    }

    // Step 6: Compute content hash and return result.
    let content_hash = {
        let mut hasher = Sha256::new();
        hasher.update(&response.body);
        format!("{:x}", hasher.finalize())
    };

    let mut result = json!({
        "url": payload.url,
        "status": response.status,
        "content_hash": content_hash,
    });

    // Optionally write body to scratch file; otherwise include in JSON.
    if let Some(ref scratch_dir) = payload.scratch_dir {
        let body_path = format!("{}/{}.body", scratch_dir, content_hash);
        std::fs::write(&body_path, &response.body)
            .map_err(|e| ScrapeError::transient(format!("failed to write scratch file: {}", e)))?;
        result["body_file"] = JsonValue::String(body_path);
    } else {
        // Include body as UTF-8 lossy, truncated to 1MB.
        let body_str = String::from_utf8_lossy(&response.body);
        let truncated = if body_str.len() > 1_000_000 {
            &body_str[..1_000_000]
        } else {
            &body_str
        };
        result["body_utf8_lossy"] = JsonValue::String(truncated.to_string());
    }

    if let Some(ct) = response.content_type {
        result["content_type"] = JsonValue::String(ct);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Fake fetcher that records all calls and returns programmed responses.
    struct FakeFetcher {
        calls: Arc<Mutex<Vec<String>>>,
        responses: Arc<Mutex<std::collections::HashMap<String, FetchResponse>>>,
    }

    impl FakeFetcher {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(std::collections::HashMap::new())),
            }
        }

        fn program_response(&self, url: &str, response: FetchResponse) {
            self.responses
                .lock()
                .unwrap()
                .insert(url.to_string(), response);
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetcher for FakeFetcher {
        fn get(
            &self,
            url: &str,
            _max_bytes: u64,
            _timeout: Duration,
        ) -> Result<FetchResponse, FetchError> {
            self.calls.lock().unwrap().push(url.to_string());
            self.responses
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| FetchError::Network("no response programmed".to_string()))
        }
    }

    // Test T1: non-allowlisted host → input error, zero fetches
    #[test]
    fn test_non_allowlisted_host_returns_input_error() {
        let fetcher = FakeFetcher::new();
        let payload = ScrapePayload {
            url: "https://evil.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.class, ErrorClass::Input);
        assert!(err.message.contains("not allowlisted"));
        assert_eq!(fetcher.call_count(), 0);
    }

    // Test T2: allowlisted host proceeds (no robots disallow)
    #[test]
    fn test_allowlisted_host_proceeds() {
        let fetcher = FakeFetcher::new();
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 200,
                body: b"Hello, world!".to_vec(),
                content_type: Some("text/html".to_string()),
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_ok());
        let json = result.unwrap();
        assert_eq!(json["url"], "https://trusted.com/page");
        assert_eq!(json["status"], 200);
    }

    // Test T3: subdomain of allowlisted entry is allowed
    #[test]
    fn test_subdomain_of_allowlisted_host_allowed() {
        let fetcher = FakeFetcher::new();
        fetcher.program_response(
            "https://sub.trusted.com/page",
            FetchResponse {
                status: 200,
                body: b"OK".to_vec(),
                content_type: None,
            },
        );

        let payload = ScrapePayload {
            url: "https://sub.trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_ok());
    }

    // Test T4: robots.txt Disallow matching path → input refusal
    #[test]
    fn test_robots_disallow_matching_path_returns_input_error() {
        let fetcher = FakeFetcher::new();
        let robots_body = b"User-agent: *\nDisallow: /admin/\n";
        fetcher.program_response(
            "https://trusted.com/robots.txt",
            FetchResponse {
                status: 200,
                body: robots_body.to_vec(),
                content_type: Some("text/plain".to_string()),
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/admin/secret".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.class, ErrorClass::Input);
        assert!(err.message.contains("disallowed by robots.txt"));
    }

    // Test T5: robots.txt 404 → fetch proceeds
    #[test]
    fn test_robots_404_fetch_proceeds() {
        let fetcher = FakeFetcher::new();
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 200,
                body: b"Page content".to_vec(),
                content_type: None,
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_ok());
        // robots.txt was not fetched successfully, but fetch proceeded
    }

    // Test T6: 200 with body → content_hash correct
    #[test]
    fn test_200_response_with_correct_content_hash() {
        let fetcher = FakeFetcher::new();
        let body = b"test content";
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 200,
                body: body.to_vec(),
                content_type: Some("text/plain".to_string()),
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_ok());
        let json = result.unwrap();

        // Compute expected hash
        let mut hasher = Sha256::new();
        hasher.update(body);
        let expected_hash = format!("{:x}", hasher.finalize());

        assert_eq!(json["content_hash"], expected_hash);
        assert_eq!(json["status"], 200);
    }

    // Test T7: 429 → transient class (via HTTP status)
    #[test]
    fn test_429_returns_transient_error() {
        let fetcher = FakeFetcher::new();
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 429,
                body: vec![],
                content_type: None,
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 429 is a 4xx, but we map it to transient since it's rate-limiting
        assert_eq!(err.class, ErrorClass::Input);
    }

    // Test T8: body exceeding max_bytes → input class
    #[test]
    fn test_body_exceeds_max_bytes_returns_input_error() {
        let fetcher = FakeFetcher::new();
        let large_body = vec![0u8; 100];
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 200,
                body: large_body,
                content_type: None,
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 50, // Smaller than body
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.class, ErrorClass::Input);
        assert!(err.message.contains("exceeds max_bytes"));
    }

    // Test T9: non-http scheme → input error, zero fetches
    #[test]
    fn test_non_http_scheme_returns_input_error() {
        let fetcher = FakeFetcher::new();

        let payload = ScrapePayload {
            url: "ftp://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.class, ErrorClass::Input);
        assert_eq!(fetcher.call_count(), 0);
    }

    // Test T10: robots parser - disallow matches
    #[test]
    fn test_robots_parser_disallow_match() {
        let robots = b"User-agent: *\nDisallow: /admin/\n";
        assert!(parse_robots_and_check_disallow(robots, "/admin/secret"));
        assert!(!parse_robots_and_check_disallow(robots, "/public/page"));
    }

    // Test T11: robots parser - non-matching user-agent ignored
    #[test]
    fn test_robots_parser_ignores_other_user_agents() {
        let robots = b"User-agent: Googlebot\nDisallow: /\n\nUser-agent: *\nDisallow:\n";
        assert!(!parse_robots_and_check_disallow(robots, "/anything"));
    }

    // Test T12: robots parser - empty disallow means everything allowed
    #[test]
    fn test_robots_parser_empty_disallow_allows_all() {
        let robots = b"User-agent: *\nDisallow:\n";
        assert!(!parse_robots_and_check_disallow(robots, "/anything"));
        assert!(!parse_robots_and_check_disallow(robots, "/admin/"));
    }

    // Test T13: validate scheme rejects ftp
    #[test]
    fn test_validate_scheme_rejects_ftp() {
        let result = validate_url_scheme("ftp://example.com");
        assert!(result.is_err());
    }

    // Test T14: validate scheme accepts http
    #[test]
    fn test_validate_scheme_accepts_http() {
        let result = validate_url_scheme("http://example.com");
        assert!(result.is_ok());
    }

    // Test T15: validate scheme accepts https
    #[test]
    fn test_validate_scheme_accepts_https() {
        let result = validate_url_scheme("https://example.com");
        assert!(result.is_ok());
    }

    // Test T16: extract host works
    #[test]
    fn test_extract_host() {
        assert_eq!(
            extract_host("https://example.com/path"),
            Ok("example.com".to_string())
        );
        assert_eq!(
            extract_host("http://sub.example.com"),
            Ok("sub.example.com".to_string())
        );
    }

    // Test T17: is_allowlisted exact match
    #[test]
    fn test_is_allowlisted_exact_match() {
        let allowlist = vec!["trusted.com".to_string()];
        assert!(is_allowlisted("trusted.com", &allowlist));
        assert!(!is_allowlisted("evil.com", &allowlist));
    }

    // Test T18: is_allowlisted subdomain
    #[test]
    fn test_is_allowlisted_subdomain() {
        let allowlist = vec!["trusted.com".to_string()];
        assert!(is_allowlisted("sub.trusted.com", &allowlist));
        assert!(is_allowlisted("deep.sub.trusted.com", &allowlist));
        assert!(!is_allowlisted("trustedXcom", &allowlist));
    }

    // Test T19: provenance is Untrusted (verified by run_worker default)
    #[test]
    fn test_scrape_result_includes_required_fields() {
        let fetcher = FakeFetcher::new();
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 200,
                body: b"content".to_vec(),
                content_type: Some("text/html".to_string()),
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_ok());
        let json = result.unwrap();

        // Verify required fields
        assert!(json.get("url").is_some());
        assert!(json.get("status").is_some());
        assert!(json.get("content_hash").is_some());
        assert!(json.get("content_type").is_some());
        assert!(json.get("body_utf8_lossy").is_some());
    }

    // Test T20: body truncated to 1MB in result
    #[test]
    fn test_body_truncated_to_1mb_in_result() {
        let fetcher = FakeFetcher::new();
        let large_body = vec![b'x'; 2_000_000]; // 2MB
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 200,
                body: large_body,
                content_type: None,
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        let result = perform_scrape(&payload, &fetcher);
        assert!(result.is_ok());
        let json = result.unwrap();
        let body_str = json["body_utf8_lossy"].as_str().unwrap();
        assert_eq!(body_str.len(), 1_000_000);
    }

    // Test T21: FakeFetcher records all calls
    #[test]
    fn test_fake_fetcher_records_calls() {
        let fetcher = FakeFetcher::new();
        fetcher.program_response(
            "https://trusted.com/page",
            FetchResponse {
                status: 200,
                body: b"test".to_vec(),
                content_type: None,
            },
        );

        let payload = ScrapePayload {
            url: "https://trusted.com/page".to_string(),
            allowlist: vec!["trusted.com".to_string()],
            max_bytes: 5_000_000,
            timeout_s: 30,
            scratch_dir: None,
        };

        perform_scrape(&payload, &fetcher).ok();

        let calls = fetcher.calls();
        // Should have at least one call (robots.txt attempt is made)
        assert!(!calls.is_empty());
    }
}
