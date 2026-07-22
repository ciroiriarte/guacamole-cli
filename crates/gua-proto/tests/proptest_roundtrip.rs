//! Property tests: round-trip fidelity, chunk-invariance, and no-panic
//! robustness against arbitrary bytes.

use gua_proto::{encode_all, parse_all, Decoder, Instruction};
use proptest::prelude::*;

/// Arbitrary element string, including empty and multi-byte characters.
fn arb_element() -> impl Strategy<Value = String> {
    ".{0,32}"
}

/// Arbitrary instruction: any opcode plus 0..8 arguments.
fn arb_instruction() -> impl Strategy<Value = Instruction> {
    (arb_element(), prop::collection::vec(arb_element(), 0..8))
        .prop_map(|(opcode, args)| Instruction::new(opcode, args))
}

proptest! {
    /// Encode → parse is the identity for any single instruction.
    #[test]
    fn single_round_trips(inst in arb_instruction()) {
        let wire = inst.encode();
        let parsed = parse_all(wire.as_bytes()).unwrap();
        prop_assert_eq!(parsed, vec![inst]);
    }

    /// Encode → parse is the identity for any batch of instructions.
    #[test]
    fn batch_round_trips(instrs in prop::collection::vec(arb_instruction(), 0..16)) {
        let wire = encode_all(&instrs);
        let parsed = parse_all(wire.as_bytes()).unwrap();
        prop_assert_eq!(parsed, instrs);
    }

    /// Decoding is invariant to how the byte stream is chunked.
    #[test]
    fn chunking_is_invariant(
        instrs in prop::collection::vec(arb_instruction(), 0..12),
        chunk_sizes in prop::collection::vec(1usize..7, 1..64),
    ) {
        let wire = encode_all(&instrs);
        let bytes = wire.as_bytes();

        let mut dec = Decoder::new();
        let mut out = Vec::new();
        let mut pos = 0;
        let mut i = 0;
        while pos < bytes.len() {
            let n = chunk_sizes[i % chunk_sizes.len()].min(bytes.len() - pos);
            dec.push_bytes(&bytes[pos..pos + n]);
            pos += n;
            i += 1;
            while let Some(inst) = dec.next_instruction().unwrap() {
                out.push(inst);
            }
        }
        prop_assert_eq!(out, instrs);
    }

    /// The decoder never panics on arbitrary (possibly malformed) bytes.
    #[test]
    fn arbitrary_bytes_never_panic(data in prop::collection::vec(any::<u8>(), 0..1024)) {
        let mut dec = Decoder::with_max_len(4096);
        dec.push_bytes(&data);
        // Drain until it needs more bytes or errors; the point is that it never
        // panics.
        while let Ok(Some(_)) = dec.next_instruction() {}
    }
}
