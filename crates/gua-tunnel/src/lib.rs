//! Guacamole WebSocket tunnel transport.
//!
//! Opens guacamole-client's `/websocket-tunnel` endpoint and exchanges raw
//! Guacamole instruction-protocol messages. This is the same gateway-facing
//! path used by the browser client: connection selection/authentication is
//! carried in the tunnel URL (`GUAC_ID`, `GUAC_DATA_SOURCE`, token), so this
//! crate does not perform the direct-guacd `select`/`connect` handshake.

#![forbid(unsafe_code)]

use std::io::ErrorKind;
use std::time::Duration;

use gua_core::credentials::Token;
use gua_core::{Error, Result};
use gua_proto::{Decoder, Instruction};
use secrecy::ExposeSecret;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Error as WsError, Message, WebSocket};
use url::Url;

/// A live Guacamole tunnel over WebSocket.
pub struct Tunnel {
    socket: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    decoder: Decoder,
}

/// Parameters needed to open `/websocket-tunnel` through guacamole-client.
#[derive(Debug, Clone)]
pub struct TunnelParams<'a> {
    /// Base Guacamole webapp URL, e.g. `https://host/guacamole`.
    pub base_url: &'a str,
    /// Stored auth token.
    pub token: &'a Token,
    /// Auth data source, e.g. `postgresql`.
    pub data_source: &'a str,
    /// Connection identifier.
    pub connection_id: &'a str,
}

impl Tunnel {
    /// Open a connection tunnel (`GUAC_TYPE=c`) through guacamole-client.
    pub fn connect_connection(params: TunnelParams<'_>) -> Result<Self> {
        let url = websocket_tunnel_url(params)?;
        let (socket, _response) = connect(url.as_str())
            .map_err(|e| Error::Transport(format!("opening WebSocket tunnel: {e}")))?;
        Ok(Self {
            socket,
            decoder: Decoder::new(),
        })
    }

    /// Send one Guacamole instruction.
    pub fn send(&mut self, instruction: &Instruction) -> Result<()> {
        self.socket
            .send(Message::Text(instruction.encode()))
            .map_err(|e| Error::Transport(format!("sending tunnel instruction: {e}")))
    }

    /// Read the next complete Guacamole instruction, blocking until one is
    /// available or the tunnel closes.
    pub fn read_instruction(&mut self) -> Result<Instruction> {
        loop {
            if let Some(inst) = self.try_decode_next()? {
                return Ok(inst);
            }
            // A transient read (notably EINTR, which a terminal resize delivers
            // via SIGWINCH) carries no data and is not a disconnect: retry.
            self.read_one_message()?;
        }
    }

