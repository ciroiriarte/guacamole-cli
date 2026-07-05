//! Guacamole session lifecycle and stream demultiplexing.
//!
//! Owns everything common to a live session regardless of renderer: handshake
//! completion, the stream-demux state machine (`clipboard`, `pipe`, `blob`/`end`,
//! `file`/`filesystem`), `sync`/`nop` keepalive and disconnect. Consumed by the
//! TUI, GUI and FUSE front-ends. Implemented in roadmap issue #20.

#![forbid(unsafe_code)]
