//! Tracing setup and secret redaction (issue #5).

use std::fmt;

use tracing_subscriber::{fmt as tfmt, EnvFilter};

/// Initialize the global tracing subscriber.
///
/// `verbosity` is the net `-v` count minus `-q` count from the CLI:
/// `<0` → errors only, `0` → warn, `1` → info, `2` → debug, `>=3` → trace.
/// A `RUST_LOG` environment value, if present, overrides this mapping.
pub fn init(verbosity: i8, json: bool) {
    let default_level = match verbosity {
        i8::MIN..=-1 => "error",
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!("gua={default_level},gua_core={default_level}"))
    });

    let builder = tfmt::Subscriber::builder()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);

    // `try_init` so repeated calls (e.g. in tests) don't panic.
    if json {
        let _ = builder.json().try_init();
    } else {
        let _ = builder.try_init();
    }
}

/// Wrapper whose `Display`/`Debug` never reveal the wrapped secret.
///
/// Use in log/format calls so tokens and passwords cannot leak:
/// `tracing::info!(token = %Redacted::new(&tok), "authenticated")`.
pub struct Redacted<'a>(&'a str);

impl<'a> Redacted<'a> {
    /// Wrap a secret string slice.
    pub fn new(secret: &'a str) -> Self {
        Redacted(secret)
    }
}

impl fmt::Display for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redact(self.0))
    }
}

impl fmt::Debug for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", redact(self.0))
    }
}

/// Redact a secret to a fixed marker, keeping only a short prefix hint for
/// long values so logs remain correlatable without exposing the secret.
pub fn redact(secret: &str) -> String {
    match secret.chars().count() {
        0 => "<empty>".to_string(),
        1..=8 => "***".to_string(),
        _ => {
            let prefix: String = secret.chars().take(3).collect();
            format!("{prefix}***")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_secrets_fully_hidden() {
        assert_eq!(redact("hunter2"), "***");
        assert_eq!(redact(""), "<empty>");
    }

    #[test]
    fn long_secrets_keep_only_prefix() {
        let token = "ABCDEF0123456789ABCDEF";
        let out = redact(token);
        assert_eq!(out, "ABC***");
        assert!(!out.contains("456789"));
        // The Display wrapper must not leak either.
        assert_eq!(format!("{}", Redacted::new(token)), "ABC***");
    }
}