    /// Read the next complete instruction, returning `Ok(None)` if no message
    /// arrives before `timeout`.
    pub fn read_instruction_timeout(&mut self, timeout: Duration) -> Result<Option<Instruction>> {
        if let Some(inst) = self.try_decode_next()? {
            return Ok(Some(inst));
        }

        self.set_read_timeout(Some(timeout))?;
        let result = self.read_one_message();
        self.set_read_timeout(None)?;

        match result {
            Ok(ReadOutcome::Message) => self.try_decode_next(),
            Ok(ReadOutcome::Transient) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn try_decode_next(&mut self) -> Result<Option<Instruction>> {
        self.decoder
            .next_instruction()
            .map_err(|e| Error::Protocol(e.to_string()))
    }

    fn read_one_message(&mut self) -> Result<ReadOutcome> {
        let message = match self.socket.read() {
            Ok(message) => message,
            Err(WsError::Io(io)) if is_transient_read(&io) => {
                return Ok(ReadOutcome::Transient)
            }
            Err(e) => return Err(ws_read_error(e)),
        };
        match message {
            Message::Text(s) => self.decoder.push_str(&s),
            Message::Binary(b) => self.decoder.push_bytes(&b),
            Message::Ping(payload) => self
                .socket
                .send(Message::Pong(payload))
                .map_err(|e| Error::Transport(format!("sending pong: {e}")))?,
            Message::Pong(_) => {}
            Message::Close(frame) => {
                return Err(Error::Transport(format!(
                    "WebSocket tunnel closed{}",
                    frame
                        .as_ref()
                        .map(|f| format!(": {}", f.reason))
                        .unwrap_or_default()
                )))
            }
            Message::Frame(_) => {}
        }
        Ok(ReadOutcome::Message)
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        match self.socket.get_mut() {
            MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
            MaybeTlsStream::Rustls(stream) => stream.sock.set_read_timeout(timeout),
            _ => Ok(()),
        }
        .map_err(|e| Error::Transport(format!("configuring tunnel read timeout: {e}")))
    }

    /// Send an orderly disconnect and close the WebSocket.
    pub fn disconnect(mut self) -> Result<()> {
        let _ = self.send(&Instruction::bare("disconnect"));
        self.socket
            .close(None)
            .map_err(|e| Error::Transport(format!("closing WebSocket tunnel: {e}")))
    }
}

fn ws_read_error(error: WsError) -> Error {
    match error {
        WsError::Io(io) => Error::Transport(format!("reading tunnel message: {io}")),
        other => Error::Transport(format!("reading tunnel message: {other}")),
    }
}

/// Outcome of a single WebSocket read.
enum ReadOutcome {
    /// A message was read and handed to the decoder.
    Message,
    /// The read ended without data for a reason that is not a disconnect, and
    /// may simply be retried.
    Transient,
}

/// Whether a read error is transient — the read produced no data but the tunnel
/// is still healthy, so it can be retried or reported as "nothing yet".
///
/// `TimedOut`/`WouldBlock` are the expected outcome of a read deadline.
/// `Interrupted` (EINTR) is delivered whenever a signal lands during a blocking
/// read; SIGWINCH from a terminal resize is the common case, and treating that
/// as a transport failure would tear down a perfectly healthy session.
fn is_transient_read(io: &std::io::Error) -> bool {
    matches!(
        io.kind(),
        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
    )
}

fn websocket_tunnel_url(params: TunnelParams<'_>) -> Result<Url> {
    let mut url = Url::parse(params.base_url)
        .map_err(|e| Error::InvalidInput(format!("invalid Guacamole URL: {e}")))?;
    match url.scheme() {
        "http" => url
            .set_scheme("ws")
            .map_err(|_| Error::InvalidInput("cannot convert http URL to ws".into()))?,
        "https" => url
            .set_scheme("wss")
            .map_err(|_| Error::InvalidInput("cannot convert https URL to wss".into()))?,
        other => {
            return Err(Error::InvalidInput(format!(
                "Guacamole URL must use http or https, got {other:?}"
            )))
        }
    }
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| Error::InvalidInput("base URL cannot be a base".into()))?;
        path.pop_if_empty();
        path.push("websocket-tunnel");
    }
    url.query_pairs_mut()
        .clear()
        .append_pair("token", params.token.value.expose_secret())
        .append_pair("GUAC_DATA_SOURCE", params.data_source)
        .append_pair("GUAC_ID", params.connection_id)
        .append_pair("GUAC_TYPE", "c");
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_websocket_tunnel_url() {
        let token = Token::new("secret-token");
        let url = websocket_tunnel_url(TunnelParams {
            base_url: "https://gw.example/guacamole/",
            token: &token,
            data_source: "postgresql",
            connection_id: "42",
        })
        .unwrap();
        assert_eq!(url.scheme(), "wss");
        assert_eq!(url.path(), "/guacamole/websocket-tunnel");
        let q = url.query().unwrap();
        assert!(q.contains("token=secret-token"));
        assert!(q.contains("GUAC_DATA_SOURCE=postgresql"));
        assert!(q.contains("GUAC_ID=42"));
        assert!(q.contains("GUAC_TYPE=c"));
    }
}
