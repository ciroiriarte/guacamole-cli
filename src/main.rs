//! `gua` — native command-line / native client for Apache Guacamole.

mod cli;

use std::collections::BTreeMap;
use std::io::{self, Write};

use clap::Parser;
use secrecy::SecretString;

use gua_core::config::{Config, DEFAULT_PROFILE};
use gua_core::credentials::default_store;
use gua_core::output::{OutputFormat, Tabular};
use gua_core::{Error, Result};
use gua_rest::{Client, Connection, ConnectionDetail};
use serde::Serialize;

use cli::{Cli, Command, ConfigCmd, ConnectArgs, ConnectionCmd, LoginArgs, RecordCmd, SessionCmd};

fn main() {
    let cli = Cli::parse();
    gua_core::logging::init(cli.verbosity(), cli.log_json);

    if let Err(err) = run(&cli) {
        eprintln!("error: {err}");
        std::process::exit(err.exit_code());
    }
}

fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Config(cmd) => run_config(cli, cmd),
        Command::Login(args) => run_login(cli, args),
        Command::Logout => run_logout(cli),

        // The following belong to later roadmap phases. They are wired into the
        // CLI now (issue #8) but not yet implemented.
        Command::Connection(c) => match c {
            ConnectionCmd::List => run_connection_list(cli),
            ConnectionCmd::Get { id } => run_connection_get(cli, id),
            ConnectionCmd::Create => unimplemented("connection create", 13),
            ConnectionCmd::Update { .. } => unimplemented("connection update", 13),
            ConnectionCmd::Delete { .. } => unimplemented("connection delete", 13),
            ConnectionCmd::Share { .. } => unimplemented("connection share", 17),
        },
        Command::Session(c) => match c {
            SessionCmd::List => unimplemented("session list", 18),
            SessionCmd::Kill { .. } => unimplemented("session kill", 18),
        },
        Command::Connect(args) => run_connect(cli, args),
        Command::Record(c) => match c {
            RecordCmd::Get { .. } => unimplemented("record get", 41),
            RecordCmd::Play { .. } => unimplemented("record play", 42),
        },
    }
}

/// Resolve which profile `config get/set` should act on.
fn target_profile(cli: &Cli, cfg: &Config) -> String {
    cli.profile
        .clone()
        .or_else(|| cfg.current_profile.clone())
        .unwrap_or_else(|| DEFAULT_PROFILE.to_string())
}

#[derive(Debug, Serialize)]
struct ConnectionRow {
    id: String,
    name: String,
    protocol: String,
    parent: String,
    active: String,
}

impl From<Connection> for ConnectionRow {
    fn from(c: Connection) -> Self {
        Self {
            id: c.identifier,
            name: c.name,
            protocol: c.protocol,
            parent: c.parent_identifier.unwrap_or_else(|| "-".to_string()),
            active: c
                .active_connections
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
        }
    }
}

impl Tabular for ConnectionRow {
    fn headers() -> Vec<&'static str> {
        vec!["ID", "NAME", "PROTOCOL", "PARENT", "ACTIVE"]
    }

    fn row(&self) -> Vec<String> {
        vec![
            self.id.clone(),
            self.name.clone(),
            self.protocol.clone(),
            self.parent.clone(),
            self.active.clone(),
        ]
    }
}

#[derive(Debug, Serialize)]
struct ConnectionDetailRow {
    id: String,
    name: String,
    protocol: String,
    parent: String,
    active: String,
    parameters: BTreeMap<String, String>,
}

impl From<ConnectionDetail> for ConnectionDetailRow {
    fn from(d: ConnectionDetail) -> Self {
        let base = ConnectionRow::from(d.connection);
        let parameters = d
            .parameters
            .into_iter()
            .map(|(k, v)| {
                let value = if is_sensitive_parameter(&k) {
                    "***".to_string()
                } else {
                    v
                };
                (k, value)
            })
            .collect();
        Self {
            id: base.id,
            name: base.name,
            protocol: base.protocol,
            parent: base.parent,
            active: base.active,
            parameters,
        }
    }
}

