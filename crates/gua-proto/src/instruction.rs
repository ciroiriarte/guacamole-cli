//! The [`Instruction`] type and its wire encoding.

use std::fmt;

/// A single decoded Guacamole instruction: an opcode plus zero or more string
/// arguments.
///
/// The wire form is `LENGTH.OPCODE,LENGTH.ARG,...;` where each `LENGTH` is the
/// element length in **Unicode codepoints, not bytes**.
///
/// ```
/// use gua_proto::Instruction;
/// let inst = Instruction::new("size", ["1024", "768", "96"]);
/// assert_eq!(inst.encode(), "4.size,4.1024,3.768,2.96;");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Instruction {
    opcode: String,
    args: Vec<String>,
}

impl Instruction {
    /// Build an instruction from an opcode and arguments.
    pub fn new<O, I, S>(opcode: O, args: I) -> Self
    where
        O: Into<String>,
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Instruction {
            opcode: opcode.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Build an instruction with no arguments (e.g. `nop`, `disconnect`).
    pub fn bare<O: Into<String>>(opcode: O) -> Self {
        Instruction {
            opcode: opcode.into(),
            args: Vec::new(),
        }
    }

    /// The opcode (first element).
    pub fn opcode(&self) -> &str {
        &self.opcode
    }

    /// The arguments (all elements after the opcode).
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The argument at `index`, if present.
    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args.get(index).map(String::as_str)
    }

    /// Consume the instruction, returning `(opcode, args)`.
    pub fn into_parts(self) -> (String, Vec<String>) {
        (self.opcode, self.args)
    }

    /// Append the wire encoding of this instruction to `out`.
    pub fn encode_into(&self, out: &mut String) {
        push_element(out, &self.opcode);
        for arg in &self.args {
            out.push(',');
            push_element(out, arg);
        }
        out.push(';');
    }

    /// The wire encoding of this instruction (`LENGTH.VALUE,...;`).
    pub fn encode(&self) -> String {
        // Pre-size: element bytes + a length/sep overhead per element.
        let mut out = String::with_capacity(self.encoded_len_hint());
        self.encode_into(&mut out);
        out
    }

    fn encoded_len_hint(&self) -> usize {
        let mut n = self.opcode.len() + 6;
        for a in &self.args {
            n += a.len() + 6;
        }
        n
    }
}

/// Encode multiple instructions back-to-back (no separators), as they travel on
/// the wire.
pub fn encode_all(instructions: &[Instruction]) -> String {
    let mut out = String::new();
    for inst in instructions {
        inst.encode_into(&mut out);
    }
    out
}

fn push_element(out: &mut String, value: &str) {
    use fmt::Write as _;
    // Length is the codepoint count, not the byte count.
    let len = value.chars().count();
    let _ = write!(out, "{len}");
    out.push('.');
    out.push_str(value);
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}
