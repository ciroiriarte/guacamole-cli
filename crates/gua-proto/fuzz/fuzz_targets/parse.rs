#![no_main]
//! Fuzz the streaming decoder: arbitrary bytes must never panic; the decoder
//! must always terminate in `Ok(None)` or `Err`.

use libfuzzer_sys::fuzz_target;

use gua_proto::Decoder;

fuzz_target!(|data: &[u8]| {
    let mut dec = Decoder::with_max_len(1 << 20);
    dec.push_bytes(data);
    loop {
        match dec.next_instruction() {
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => break,
        }
    }
});
