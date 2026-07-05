//! Apache Guacamole instruction protocol.
//!
//! This crate is the **keystone** of the workspace: a pure, I/O-free, async-free
//! parser/encoder for the Guacamole wire format so it can be exhaustively fuzzed
//! and reused by both the live tunnel ([`gua-tunnel`]) and offline recording
//! playback.
//!
//! The wire format is `LENGTH.VALUE,LENGTH.VALUE,...;` where `LENGTH` is the
//! element length in **Unicode codepoints, not bytes**, elements are separated
//! by `,` and instructions terminated by `;`.
//!
//! The full parser/encoder and typed handshake instructions are implemented in
//! roadmap issue #10; this crate currently establishes the module boundary.
//!
//! [`gua-tunnel`]: https://github.com/ciroiriarte/guacamole-cli

#![forbid(unsafe_code)]
