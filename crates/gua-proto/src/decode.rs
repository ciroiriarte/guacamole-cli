//! Streaming decoder for the Guacamole wire format.
//!
//! [`Decoder`] accepts bytes in arbitrary chunks (as delivered by a tunnel) and
//! yields complete [`Instruction`]s. It correctly handles:
//!
//! - element lengths measured in **codepoints, not bytes**;
//! - instructions and multi-byte characters split across chunk boundaries
//!   (returns "need more data" rather than erroring);
//! - inter-instruction ASCII whitespace (tolerated and skipped, per the fixture
//!   format);
//! - bounded memory: a configurable maximum guards against a malicious stream
//!   that never terminates an instruction.

use crate::error::ParseError;
use crate::instruction::Instruction;

/// Default maximum for a single declared element length and for the buffered
/// bytes of one in-progress instruction (16 MiB).
pub const DEFAULT_MAX_LEN: usize = 16 * 1024 * 1024;

/// Incremental decoder over a byte stream.
#[derive(Debug, Clone)]
pub struct Decoder {
    buf: Vec<u8>,
    /// Bytes at the front of `buf` already yielded, awaiting compaction.
    start: usize,
    max_len: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// A decoder with the [`DEFAULT_MAX_LEN`] bound.
    pub fn new() -> Self {
        Self::with_max_len(DEFAULT_MAX_LEN)
    }

    /// A decoder bounding declared element length and in-progress buffering to
    /// `max_len` bytes.
    pub fn with_max_len(max_len: usize) -> Self {
        Decoder {
            buf: Vec::new(),
            start: 0,
            max_len,
        }
    }

    /// Append raw bytes to the decode buffer.
    pub fn push_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Append a string slice to the decode buffer.
    pub fn push_str(&mut self, data: &str) {
        self.push_bytes(data.as_bytes());
    }

    /// Pull the next complete instruction, or `Ok(None)` if more bytes are
    /// needed. Errors on malformed input or a bound violation.
    pub fn next_instruction(&mut self) -> Result<Option<Instruction>, ParseError> {
        match parse_one(&self.buf[self.start..], self.max_len)? {
            Some((inst, consumed)) => {
                self.start += consumed;
                self.compact();
                Ok(Some(inst))
            }
            None => {
                let pending = self.buf.len() - self.start;
                if pending > self.max_len {
                    return Err(ParseError::BufferTooLarge {
                        len: pending,
                        max: self.max_len,
                    });
                }
                Ok(None)
            }
        }
    }

    /// Number of unparsed bytes currently buffered.
    pub fn pending_len(&self) -> usize {
        self.buf.len() - self.start
    }

    /// Reclaim the space of already-yielded bytes once it dominates the buffer.
    fn compact(&mut self) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > 4096 && self.start * 2 >= self.buf.len() {
            self.buf.drain(0..self.start);
            self.start = 0;
        }
    }
}

/// Decode a single instruction from the front of `input`.
///
/// Returns `Ok(Some((instruction, bytes_consumed)))` on success,
/// `Ok(None)` if `input` is a valid but incomplete prefix (need more bytes),
/// or `Err` on malformed input.
pub fn parse_one(input: &[u8], max_len: usize) -> Result<Option<(Instruction, usize)>, ParseError> {
    let mut pos = skip_ws(input, 0);
    if pos >= input.len() {
        return Ok(None);
    }

    let mut elements: Vec<String> = Vec::new();

    loop {
        // --- element length (ASCII decimal) ---
        let len_start = pos;
        let mut len: usize = 0;
        let mut overflow = false;
        while pos < input.len() && input[pos].is_ascii_digit() {
            let digit = (input[pos] - b'0') as usize;
            match len.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                Some(v) => len = v,
                None => overflow = true,
            }
            pos += 1;
        }
        if pos >= input.len() {
            return Ok(None); // need the '.' (or more digits)
        }
        if pos == len_start {
            return Err(ParseError::ExpectedLength { found: input[pos] });
        }
        if overflow || len > max_len {
            return Err(ParseError::LengthTooLarge {
                declared: if overflow { None } else { Some(len) },
                max: max_len,
            });
        }
        if input[pos] != b'.' {
            return Err(ParseError::ExpectedDot { found: input[pos] });
        }
        pos += 1; // consume '.'

        // --- value: exactly `len` codepoints ---
        let value_start = pos;
        let mut seen = 0usize;
        while seen < len {
            if pos >= input.len() {
                return Ok(None); // value not fully arrived
            }
            let char_len = match utf8_char_len(input[pos]) {
                Some(n) => n,
                None => return Err(ParseError::InvalidUtf8),
            };
            if pos + char_len > input.len() {
                return Ok(None); // multi-byte char split across chunks
            }
            // Validate this one character is well-formed UTF-8.
            if std::str::from_utf8(&input[pos..pos + char_len]).is_err() {
                return Err(ParseError::InvalidUtf8);
            }
            pos += char_len;
            seen += 1;
        }
        let value = match std::str::from_utf8(&input[value_start..pos]) {
            Ok(s) => s.to_string(),
            Err(_) => return Err(ParseError::InvalidUtf8),
        };
        elements.push(value);

        // --- delimiter ---
        if pos >= input.len() {
            return Ok(None); // need ',' or ';'
        }
        match input[pos] {
            b',' => {
                pos += 1;
                continue;
            }
            b';' => {
                pos += 1;
                break;
            }
            other => return Err(ParseError::ExpectedDelimiter { found: other }),
        }
    }

    // The loop always pushes at least one element before a ';'.
    let mut it = elements.into_iter();
    let opcode = it.next().expect("at least one element");
    let args: Vec<String> = it.collect();
    Ok(Some((Instruction::new(opcode, args), pos)))
}

/// Parse every complete instruction in `input`, requiring it to end exactly on
/// an instruction boundary (trailing whitespace allowed). Convenience for tests
/// and fixture loading.
pub fn parse_all(input: &[u8]) -> Result<Vec<Instruction>, ParseError> {
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        match parse_one(&input[pos..], DEFAULT_MAX_LEN)? {
            Some((inst, consumed)) => {
                out.push(inst);
                pos += consumed;
            }
            None => {
                // Trailing whitespace after the last instruction is fine.
                if skip_ws(input, pos) >= input.len() {
                    return Ok(out);
                }
                // Otherwise we ran out of bytes mid-instruction.
                return Err(ParseError::BufferTooLarge {
                    len: input.len() - pos,
                    max: DEFAULT_MAX_LEN,
                });
            }
        }
    }
}

fn skip_ws(input: &[u8], mut pos: usize) -> usize {
    while pos < input.len() && input[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

/// Byte length of the UTF-8 character starting with `b`, or `None` for an
/// invalid leading byte (continuation byte or illegal 0xF8..=0xFF).
fn utf8_char_len(b: u8) -> Option<usize> {
    match b {
        0x00..=0x7F => Some(1),
        0xC0..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF7 => Some(4),
        _ => None,
    }
}
