//! Typed handshake instructions.
//!
//! Thin, lossless views over [`Instruction`] for the connection handshake and
//! control opcodes. Client→server types build an `Instruction`; server→client
//! types parse one. All implement [`TypedInstruction`].
//!
//! The handshake sequence (verified against `guacamole-client`):
//! client `select` → server [`Args`] → client [`Size`]/[`Audio`]/[`Video`]/
//! [`Image`]/[`Timezone`] → client [`Connect`] (first value echoes the
//! negotiated version) → server [`Ready`] (first arg is the tunnel UUID).

use crate::error::TypedError;
use crate::instruction::Instruction;

/// Conversion between a typed instruction and the generic [`Instruction`].
pub trait TypedInstruction: Sized {
    /// The wire opcode for this instruction.
    const OPCODE: &'static str;

    /// Render as a generic [`Instruction`].
    fn to_instruction(&self) -> Instruction;

    /// Parse from a generic [`Instruction`], checking the opcode.
    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError>;
}

fn check_opcode(inst: &Instruction, expected: &'static str) -> Result<(), TypedError> {
    if inst.opcode() == expected {
        Ok(())
    } else {
        Err(TypedError::WrongOpcode {
            expected,
            found: inst.opcode().to_string(),
        })
    }
}

fn required<'a>(
    inst: &'a Instruction,
    opcode: &'static str,
    index: usize,
) -> Result<&'a str, TypedError> {
    inst.arg(index)
        .ok_or(TypedError::MissingArg { opcode, index })
}

fn parse_num<T: std::str::FromStr>(
    value: &str,
    opcode: &'static str,
    index: usize,
) -> Result<T, TypedError> {
    value.parse::<T>().map_err(|_| TypedError::NotANumber {
        opcode,
        index,
        value: value.to_string(),
    })
}

/// `args` — server advertises the protocol version and connection parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// Protocol version token, e.g. `VERSION_1_5_0`.
    pub version: String,
    /// Names of the connection parameters the server expects, in order.
    pub parameters: Vec<String>,
}

impl TypedInstruction for Args {
    const OPCODE: &'static str = "args";

    fn to_instruction(&self) -> Instruction {
        let mut elems = Vec::with_capacity(self.parameters.len() + 1);
        elems.push(self.version.clone());
        elems.extend(self.parameters.iter().cloned());
        Instruction::new(Self::OPCODE, elems)
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        let version = required(inst, Self::OPCODE, 0)?.to_string();
        let parameters = inst.args().iter().skip(1).cloned().collect();
        Ok(Args {
            version,
            parameters,
        })
    }
}

/// `size` — client display dimensions (handshake form: width, height, dpi).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Resolution in dots per inch.
    pub dpi: u32,
}

impl TypedInstruction for Size {
    const OPCODE: &'static str = "size";

    fn to_instruction(&self) -> Instruction {
        Instruction::new(
            Self::OPCODE,
            [
                self.width.to_string(),
                self.height.to_string(),
                self.dpi.to_string(),
            ],
        )
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        let width = parse_num(required(inst, Self::OPCODE, 0)?, Self::OPCODE, 0)?;
        let height = parse_num(required(inst, Self::OPCODE, 1)?, Self::OPCODE, 1)?;
        // dpi is conventionally present; default to 96 if omitted.
        let dpi = match inst.arg(2) {
            Some(v) => parse_num(v, Self::OPCODE, 2)?,
            None => 96,
        };
        Ok(Size { width, height, dpi })
    }
}

/// A capability instruction carrying a list of MIME types (`audio`/`video`/
/// `image`).
macro_rules! mime_list_instruction {
    ($(#[$m:meta])* $name:ident, $opcode:literal) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Default)]
        pub struct $name {
            /// Supported MIME types, in preference order (may be empty).
            pub mimetypes: Vec<String>,
        }

        impl TypedInstruction for $name {
            const OPCODE: &'static str = $opcode;

            fn to_instruction(&self) -> Instruction {
                Instruction::new(Self::OPCODE, self.mimetypes.iter().cloned())
            }

            fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
                check_opcode(inst, Self::OPCODE)?;
                Ok($name {
                    mimetypes: inst.args().to_vec(),
                })
            }
        }
    };
}

mime_list_instruction!(
    /// `audio` — client-supported audio MIME types.
    Audio, "audio"
);
mime_list_instruction!(
    /// `video` — client-supported video MIME types (often empty).
    Video, "video"
);
mime_list_instruction!(
    /// `image` — client-supported image MIME types.
    Image, "image"
);

/// `timezone` — client IANA timezone (protocol 1.1.0+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timezone {
    /// IANA timezone name, e.g. `America/Asuncion`.
    pub tz: String,
}

impl TypedInstruction for Timezone {
    const OPCODE: &'static str = "timezone";

    fn to_instruction(&self) -> Instruction {
        Instruction::new(Self::OPCODE, [self.tz.clone()])
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        Ok(Timezone {
            tz: required(inst, Self::OPCODE, 0)?.to_string(),
        })
    }
}

