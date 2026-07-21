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
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Name of the patched guacd raw terminal output pipe.
pub const STDOUT_PIPE_NAME: &str = "STDOUT";

/// Mimetype used by the patched guacd raw terminal output pipe.
pub const STDOUT_MIMETYPE: &str = "application/octet-stream";

/// Name of the optional structured IPMI control/status pipe.
pub const IPMI_CONTROL_PIPE_NAME: &str = "ipmi-control";

/// Mimetype used by the IPMI control/status pipe.
pub const IPMI_CONTROL_MIMETYPE: &str = "application/json";

/// High-level events emitted by a live Guacamole session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// The expected `STDOUT` pipe has been opened.
    StdoutOpened { stream: String, mimetype: String },
    /// Raw terminal bytes arrived from the `STDOUT` pipe.
    StdoutBytes(Vec<u8>),
    /// The `STDOUT` pipe ended.
    StdoutEnded,
    /// The optional structured IPMI control pipe has been opened by guacd.
    IpmiControlOpened { stream: String, mimetype: String },
    /// One typed JSON message arrived from the `ipmi-control` pipe.
    IpmiControl(IpmiControlEvent),
    /// The `ipmi-control` pipe ended.
    IpmiControlEnded,
    /// Server requested disconnect or closed the protocol session.
    Disconnected,
    /// A non-STDOUT pipe/stream/control instruction was ignored by text mode.
    Ignored(Instruction),
}

/// Parsed server-to-client IPMI control messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpmiControlEvent {
    State(IpmiState),
    Result(IpmiCommandResult),
    Sel(IpmiSel),
    Unknown(Value),
}

/// Passive IPMI state pushed by guacd.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IpmiState {
    pub power: String,
    #[serde(default)]
    pub identify: Option<bool>,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default, rename = "lastSel")]
    pub last_sel: Option<String>,
}

/// Result of an IPMI control command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IpmiCommandResult {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub message: Option<String>,
}

/// IPMI System Event Log payload.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IpmiSel {
    pub total: u64,
    #[serde(default)]
    pub entries: Vec<IpmiSelEntry>,
}

/// One IPMI SEL entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IpmiSelEntry {
    pub id: u64,
    pub time: String,
    pub severity: String,
    pub sensor: String,
    pub event: String,
}

/// Client-to-server IPMI control commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpmiCommand {
    PowerOn,
    PowerOff,
    PowerCycle,
    HardReset,
    SoftShutdown,
    DiagnosticInterrupt,
    Identify,
    SendBreak,
    RefreshStatus,
    ReadSel,
}

impl IpmiCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PowerOn => "power-on",
            Self::PowerOff => "power-off",
            Self::PowerCycle => "power-cycle",
            Self::HardReset => "hard-reset",
            Self::SoftShutdown => "soft-shutdown",
            Self::DiagnosticInterrupt => "diagnostic-interrupt",
            Self::Identify => "identify",
            Self::SendBreak => "send-break",
            Self::RefreshStatus => "refresh-status",
            Self::ReadSel => "read-sel",
        }
    }

    pub fn confirmation(self) -> IpmiConfirmation {
        match self {
            Self::PowerOff | Self::HardReset | Self::DiagnosticInterrupt => {
                IpmiConfirmation::TypeToConfirm
            }
            Self::PowerCycle | Self::SoftShutdown => IpmiConfirmation::Simple,
            Self::PowerOn
            | Self::Identify
            | Self::SendBreak
            | Self::RefreshStatus
            | Self::ReadSel => IpmiConfirmation::None,
        }
    }
}

/// Safety tier required before sending an IPMI command from an interactive UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpmiConfirmation {
    None,
    Simple,
    TypeToConfirm,
}

#[derive(Debug, Serialize)]
struct IpmiCommandMessage<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    message_type: &'static str,
    command: &'static str,
}

