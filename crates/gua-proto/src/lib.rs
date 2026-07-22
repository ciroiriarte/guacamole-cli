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
//! # Example
//!
//! ```
//! use gua_proto::{Decoder, Instruction};
//!
//! // Encode.
//! let ready = Instruction::new("ready", ["$260d01d6-...".to_string()]);
//! let wire = ready.encode();
//!
//! // Decode incrementally — bytes may arrive in any chunking.
//! let mut dec = Decoder::new();
//! dec.push_str("4.size,4.10");
//! assert!(dec.next_instruction().unwrap().is_none()); // incomplete
//! dec.push_str("24,3.768,2.96;");
//! let inst = dec.next_instruction().unwrap().unwrap();
//! assert_eq!(inst.opcode(), "size");
//! assert_eq!(inst.args(), ["1024", "768", "96"]);
//! ```
//!
//! [`gua-tunnel`]: https://github.com/ciroiriarte/guacamole-cli

#![forbid(unsafe_code)]

mod decode;
mod error;
mod instruction;

pub mod handshake;

pub use decode::{parse_all, parse_one, Decoder, DEFAULT_MAX_LEN};
pub use error::{ParseError, TypedError};
pub use handshake::TypedInstruction;
pub use instruction::{encode_all, Instruction};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_codepoint_lengths_not_bytes() {
        // "£" is 1 codepoint but 2 UTF-8 bytes; "😀" is 1 codepoint, 4 bytes.
        let inst = Instruction::new("x", ["£😀"]);
        assert_eq!(inst.encode(), "1.x,2.£😀;");
    }

    #[test]
    fn round_trips_via_decoder() {
        let instrs = vec![
            Instruction::new("select", ["ssh"]),
            Instruction::new("size", ["1024", "768", "96"]),
            Instruction::bare("nop"),
            Instruction::new("x", ["£😀 mixed", ""]),
        ];
        let wire = encode_all(&instrs);

        let mut dec = Decoder::new();
        dec.push_str(&wire);
        let mut out = Vec::new();
        while let Some(inst) = dec.next_instruction().unwrap() {
            out.push(inst);
        }
        assert_eq!(instrs, out);
    }

    #[test]
    fn parse_all_matches_encode_all() {
        let instrs = vec![
            Instruction::new("args", ["VERSION_1_5_0", "hostname"]),
            Instruction::new("ready", ["$id"]),
        ];
        let wire = encode_all(&instrs);
        assert_eq!(parse_all(wire.as_bytes()).unwrap(), instrs);
    }

    #[test]
    fn tolerates_inter_instruction_whitespace() {
        // Fixtures put a newline after each ';'.
        let wire = "3.nop;\n5.ready,3.$id;\n";
        let parsed = parse_all(wire.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].opcode(), "ready");
    }

    #[test]
    fn zero_length_element_is_empty_string() {
        let mut dec = Decoder::new();
        dec.push_str("7.connect,0.,5.alice;");
        let inst = dec.next_instruction().unwrap().unwrap();
        assert_eq!(inst.args(), ["", "alice"]);
    }

    #[test]
    fn multibyte_char_split_across_chunks_is_incomplete_then_completes() {
        // "£" = bytes 0xC2 0xA3. Feed the first byte only.
        let full = Instruction::new("x", ["£"]).encode(); // "1.x,1.£;"
        let bytes = full.as_bytes();
        // Split in the middle of the multi-byte value.
        let split = bytes.len() - 2; // just before 0xC2
        let mut dec = Decoder::new();
        dec.push_bytes(&bytes[..split + 1]); // include only 0xC2
        assert!(dec.next_instruction().unwrap().is_none());
        dec.push_bytes(&bytes[split + 1..]); // the rest
        let inst = dec.next_instruction().unwrap().unwrap();
        assert_eq!(inst.args(), ["£"]);
    }

    #[test]
    fn rejects_malformed() {
        // Non-digit where a length is expected.
        let mut d = Decoder::new();
        d.push_str("x.nop;");
        assert!(matches!(
            d.next_instruction(),
            Err(ParseError::ExpectedLength { .. })
        ));

        // Wrong delimiter after a value.
        let mut d = Decoder::new();
        d.push_str("3.nop|");
        assert!(matches!(
            d.next_instruction(),
            Err(ParseError::ExpectedDelimiter { .. })
        ));

        // Missing dot after length.
        let mut d = Decoder::new();
        d.push_str("3nop;");
        assert!(matches!(
            d.next_instruction(),
            Err(ParseError::ExpectedDot { .. })
        ));
    }

    #[test]
    fn declared_length_bound_is_enforced() {
        let mut d = Decoder::with_max_len(8);
        d.push_str("100.");
        assert!(matches!(
            d.next_instruction(),
            Err(ParseError::LengthTooLarge { .. })
        ));
    }

    #[test]
    fn lonely_length_prefix_is_incomplete_not_error() {
        let mut d = Decoder::new();
        d.push_str("10"); // could still be "100.", need more bytes
        assert!(d.next_instruction().unwrap().is_none());
    }
}
