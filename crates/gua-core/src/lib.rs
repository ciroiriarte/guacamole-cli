//! Shared foundations for `guacamole-cli`.
//!
//! Dependency-light leaf crate used by every other crate in the workspace:
//!
//! - [`error`] — the crate-wide [`Error`]/[`Result`] types and process exit-code
//!   mapping (issue #4).
//! - [`logging`] — `tracing` setup + secret redaction (issue #5).
//! - [`config`] — profiles/"contexts" loaded from disk with env overrides
//!   (issue #6).
//! - [`credentials`] — pluggable token storage with expiry (issue #7).
//! - [`output`] — table/json/yaml rendering (issue #8).

#![forbid(unsafe_code)]

pub mod config;
pub mod credentials;
pub mod error;
pub mod logging;
pub mod output;

pub use error::{Error, Result};
