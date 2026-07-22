//! Parse the committed handshake fixture and verify structural round-tripping.

use gua_proto::{encode_all, parse_all, Decoder};

const HANDSHAKE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/handshake/minimal-ssh.guac"
));

#[test]
fn handshake_fixture_parses_expected_sequence() {
    let instrs = parse_all(HANDSHAKE).expect("fixture parses");
    let opcodes: Vec<&str> = instrs.iter().map(|i| i.opcode()).collect();
    assert_eq!(
        opcodes,
        ["select", "args", "size", "audio", "video", "image", "timezone", "connect", "ready",]
    );

    // `connect` echoes the version and ends with an empty (password) value.
    let connect = &instrs[7];
    assert_eq!(connect.arg(0), Some("VERSION_1_5_0"));
    assert_eq!(connect.args().last().map(String::as_str), Some(""));

    // `ready` carries the tunnel UUID (guacd prefixes it with `$`).
    assert_eq!(instrs[8].opcode(), "ready");
    assert!(instrs[8].arg(0).unwrap().starts_with('$'));
}

#[test]
fn fixture_survives_reencode_and_bytewise_streaming() {
    let instrs = parse_all(HANDSHAKE).expect("fixture parses");

    // Canonical re-encode (no inter-instruction whitespace) round-trips.
    let wire = encode_all(&instrs);
    assert_eq!(parse_all(wire.as_bytes()).unwrap(), instrs);

    // Feeding the canonical bytes one at a time yields the same instructions.
    let mut dec = Decoder::new();
    let mut streamed = Vec::new();
    for &b in wire.as_bytes() {
        dec.push_bytes(&[b]);
        while let Some(inst) = dec.next_instruction().unwrap() {
            streamed.push(inst);
        }
    }
    assert_eq!(streamed, instrs);
}
