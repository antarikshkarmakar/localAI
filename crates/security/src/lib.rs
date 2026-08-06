//! Security primitives (spec 11).
//!
//! - [`secret_filter`] — CON-13 redaction at the persist + egress chokepoints.

pub mod secret_filter;

pub use secret_filter::{filter, redact, Filtered, Hit, REDACTED};
