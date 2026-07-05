//! Configuration profiles / "contexts" (issue #6).
//!
//! A [`Config`] holds named [`Profile`]s (server URL, data source, default
//! output, TLS options). It loads from a TOML file in the OS config dir and is
//! overlaid by environment variables at resolution time:
//!
//! | Env var            | Overrides            |
//! |--------------------|----------------------|
//! | `GUA_PROFILE`      | active profile name  |
//! | `GUA_SERVER`       | `server`             |
//! | `GUA_DATA_SOURCE`  | `data_source`        |
//! | `GUA_OUTPUT`       | `output`             |
//! | `GUA_TLS_INSECURE` | `tls_insecure`       |
//! | `GUA_CONFIG`       | config file path     |

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::output::OutputFormat;

/// Default profile name used when none is configured or selected.
pub const DEFAULT_PROFILE: &str = "default";

/// Top-level configuration document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Name of the profile used when `--profile`/`GUA_PROFILE` are absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_profile: Option<String>,
    /// All configured profiles, keyed by name.
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// A single connection context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Base URL of the `guacamole-client` gateway, e.g. `https://host/guacamole`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Data source / auth backend identifier, e.g. `postgresql`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    /// Default output format for this profile.
    #[serde(default)]
    pub output: OutputFormat,
    /// Skip TLS certificate verification (dangerous; dev only).
    #[serde(default)]
    pub tls_insecure: bool,
}

impl Config {
    /// Resolve the config file path, honoring the `GUA_CONFIG` override.
    pub fn default_path() -> crate::Result<PathBuf> {
        if let Ok(p) = std::env::var("GUA_CONFIG") {
            if !p.is_empty() {
                return Ok(PathBuf::from(p));
            }
        }
        let dirs = ProjectDirs::from("io.github", "ciroiriarte", "guacamole-cli")
            .ok_or_else(|| Error::Config("cannot determine OS config directory".into()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load configuration from the default path (empty config if absent).
    pub fn load() -> crate::Result<Self> {
        Self::load_from(&Self::default_path()?)
    }

    /// Load configuration from a specific path (empty config if absent).
    pub fn load_from(path: &Path) -> crate::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s)
                .map_err(|e| Error::Config(format!("parsing {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Config(format!("reading {}: {e}", path.display()))),
        }
    }

    /// Persist configuration to the default path, creating parent dirs.
    pub fn save(&self) -> crate::Result<()> {
        self.save_to(&Self::default_path()?)
    }

    /// Persist configuration to a specific path, creating parent dirs.
    pub fn save_to(&self, path: &Path) -> crate::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Config(format!("creating {}: {e}", parent.display())))?;
        }
        let s = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("serializing config: {e}")))?;
        std::fs::write(path, s)
            .map_err(|e| Error::Config(format!("writing {}: {e}", path.display())))
    }

    /// Name of the active profile: `cli_override` → `GUA_PROFILE` →
    /// `current_profile` → [`DEFAULT_PROFILE`].
    pub fn active_profile_name(&self, cli_override: Option<&str>) -> String {
        cli_override
            .map(str::to_owned)
            .or_else(|| std::env::var("GUA_PROFILE").ok().filter(|s| !s.is_empty()))
            .or_else(|| self.current_profile.clone())
            .unwrap_or_else(|| DEFAULT_PROFILE.to_string())
    }

    /// The active profile with environment-variable overrides applied.
    pub fn effective_profile(&self, cli_override: Option<&str>) -> crate::Result<Profile> {
        let name = self.active_profile_name(cli_override);
        let mut profile = self.profiles.get(&name).cloned().unwrap_or_default();
        apply_env_overrides(&mut profile)?;
        Ok(profile)
    }

    /// Get or create a mutable profile by name.
    pub fn profile_mut(&mut self, name: &str) -> &mut Profile {
        self.profiles.entry(name.to_string()).or_default()
    }

    /// Set a single `key=value` field on a named profile (used by `gua config set`).
    ///
    /// Recognized keys: `server`, `data_source`, `output`, `tls_insecure`.
    pub fn set_field(&mut self, profile: &str, key: &str, value: &str) -> crate::Result<()> {
        let p = self.profile_mut(profile);
        match key {
            "server" => p.server = Some(value.to_string()),
            "data_source" => p.data_source = Some(value.to_string()),
            "output" => p.output = value.parse()?,
            "tls_insecure" => {
                p.tls_insecure = value.parse().map_err(|_| {
                    Error::InvalidInput(format!("tls_insecure must be true|false, got {value:?}"))
                })?
            }
            other => {
                return Err(Error::InvalidInput(format!(
                    "unknown config key {other:?} (expected server|data_source|output|tls_insecure)"
                )))
            }
        }
        Ok(())
    }

    /// Read a single field from a named profile as a string (used by `gua config get`).
    pub fn get_field(&self, profile: &str, key: &str) -> crate::Result<Option<String>> {
        let Some(p) = self.profiles.get(profile) else {
            return Err(Error::NotFound(format!("profile {profile:?}")));
        };
        let v = match key {
            "server" => p.server.clone(),
            "data_source" => p.data_source.clone(),
            "output" => Some(p.output.to_string()),
            "tls_insecure" => Some(p.tls_insecure.to_string()),
            other => {
                return Err(Error::InvalidInput(format!(
                    "unknown config key {other:?} (expected server|data_source|output|tls_insecure)"
                )))
            }
        };
        Ok(v)
    }
}

