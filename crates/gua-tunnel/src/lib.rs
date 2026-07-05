//! Guacamole tunnel transport.
//!
//! Opens the gateway WebSocket tunnel (`/websocket-tunnel`, with an HTTP
//! `/tunnel` fallback), negotiates the Guacamole handshake, and exchanges
//! [`gua_proto`] instructions with `guacd`. Implemented in roadmap issue #19.

#![forbid(unsafe_code)]
