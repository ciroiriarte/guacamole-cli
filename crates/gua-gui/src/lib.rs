//! Native GUI renderer for Guacamole graphical protocols.
//!
//! Renders the Guacamole drawing instruction set (layer compositing of decoded
//! image tiles with porter-duff operators, plus rects/copy/transfer blits) into
//! a native window, with input/audio/clipboard. Starts as a software 2D
//! compositor. Implemented in roadmap issues #29–#37.

#![forbid(unsafe_code)]