impl Tabular for ConnectionDetailRow {
    fn headers() -> Vec<&'static str> {
        vec!["ID", "NAME", "PROTOCOL", "PARENT", "ACTIVE", "PARAMETERS"]
    }

    fn row(&self) -> Vec<String> {
        let parameters = self
            .parameters
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        vec![
            self.id.clone(),
            self.name.clone(),
            self.protocol.clone(),
            self.parent.clone(),
            self.active.clone(),
            parameters,
        ]
    }
}

fn active_profile(cli: &Cli, cfg: &Config) -> String {
    cfg.active_profile_name(cli.profile.as_deref())
}

fn rest_client(cli: &Cli, cfg: &Config) -> Result<Client> {
    let mut profile = cfg.effective_profile(cli.profile.as_deref())?;
    if let Some(server) = &cli.server {
        profile.server = Some(server.clone());
    }
    let server = profile.server.ok_or_else(|| {
        Error::Config("server is not configured (set `gua config set server https://host/guacamole` or pass --server)".into())
    })?;
    Client::builder(&server)?
        .tls_insecure(profile.tls_insecure)
        .build()
}

fn output_format(cli: &Cli, cfg: &Config) -> Result<OutputFormat> {
    if let Some(output) = cli.output {
        return Ok(output.into());
    }
    Ok(cfg.effective_profile(cli.profile.as_deref())?.output)
}

fn data_source(cli: &Cli, cfg: &Config) -> Result<String> {
    cfg.effective_profile(cli.profile.as_deref())?
        .data_source
        .ok_or_else(|| {
            Error::Config(
                "data_source is not configured (run `gua login` or `gua config set data_source <name>`)"
                    .into(),
            )
        })
}

fn stored_token(cli: &Cli, cfg: &Config) -> Result<gua_core::credentials::Token> {
    let profile_name = active_profile(cli, cfg);
    let store = default_store()?;
    store.load_token(&profile_name)?.ok_or_else(|| {
        Error::Auth(format!(
            "no stored token for profile {profile_name}; run `gua login` first"
        ))
    })
}

fn run_connection_list(cli: &Cli) -> Result<()> {
    let cfg = Config::load()?;
    let client = rest_client(cli, &cfg)?;
    let data_source = data_source(cli, &cfg)?;
    let token = stored_token(cli, &cfg)?;
    let rows = client
        .list_connections(&data_source, &token)?
        .into_iter()
        .map(ConnectionRow::from)
        .collect::<Vec<_>>();
    gua_core::output::print(&rows, output_format(cli, &cfg)?)
}

fn run_connection_get(cli: &Cli, id: &str) -> Result<()> {
    let cfg = Config::load()?;
    let client = rest_client(cli, &cfg)?;
    let data_source = data_source(cli, &cfg)?;
    let token = stored_token(cli, &cfg)?;
    let row = ConnectionDetailRow::from(client.get_connection(&data_source, id, &token)?);
    gua_core::output::print(&[row], output_format(cli, &cfg)?)
}

#[cfg(feature = "tui")]
fn run_connect(cli: &Cli, args: &ConnectArgs) -> Result<()> {
    if args.mount.is_some() {
        return unimplemented("connect --mount", 39);
    }

    let cfg = Config::load()?;
    let mut profile = cfg.effective_profile(cli.profile.as_deref())?;
    if let Some(server) = &cli.server {
        profile.server = Some(server.clone());
    }
    let server = profile.server.ok_or_else(|| {
        Error::Config("server is not configured (set `gua config set server https://host/guacamole` or pass --server)".into())
    })?;
    let data_source = profile.data_source.ok_or_else(|| {
        Error::Config(
            "data_source is not configured (run `gua login` or `gua config set data_source <name>`)"
                .into(),
        )
    })?;
    let token = stored_token(cli, &cfg)?;

    let mut session = gua_session::Session::connect(gua_tunnel::TunnelParams {
        base_url: &server,
        token: &token,
        data_source: &data_source,
        connection_id: &args.id,
    })?;

    let run_result = gua_tui::run_text_session_with_options(
        &mut session,
        gua_tui::TextSessionOptions {
            ipmi_control: args.ipmi_control,
            chrome: args.chrome && !args.raw,
        },
    );
    let disconnect_result = session.disconnect();
    run_result.and(disconnect_result)
}

