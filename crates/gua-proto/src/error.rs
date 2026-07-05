//! Error types for parsing and typed-instruction conversion.
//!
//! Hand-rolled (no `thiserror`) to keep this keystone crate dependency-free and
//! trivially fuzzable.

use std::fmt;

/// A failure while decoding the Guacamole wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// A length field was expected but a non-digit byte was found.
    ExpectedLength {
        /// The offending byte.
        found: u8,
    },
    /// A `.` separating an element length from its value was expected.
    ExpectedDot {
        /// The offending byte.
        found: u8,
    },
    /// A `,` or `;` element/instruction delimiter was expected.
    ExpectedDelimiter {
        /// The offending byte.
        found: u8,
    },
    /// A declared element length overflowed or exceeded the configured maximum.
    LengthTooLarge {
        /// The declared codepoint length (or `None` on integer overflow).
        declared: Option<usize>,
        /// The configured maximum.
        max: usize,
    },
    /// The buffered but still-incomplete input exceeded the configured maximum.
    BufferTooLarge {
        /// Current buffered byte length.
        len: usize,
        /// The configured maximum.
        max: usize,
    },
    /// An element value was not valid UTF-8.
    InvalidUtf8,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::ExpectedLength { found } => {
                write!(f, "expected element length digit, found byte 0x{found:02x}")
            }
            ParseError::ExpectedDot { found } => {
                write!(f, "expected '.', found byte 0x{found:02x}")
            }
            ParseError::ExpectedDelimiter { found } => {
                write!(f, "expected ',' or ';', found byte 0x{found:02x}")
            }
            ParseError::LengthTooLarge { declared, max } => match declared {
                Some(d) => write!(f, "declared element length {d} exceeds max {max}"),
                None => write!(f, "declared element length overflowed (max {max})"),
            },
            ParseError::BufferTooLarge { len, max } => {
                write!(
                    f,
                    "incomplete instruction buffer of {len} bytes exceeds max {max}"
                )
            }
            ParseError::InvalidUtf8 => f.write_str("element value is not valid UTF-8"),
        }
    }
}

impl std::error::Error for ParseError {}

/// A failure while converting between an [`Instruction`](crate::Instruction) and
/// a typed handshake instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypedError {
    /// The instruction opcode did not match the expected typed opcode.
    WrongOpcode {
        /// The opcode the type expected.
        expected: &'static str,
        /// The opcode that was found.
        found: String,
    },
    /// A required argument at `index` was missing.
    MissingArg {
        /// The expected opcode.
        opcode: &'static str,
        /// The missing argument index.
        index: usize,
    },
    /// An argument that should have been numeric could not be parsed.
    NotANumber {
        /// The expected opcode.
        opcode: &'static str,
        /// The argument index.
        index: usize,
        /// The value that failed to parse.
        value: String,
    },
}

impl fmt::Display for TypedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypedError::WrongOpcode { expected, found } => {
                write!(f, "expected opcode {expected:?}, found {found:?}")
            }
            TypedError::MissingArg { opcode, index } => {
                write!(f, "{opcode}: missing argument at index {index}")
            }
            TypedError::NotANumber {
                opcode,
                index,
                value,
            } => write!(f, "{opcode}: argument {index} {value:?} is not a number"),
        }
    }
}

impl std::error::Error for TypedError {}
