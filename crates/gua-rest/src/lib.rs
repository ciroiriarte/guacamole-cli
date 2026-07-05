//! Apache Guacamole REST management-API client.
//!
//! Wraps the gateway's management plane under `/api/session/data/{dataSource}/`
//! (connections, groups, users, permissions, sharing profiles, active sessions,
//! history) plus `/api/tokens` authentication. The live client is implemented in
//! roadmap issues #11 onward; this crate currently ships the reusable
//! [`testing`] mock gateway used across the workspace.

#![forbid(unsafe_code)]

#[cfg(feature = "test-support")]
pub mod testing;