#[cfg(not(feature = "tui"))]
fn run_connect(_cli: &Cli, _args: &ConnectArgs) -> Result<()> {
    Err(Error::Unimplemented(
        "`gua connect` requires the default `tui` feature; rebuild without `--no-default-features`"
            .into(),
    ))
}

fn is_sensitive_parameter(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("password") || lower.contains("passphrase") || lower.contains("private-key")
}

fn run_login(cli: &Cli, args: &LoginArgs) -> Result<()> {
    let mut cfg = Config::load()?;
    let profile_name = active_profile(cli, &cfg);
    let client = rest_client(cli, &cfg)?;
    let username = args.username.clone().ok_or_else(|| {
        Error::InvalidInput("username is required (use --username or GUA_USERNAME)".into())
    })?;
    let password = SecretString::new(match &args.password {
        Some(p) => p.clone(),
        None => read_password_from_stdin()?,
    });

    let store = default_store()?;
    let auth =
        gua_rest::login_and_store(&client, store.as_ref(), &profile_name, &username, &password)?;

    // Remember the selected data source so later management commands can reuse it.
    let p = cfg.profile_mut(&profile_name);
    if p.data_source.is_none() {
        p.data_source = Some(auth.data_source.clone());
    }
    if cfg.current_profile.is_none() {
        cfg.current_profile = Some(profile_name.clone());
    }
    cfg.save()?;

    println!(
        "authenticated as {} on {} (profile {})",
        auth.username, auth.data_source, profile_name
    );
    Ok(())
}

fn run_logout(cli: &Cli) -> Result<()> {
    let cfg = Config::load()?;
    let profile_name = active_profile(cli, &cfg);
    let client = rest_client(cli, &cfg)?;
    let store = default_store()?;
    if gua_rest::logout_stored(&client, store.as_ref(), &profile_name)? {
        println!("logged out profile {profile_name}");
    } else {
        println!("no stored token for profile {profile_name}");
    }
    Ok(())
}

fn read_password_from_stdin() -> Result<String> {
    eprint!("Password: ");
    io::stderr().flush()?;
    let mut password = String::new();
    io::stdin().read_line(&mut password)?;
    Ok(password.trim_end_matches(['\r', '\n']).to_string())
}

fn run_config(cli: &Cli, cmd: &ConfigCmd) -> Result<()> {
    match cmd {
        ConfigCmd::Path => {
            println!("{}", Config::default_path()?.display());
            Ok(())
        }
        ConfigCmd::List => {
            let cfg = Config::load()?;
            let active = cfg.active_profile_name(cli.profile.as_deref());
            println!("current profile: {active}");
            if cfg.profiles.is_empty() {
                println!("(no profiles configured)");
            }
            for (name, p) in &cfg.profiles {
                let marker = if *name == active { "*" } else { " " };
                println!(
                    "{marker} {name}: server={} data_source={} output={} tls_insecure={}",
                    p.server.as_deref().unwrap_or("-"),
                    p.data_source.as_deref().unwrap_or("-"),
                    p.output,
                    p.tls_insecure,
                );
            }
            Ok(())
        }
        ConfigCmd::Get { key } => {
            let cfg = Config::load()?;
            let profile = target_profile(cli, &cfg);
            match cfg.get_field(&profile, key)? {
                Some(v) => println!("{v}"),
                None => println!("(unset)"),
            }
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg = Config::load()?;
            let profile = target_profile(cli, &cfg);
            cfg.set_field(&profile, key, value)?;
            // First profile written becomes the current one.
            if cfg.current_profile.is_none() {
                cfg.current_profile = Some(profile.clone());
            }
            cfg.save()?;
            println!("set {key}={value} on profile {profile:?}");
            Ok(())
        }
    }
}

fn unimplemented(what: &str, issue: u32) -> Result<()> {
    Err(Error::Unimplemented(format!(
        "`gua {what}` (tracked as roadmap issue #{issue})"
    )))
}