/// `connect` — client completes the handshake with parameter values.
///
/// The first value echoes the negotiated protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Connect {
    /// Parameter values, aligned to the server's [`Args::parameters`].
    pub values: Vec<String>,
}

impl TypedInstruction for Connect {
    const OPCODE: &'static str = "connect";

    fn to_instruction(&self) -> Instruction {
        Instruction::new(Self::OPCODE, self.values.iter().cloned())
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        Ok(Connect {
            values: inst.args().to_vec(),
        })
    }
}

/// `ready` — server signals the interactive phase; the first arg is the tunnel
/// UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    /// Unique tunnel/connection identifier (may be reused with `select`).
    pub id: String,
}

impl TypedInstruction for Ready {
    const OPCODE: &'static str = "ready";

    fn to_instruction(&self) -> Instruction {
        Instruction::new(Self::OPCODE, [self.id.clone()])
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        Ok(Ready {
            id: required(inst, Self::OPCODE, 0)?.to_string(),
        })
    }
}

/// `sync` — timestamp exchange / frame acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sync {
    /// Server timestamp in milliseconds; the client echoes it back.
    pub timestamp: i64,
}

impl TypedInstruction for Sync {
    const OPCODE: &'static str = "sync";

    fn to_instruction(&self) -> Instruction {
        Instruction::new(Self::OPCODE, [self.timestamp.to_string()])
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        let timestamp = parse_num(required(inst, Self::OPCODE, 0)?, Self::OPCODE, 0)?;
        Ok(Sync { timestamp })
    }
}

/// `nop` — keep-alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Nop;

impl TypedInstruction for Nop {
    const OPCODE: &'static str = "nop";

    fn to_instruction(&self) -> Instruction {
        Instruction::bare(Self::OPCODE)
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        Ok(Nop)
    }
}

/// `disconnect` — orderly shutdown (either direction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Disconnect;

impl TypedInstruction for Disconnect {
    const OPCODE: &'static str = "disconnect";

    fn to_instruction(&self) -> Instruction {
        Instruction::bare(Self::OPCODE)
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        Ok(Disconnect)
    }
}

/// `error` — server reports a failure with a human message and status code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorInstruction {
    /// Human-readable message (may be empty).
    pub message: String,
    /// Guacamole status code (0 = success; non-zero classes the failure).
    pub status: u32,
}

impl TypedInstruction for ErrorInstruction {
    const OPCODE: &'static str = "error";

    fn to_instruction(&self) -> Instruction {
        Instruction::new(
            Self::OPCODE,
            [self.message.clone(), self.status.to_string()],
        )
    }

    fn from_instruction(inst: &Instruction) -> Result<Self, TypedError> {
        check_opcode(inst, Self::OPCODE)?;
        // Both fields are lenient: guacd usually sends (message, status) but we
        // tolerate either being absent.
        let message = inst.arg(0).unwrap_or_default().to_string();
        let status = match inst.arg(1) {
            Some(v) => parse_num(v, Self::OPCODE, 1)?,
            None => 0,
        };
        Ok(ErrorInstruction { message, status })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: T)
    where
        T: TypedInstruction + PartialEq + std::fmt::Debug,
    {
        let inst = value.to_instruction();
        let back = T::from_instruction(&inst).expect("from_instruction");
        assert_eq!(value, back);
    }

    #[test]
    fn typed_round_trips() {
        round_trip(Args {
            version: "VERSION_1_5_0".into(),
            parameters: vec!["hostname".into(), "port".into()],
        });
        round_trip(Size {
            width: 1024,
            height: 768,
            dpi: 96,
        });
        round_trip(Audio {
            mimetypes: vec!["audio/L16".into()],
        });
        round_trip(Video::default());
        round_trip(Image {
            mimetypes: vec!["image/png".into(), "image/jpeg".into()],
        });
        round_trip(Timezone {
            tz: "America/Asuncion".into(),
        });
        round_trip(Connect {
            values: vec!["VERSION_1_5_0".into(), "host".into()],
        });
        round_trip(Ready {
            id: "$abc-123".into(),
        });
        round_trip(Sync {
            timestamp: 1_700_000_000_000,
        });
        round_trip(Nop);
        round_trip(Disconnect);
        round_trip(ErrorInstruction {
            message: "upstream timeout".into(),
            status: 519,
        });
    }

    #[test]
    fn wrong_opcode_is_rejected() {
        let inst = Instruction::bare("nop");
        let err = Ready::from_instruction(&inst).unwrap_err();
        assert_eq!(
            err,
            TypedError::WrongOpcode {
                expected: "ready",
                found: "nop".into()
            }
        );
    }

    #[test]
    fn size_defaults_dpi_and_rejects_nonnumeric() {
        let inst = Instruction::new("size", ["800", "600"]);
        assert_eq!(Size::from_instruction(&inst).unwrap().dpi, 96);

        let bad = Instruction::new("size", ["wide", "600", "96"]);
        assert!(matches!(
            Size::from_instruction(&bad),
            Err(TypedError::NotANumber { index: 0, .. })
        ));
    }
}
