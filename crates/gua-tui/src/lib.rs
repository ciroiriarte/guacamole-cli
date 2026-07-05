//! In-terminal (TUI) renderer for Guacamole text protocols.
//!
//! Presents SSH/telnet/Kubernetes sessions as a true in-terminal experience by
//! consuming the `text-output` `pipe` stream. Implemented in roadmap issues
//! #24–#28 (and depends on the opt-in guacd `text-output` patch, #23).

#![forbid(unsafe_code)]
