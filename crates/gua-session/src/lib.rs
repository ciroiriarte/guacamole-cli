//! Guacamole session lifecycle and stream demultiplexing.
//!
//! The current implementation focuses on the terminal `text-output` path: it
//! recognizes the patched guacd `STDOUT` pipe, decodes base64 `blob` payloads,
//! acknowledges each blob for server-side flow control, and echoes `sync`.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use gua_core::{Error, Result};
use gua_proto::{handshake::Sync, Instruction, TypedInstruction};
use gua_tunnel::{Tunnel, TunnelParams};

/// Name of the patched guacd raw terminal output pipe.
pub const STDOUT_PIPE_NAME: &str = "STDOUT";

/// Mimetype used by the patched guacd raw terminal output pipe.
pub const STDOUT_MIMETYPE: &str = "application/octet-stream";

/// High-level events emitted by a live Guacamole session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// The expected `STDOUT` pipe has been opened.
    StdoutOpened { stream: String, mimetype: String },
    /// Raw terminal bytes arrived from the `STDOUT` pipe.
    StdoutBytes(Vec<u8>),
    /// The `STDOUT` pipe ended.
    StdoutEnded,
    /// Server requested disconnect or closed the protocol session.
    Disconnected,
    /// A non-STDOUT pipe/stream/control instruction was ignored by text mode.
    Ignored(Instruction),
}

/// Live Guacamole session backed by a tunnel.
pub struct Session {
    tunnel: Tunnel,
    streams: HashMap<String, StreamKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Stdout,
    Other,
}

impl Session {
    /// Open a connection session through guacamole-client.
    pub fn connect(params: TunnelParams<'_>) -> Result<Self> {
        Ok(Self {
            tunnel: Tunnel::connect_connection(params)?,
            streams: HashMap::new(),
        })
    }

    /// Read and demultiplex the next high-level session event.
    pub fn next_event(&mut self) -> Result<SessionEvent> {
        loop {
            let inst = self.tunnel.read_instruction()?;
            if let Some(event) = self.handle_instruction(inst)? {
                return Ok(event);
            }
        }
    }

    /// Read and demultiplex the next high-level session event, returning
    /// `Ok(None)` if no tunnel message arrives before `timeout`.
    pub fn next_event_timeout(&mut self, timeout: Duration) -> Result<Option<SessionEvent>> {
        loop {
            let Some(inst) = self.tunnel.read_instruction_timeout(timeout)? else {
                return Ok(None);
            };
            if let Some(event) = self.handle_instruction(inst)? {
                return Ok(Some(event));
            }
        }
    }

    /// Send one Unicode/control key press and release through the Guacamole
    /// keyboard protocol.
    pub fn send_key(&mut self, keysym: u32) -> Result<()> {
        self.send_key_state(keysym, true)?;
        self.send_key_state(keysym, false)
    }

    /// Send one key with synthetic modifier press/release events.
    pub fn send_key_combo(&mut self, modifiers: &[u32], keysym: u32) -> Result<()> {
        for modifier in modifiers {
            self.send_key_state(*modifier, true)?;
        }
        self.send_key(keysym)?;
        for modifier in modifiers.iter().rev() {
            self.send_key_state(*modifier, false)?;
        }
        Ok(())
    }

    fn send_key_state(&mut self, keysym: u32, pressed: bool) -> Result<()> {
        self.tunnel.send(&Instruction::new(
            "key",
            [
                keysym.to_string(),
                if pressed { "1" } else { "0" }.to_string(),
            ],
        ))
    }

    /// Send an orderly disconnect.
    pub fn disconnect(self) -> Result<()> {
        self.tunnel.disconnect()
    }

    fn handle_instruction(&mut self, inst: Instruction) -> Result<Option<SessionEvent>> {
        match inst.opcode() {
            "pipe" => Ok(Some(self.handle_pipe(inst)?)),
            "blob" => self.handle_blob(inst),
            "end" => Ok(Some(self.handle_end(inst))),
            "sync" => {
                let sync = Sync::from_instruction(&inst)
                    .map_err(|e| Error::Protocol(format!("invalid sync instruction: {e}")))?;
                self.tunnel.send(&sync.to_instruction())?;
                Ok(None)
            }
            "nop" => Ok(None),
            "disconnect" => Ok(Some(SessionEvent::Disconnected)),
            "error" => Err(Error::Protocol(format!(
                "server error: {} ({})",
                inst.arg(0).unwrap_or_default(),
                inst.arg(1).unwrap_or("unknown")
            ))),
            _ => Ok(Some(SessionEvent::Ignored(inst))),
        }
    }

    fn handle_pipe(&mut self, inst: Instruction) -> Result<SessionEvent> {
        let (stream, mimetype, is_stdout) = classify_pipe(&inst)?;
        if is_stdout {
            self.streams.insert(stream.clone(), StreamKind::Stdout);
            Ok(SessionEvent::StdoutOpened { stream, mimetype })
        } else {
            self.streams.insert(stream, StreamKind::Other);
            Ok(SessionEvent::Ignored(inst))
        }
    }

    fn handle_blob(&mut self, inst: Instruction) -> Result<Option<SessionEvent>> {
        let stream = inst
            .arg(0)
            .ok_or_else(|| Error::Protocol("blob missing stream index".into()))?
            .to_string();
        let payload = inst
            .arg(1)
            .ok_or_else(|| Error::Protocol("blob missing payload".into()))?;

        if self.streams.get(&stream) != Some(&StreamKind::Stdout) {
            return Ok(Some(SessionEvent::Ignored(inst)));
        }

        let bytes = STANDARD
            .decode(payload)
            .map_err(|e| Error::Protocol(format!("invalid STDOUT blob base64: {e}")))?;

        // Required by the patched guacd text-output implementation: without
        // blob acks, guacd will eventually drop output to bound backlog.
        self.tunnel.send(&Instruction::new(
            "ack",
            [stream, "OK".to_string(), "0".to_string()],
        ))?;

        Ok(Some(SessionEvent::StdoutBytes(bytes)))
    }

    fn handle_end(&mut self, inst: Instruction) -> SessionEvent {
        let Some(stream) = inst.arg(0).map(str::to_string) else {
            return SessionEvent::Ignored(inst);
        };
        match self.streams.remove(&stream) {
            Some(StreamKind::Stdout) => SessionEvent::StdoutEnded,
            _ => SessionEvent::Ignored(inst),
        }
    }
}

fn classify_pipe(inst: &Instruction) -> Result<(String, String, bool)> {
    let stream = inst
        .arg(0)
        .ok_or_else(|| Error::Protocol("pipe missing stream index".into()))?
        .to_string();
    let mimetype = inst
        .arg(1)
        .ok_or_else(|| Error::Protocol("pipe missing mimetype".into()))?
        .to_string();
    let name = inst
        .arg(2)
        .ok_or_else(|| Error::Protocol("pipe missing name".into()))?;
    Ok((stream, mimetype, name == STDOUT_PIPE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_stdout_pipe_contract() {
        let inst = Instruction::new("pipe", ["2", STDOUT_MIMETYPE, STDOUT_PIPE_NAME]);
        let (stream, mimetype, is_stdout) = classify_pipe(&inst).unwrap();
        assert_eq!(stream, "2");
        assert_eq!(mimetype, STDOUT_MIMETYPE);
        assert!(is_stdout);
    }

    #[test]
    fn ignores_non_stdout_pipe() {
        let inst = Instruction::new("pipe", ["4", "text/plain", "OTHER"]);
        let (_, _, is_stdout) = classify_pipe(&inst).unwrap();
        assert!(!is_stdout);
    }
}
