//! FUSE mount exposing Guacamole drive-redirection file streams.
//!
//! Maps the session `filesystem`/`file` stream onto a local FUSE mount so a
//! shared drive is browsable with normal filesystem tools. Linux-first,
//! feature-gated. Implemented in roadmap issues #38–#39.

#![forbid(unsafe_code)]
