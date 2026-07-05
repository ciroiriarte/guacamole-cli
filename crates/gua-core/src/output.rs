//! Output rendering: table / json / yaml (issue #8).

use std::fmt;
use std::str::FromStr;

use comfy_table::Table;
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Selected output representation for command results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Human-friendly aligned table (default).
    #[default]
    Table,
    /// Compact machine-readable JSON.
    Json,
    /// YAML.
    Yaml,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            OutputFormat::Table => "table",
            OutputFormat::Json => "json",
            OutputFormat::Yaml => "yaml",
        };
        f.write_str(s)
    }
}

impl FromStr for OutputFormat {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "table" => Ok(OutputFormat::Table),
            "json" => Ok(OutputFormat::Json),
            "yaml" | "yml" => Ok(OutputFormat::Yaml),
            other => Err(Error::InvalidInput(format!(
                "unknown output format {other:?} (expected table|json|yaml)"
            ))),
        }
    }
}

/// Types that can render as a row in table output.
///
/// `headers` and `row` must return columns in the same order.
pub trait Tabular {
    /// Column headers.
    fn headers() -> Vec<&'static str>;
    /// This item's cell values, aligned with [`Tabular::headers`].
    fn row(&self) -> Vec<String>;
}

/// Render a slice of items in the requested format to a `String`.
///
/// `Table` uses [`Tabular`]; `Json`/`Yaml` use `serde`.
pub fn render<T>(items: &[T], format: OutputFormat) -> crate::Result<String>
where
    T: Serialize + Tabular,
{
    match format {
        OutputFormat::Json => {
            serde_json::to_string_pretty(items).map_err(|e| Error::wrap("serializing JSON", e))
        }
        OutputFormat::Yaml => {
            serde_yaml_ng::to_string(items).map_err(|e| Error::wrap("serializing YAML", e))
        }
        OutputFormat::Table => {
            let mut table = Table::new();
            table.set_header(T::headers());
            for item in items {
                table.add_row(item.row());
            }
            Ok(table.to_string())
        }
    }
}

/// Render and print items to stdout in the requested format.
pub fn print<T>(items: &[T], format: OutputFormat) -> crate::Result<()>
where
    T: Serialize + Tabular,
{
    println!("{}", render(items, format)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Conn {
        name: String,
        protocol: String,
    }

    impl Tabular for Conn {
        fn headers() -> Vec<&'static str> {
            vec!["NAME", "PROTOCOL"]
        }
        fn row(&self) -> Vec<String> {
            vec![self.name.clone(), self.protocol.clone()]
        }
    }

    fn sample() -> Vec<Conn> {
        vec![Conn {
            name: "web01".into(),
            protocol: "ssh".into(),
        }]
    }

    #[test]
    fn parses_formats_case_insensitively() {
        assert_eq!("JSON".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("yml".parse::<OutputFormat>().unwrap(), OutputFormat::Yaml);
        assert!("xml".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn renders_each_format() {
        let json = render(&sample(), OutputFormat::Json).unwrap();
        assert!(json.contains("\"protocol\": \"ssh\""));

        let yaml = render(&sample(), OutputFormat::Yaml).unwrap();
        assert!(yaml.contains("protocol: ssh"));

        let table = render(&sample(), OutputFormat::Table).unwrap();
        assert!(table.contains("PROTOCOL"));
        assert!(table.contains("web01"));
    }
}
