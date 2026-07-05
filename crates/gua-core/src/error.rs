//! Crate-wide error type and process exit-code mapping (issue #4).
//!
//! Library crates return [`Error`]; the `gua` binary maps it to a stable exit
//! code via [`Error::exit_code`]. To keep this leaf crate dependency-light,
//! transport/HTTP failures are carried as messages (plus an optional boxed
//! source) rather than by depending on `reqwest`/`tungstenite` here.

use std::error::Error as StdError;

/// The workspace result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// A boxed, thread-safe source error.
pub type BoxError = Box<dyn StdError + Send + Sync + 'static>;

/// All errors surfaced by the `gua*` crates.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Authentication or authorization failure (bad credentials, expired token).
    #[error("authentication failed: {0}")]
    Auth(String),

    /// An HTTP-level failure talking to the gateway REST API.
    #[error("HTTP error: {0}")]
    Http(String),

    /// A Guacamole instruction-protocol violation or decode failure.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A tunnel/transport failure (WebSocket close, connection reset).
    #[error("transport error: {0}")]
    Transport(String),

    /// Invalid or unreadable configuration.
    #[error("configuration error: {0}")]
    Config(String),

    /// Credential store failure (keyring/file backend).
    #[error("credential store error: {0}")]
    Credential(String),

    /// A requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Invalid user input / arguments.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A command/feature that is scaffolded but not yet implemented.
    #[error("not implemented yet: {0}")]
    Unimplemented(String),

    /// Filesystem or other I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A wrapped error from a lower layer, with human context.
    #[error("{context}")]
    Other {
        /// Human-readable context.
        context: String,
        /// Underlying cause.
        #[source]
        source: BoxError,
    },
}

impl Error {
    /// Wrap an arbitrary error with human context.
    pub fn wrap(context: impl Into<String>, source: impl Into<BoxError>) -> Self {
        Error::Other {
            context: context.into(),
            source: source.into(),
        }
    }

    /// Stable process exit code for this error.
    ///
    /// Codes are grouped so scripts can branch on failure class:
    /// - `2`  invalid input / usage
    /// - `3`  configuration
    /// - `4`  authentication
    /// - `5`  not found
    /// - `6`  HTTP / REST
    /// - `7`  transport / tunnel
    /// - `8`  protocol
    /// - `9`  credential store
    /// - `64` not implemented yet
    /// - `1`  any other failure
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::InvalidInput(_) => 2,
            Error::Config(_) => 3,
            Error::Auth(_) => 4,
            Error::NotFound(_) => 5,
            Error::Http(_) => 6,
            Error::Transport(_) => 7,
            Error::Protocol(_) => 8,
            Error::Credential(_) => 9,
            Error::Unimplemented(_) => 64,
            Error::Io(_) | Error::Other { .. } => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_grouped_by_class() {
        assert_eq!(Error::InvalidInput("x".into()).exit_code(), 2);
        assert_eq!(Error::Auth("x".into()).exit_code(), 4);
        assert_eq!(Error::Protocol("x".into()).exit_code(), 8);
    }

    #[test]
    fn wrap_preserves_source() {
        let io = std::io::Error::other("boom");
        let e = Error::wrap("while doing thing", io);
        assert!(e.source().is_some());
        assert_eq!(e.exit_code(), 1);
    }
}