/// Live Guacamole session backed by a tunnel.
pub struct Session {
    tunnel: Tunnel,
    streams: HashMap<String, StreamKind>,
    next_client_stream: u64,
    ipmi_control_outbound_stream: Option<String>,
    next_ipmi_command_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Stdout,
    IpmiControl,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipeKind {
    Stdout,
    IpmiControl,
    Other,
}

impl Session {
    /// Open a connection session through guacamole-client.
    pub fn connect(params: TunnelParams<'_>) -> Result<Self> {
        Ok(Self {
            tunnel: Tunnel::connect_connection(params)?,
            streams: HashMap::new(),
            next_client_stream: 0,
            ipmi_control_outbound_stream: None,
            next_ipmi_command_id: 0,
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

    /// Send a terminal/display size update through the Guacamole protocol.
    pub fn send_size(&mut self, width: u32, height: u32, dpi: u32) -> Result<()> {
        self.tunnel.send(&Instruction::new(
            "size",
            [width.to_string(), height.to_string(), dpi.to_string()],
        ))
    }

    /// Send one typed JSON command over the client-side `ipmi-control` pipe.
    ///
    /// This opens the client-to-server pipe lazily and keeps it open for later
    /// commands. Server-to-client status/results are delivered independently on
    /// the server-opened `ipmi-control` pipe and emitted as [`SessionEvent::IpmiControl`].
    pub fn send_ipmi_command(&mut self, command: IpmiCommand) -> Result<String> {
        let id = self.next_ipmi_command_id();
        let stream = self.ensure_ipmi_control_output_stream()?;
        let payload = serde_json::to_vec(&IpmiCommandMessage {
            id: &id,
            message_type: "command",
            command: command.as_str(),
        })
        .map_err(|e| Error::Protocol(format!("encoding ipmi-control command JSON: {e}")))?;
        self.tunnel.send(&Instruction::new(
            "blob",
            [stream, STANDARD.encode(payload)],
        ))?;
        Ok(id)
    }

    fn ensure_ipmi_control_output_stream(&mut self) -> Result<String> {
        if let Some(stream) = &self.ipmi_control_outbound_stream {
            return Ok(stream.clone());
        }
        let stream = self.alloc_client_stream();
        self.tunnel.send(&Instruction::new(
            "pipe",
            [
                stream.clone(),
                IPMI_CONTROL_MIMETYPE.to_string(),
                IPMI_CONTROL_PIPE_NAME.to_string(),
            ],
        ))?;
        self.ipmi_control_outbound_stream = Some(stream.clone());
        Ok(stream)
    }

    fn alloc_client_stream(&mut self) -> String {
        let stream = self.next_client_stream.to_string();
        self.next_client_stream += 1;
        stream
    }

    fn next_ipmi_command_id(&mut self) -> String {
        let id = format!("gua-ipmi-{}", self.next_ipmi_command_id);
        self.next_ipmi_command_id += 1;
        id
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
            "ack" => Ok(None),
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
        let (stream, mimetype, kind) = classify_pipe(&inst)?;
        match kind {
            PipeKind::Stdout => {
                self.streams.insert(stream.clone(), StreamKind::Stdout);
                Ok(SessionEvent::StdoutOpened { stream, mimetype })
            }
            PipeKind::IpmiControl => {
                self.streams.insert(stream.clone(), StreamKind::IpmiControl);
                Ok(SessionEvent::IpmiControlOpened { stream, mimetype })
            }
            PipeKind::Other => {
                self.streams.insert(stream, StreamKind::Other);
                Ok(SessionEvent::Ignored(inst))
            }
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

        match self.streams.get(&stream).copied() {
            Some(StreamKind::Stdout) => self.handle_stdout_blob(stream, payload),
            Some(StreamKind::IpmiControl) => self.handle_ipmi_control_blob(stream, payload),
            _ => Ok(Some(SessionEvent::Ignored(inst))),
        }
    }

    fn handle_stdout_blob(
        &mut self,
        stream: String,
        payload: &str,
    ) -> Result<Option<SessionEvent>> {
        let bytes = STANDARD
            .decode(payload)
            .map_err(|e| Error::Protocol(format!("invalid STDOUT blob base64: {e}")))?;
        self.ack_stream_blob(stream)?;
        Ok(Some(SessionEvent::StdoutBytes(bytes)))
    }

    fn handle_ipmi_control_blob(
        &mut self,
        stream: String,
        payload: &str,
    ) -> Result<Option<SessionEvent>> {
        let bytes = STANDARD
            .decode(payload)
            .map_err(|e| Error::Protocol(format!("invalid ipmi-control blob base64: {e}")))?;
        self.ack_stream_blob(stream)?;
        Ok(Some(SessionEvent::IpmiControl(parse_ipmi_control_event(
            &bytes,
        )?)))
    }

    fn ack_stream_blob(&mut self, stream: String) -> Result<()> {
        // Required by the patched guacd text-output/control stream implementations:
        // without blob acks, guacd will eventually drop or stall bounded output.
        self.tunnel.send(&Instruction::new(
            "ack",
            [stream, "OK".to_string(), "0".to_string()],
        ))
    }

    fn handle_end(&mut self, inst: Instruction) -> SessionEvent {
        let Some(stream) = inst.arg(0).map(str::to_string) else {
            return SessionEvent::Ignored(inst);
        };
        match self.streams.remove(&stream) {
            Some(StreamKind::Stdout) => SessionEvent::StdoutEnded,
            Some(StreamKind::IpmiControl) => SessionEvent::IpmiControlEnded,
            _ => SessionEvent::Ignored(inst),
        }
    }
}

fn parse_ipmi_control_event(bytes: &[u8]) -> Result<IpmiControlEvent> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Protocol(format!("invalid ipmi-control JSON: {e}")))?;
    match value.get("type").and_then(Value::as_str) {
        Some("state") => Ok(IpmiControlEvent::State(
            serde_json::from_value(value)
                .map_err(|e| Error::Protocol(format!("invalid ipmi state JSON: {e}")))?,
        )),
        Some("result") => Ok(IpmiControlEvent::Result(
            serde_json::from_value(value)
                .map_err(|e| Error::Protocol(format!("invalid ipmi result JSON: {e}")))?,
        )),
        Some("sel") => Ok(IpmiControlEvent::Sel(
            serde_json::from_value(value)
                .map_err(|e| Error::Protocol(format!("invalid ipmi SEL JSON: {e}")))?,
        )),
        _ => Ok(IpmiControlEvent::Unknown(value)),
    }
}

fn classify_pipe(inst: &Instruction) -> Result<(String, String, PipeKind)> {
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
    let kind = match name {
        STDOUT_PIPE_NAME => PipeKind::Stdout,
        IPMI_CONTROL_PIPE_NAME => PipeKind::IpmiControl,
        _ => PipeKind::Other,
    };
    Ok((stream, mimetype, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_stdout_pipe_contract() {
        let inst = Instruction::new("pipe", ["2", STDOUT_MIMETYPE, STDOUT_PIPE_NAME]);
        let (stream, mimetype, kind) = classify_pipe(&inst).unwrap();
        assert_eq!(stream, "2");
        assert_eq!(mimetype, STDOUT_MIMETYPE);
        assert_eq!(kind, PipeKind::Stdout);
    }

    #[test]
    fn recognizes_ipmi_control_pipe_contract() {
        let inst = Instruction::new("pipe", ["7", IPMI_CONTROL_MIMETYPE, IPMI_CONTROL_PIPE_NAME]);
        let (stream, mimetype, kind) = classify_pipe(&inst).unwrap();
        assert_eq!(stream, "7");
        assert_eq!(mimetype, IPMI_CONTROL_MIMETYPE);
        assert_eq!(kind, PipeKind::IpmiControl);
    }

    #[test]
    fn ignores_non_stdout_pipe() {
        let inst = Instruction::new("pipe", ["4", "text/plain", "OTHER"]);
        let (_, _, kind) = classify_pipe(&inst).unwrap();
        assert_eq!(kind, PipeKind::Other);
    }

    #[test]
    fn parses_ipmi_state_messages() {
        let event = parse_ipmi_control_event(
            br#"{"type":"state","power":"on","identify":true,"health":"sol-connected","lastSel":"Fan 2 Failure"}"#,
        )
        .unwrap();
        assert_eq!(
            event,
            IpmiControlEvent::State(IpmiState {
                power: "on".into(),
                identify: Some(true),
                health: Some("sol-connected".into()),
                last_sel: Some("Fan 2 Failure".into()),
            })
        );
    }

    #[test]
    fn parses_ipmi_result_messages() {
        let event = parse_ipmi_control_event(
            br#"{"id":"gua-ipmi-1","type":"result","ok":true,"message":"Power cycle command sent"}"#,
        )
        .unwrap();
        assert_eq!(
            event,
            IpmiControlEvent::Result(IpmiCommandResult {
                id: "gua-ipmi-1".into(),
                ok: true,
                message: Some("Power cycle command sent".into()),
            })
        );
    }

    #[test]
    fn parses_ipmi_sel_messages() {
        let event = parse_ipmi_control_event(
            br#"{"type":"sel","total":1,"entries":[{"id":47,"time":"2025-07-22T01:08:33Z","severity":"crit","sensor":"Fan 2","event":"Redundancy Lost"}]}"#,
        )
        .unwrap();
        assert_eq!(
            event,
            IpmiControlEvent::Sel(IpmiSel {
                total: 1,
                entries: vec![IpmiSelEntry {
                    id: 47,
                    time: "2025-07-22T01:08:33Z".into(),
                    severity: "crit".into(),
                    sensor: "Fan 2".into(),
                    event: "Redundancy Lost".into(),
                }],
            })
        );
    }

    #[test]
    fn classifies_ipmi_command_confirmation_tiers() {
        assert_eq!(IpmiCommand::ReadSel.confirmation(), IpmiConfirmation::None);
        assert_eq!(
            IpmiCommand::PowerCycle.confirmation(),
            IpmiConfirmation::Simple
        );
        assert_eq!(
            IpmiCommand::HardReset.confirmation(),
            IpmiConfirmation::TypeToConfirm
        );
    }

    #[test]
    fn serializes_ipmi_command_message_contract() {
        let json = serde_json::to_value(IpmiCommandMessage {
            id: "gua-ipmi-9",
            message_type: "command",
            command: IpmiCommand::SendBreak.as_str(),
        })
        .unwrap();
        assert_eq!(json["id"], "gua-ipmi-9");
        assert_eq!(json["type"], "command");
        assert_eq!(json["command"], "send-break");
    }
}