/// Overlay `GUA_*` environment variables onto a profile.
fn apply_env_overrides(profile: &mut Profile) -> crate::Result<()> {
    if let Ok(v) = std::env::var("GUA_SERVER") {
        if !v.is_empty() {
            profile.server = Some(v);
        }
    }
    if let Ok(v) = std::env::var("GUA_DATA_SOURCE") {
        if !v.is_empty() {
            profile.data_source = Some(v);
        }
    }
    if let Ok(v) = std::env::var("GUA_OUTPUT") {
        if !v.is_empty() {
            profile.output = v.parse()?;
        }
    }
    if let Ok(v) = std::env::var("GUA_TLS_INSECURE") {
        if !v.is_empty() {
            profile.tls_insecure = v.parse().map_err(|_| {
                Error::InvalidInput(format!("GUA_TLS_INSECURE must be true|false, got {v:?}"))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");

        let mut cfg = Config {
            current_profile: Some("prod".into()),
            ..Default::default()
        };
        cfg.set_field("prod", "server", "https://gw/guacamole")
            .unwrap();
        cfg.set_field("prod", "data_source", "postgresql").unwrap();
        cfg.set_field("prod", "output", "json").unwrap();
        cfg.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.current_profile.as_deref(), Some("prod"));
        let p = loaded.profiles.get("prod").unwrap();
        assert_eq!(p.server.as_deref(), Some("https://gw/guacamole"));
        assert_eq!(p.output, OutputFormat::Json);
    }

    #[test]
    fn missing_file_is_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load_from(&dir.path().join("nope.toml")).unwrap();
        assert!(cfg.profiles.is_empty());
    }

    #[test]
    fn active_profile_precedence() {
        let cfg = Config {
            current_profile: Some("stored".into()),
            ..Default::default()
        };
        // cli override wins over stored current_profile
        assert_eq!(cfg.active_profile_name(Some("cli")), "cli");
        // stored current_profile used when no override
        assert_eq!(cfg.active_profile_name(None), "stored");
    }

    #[test]
    fn rejects_unknown_keys_and_formats() {
        let mut cfg = Config::default();
        assert!(cfg.set_field("p", "bogus", "x").is_err());
        assert!(cfg.set_field("p", "output", "xml").is_err());
        assert!(cfg.set_field("p", "tls_insecure", "maybe").is_err());
    }
}
