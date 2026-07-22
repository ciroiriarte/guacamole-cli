use std::process::Command;

use gua_core::credentials::{CredentialStore, FileStore, Token};
use gua_rest::testing::MockGateway;
use serde_json::json;

fn gua() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gua"))
}

fn write_config(dir: &tempfile::TempDir, server: &str) -> std::path::PathBuf {
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"current_profile = "dev"

[profiles.dev]
server = "{server}"
data_source = "postgresql"
"#
        ),
    )
    .unwrap();
    path
}

fn store_token(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let token_dir = dir.path().join("tokens");
    let store = FileStore::with_dir(token_dir.clone());
    store.store_token("dev", &Token::new("TESTTOKEN")).unwrap();
    token_dir
}

#[tokio::test]
async fn connection_list_renders_table_against_mock_gateway() {
    let gw = MockGateway::start().await;
    gw.stub_authenticated_get(
        "postgresql",
        "connections",
        "TESTTOKEN",
        json!({
            "2": {"name": "db", "protocol": "ssh", "parentIdentifier": "ROOT"},
            "1": {"name": "web", "protocol": "rdp", "parentIdentifier": "ROOT", "activeConnections": 1}
        }),
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let config = write_config(&dir, &gw.base_url());
    let token_dir = store_token(&dir);

    let output = gua()
        .env("GUA_CONFIG", &config)
        .env("GUA_TOKEN_STORE_DIR", &token_dir)
        .args(["connection", "list"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("NAME"));
    assert!(stdout.contains("web"));
    assert!(stdout.contains("rdp"));
    assert!(stdout.contains("db"));
}

#[tokio::test]
async fn connection_get_renders_json_with_redacted_parameters() {
    let gw = MockGateway::start().await;
    gw.stub_connection(
        "postgresql",
        "1",
        json!({"identifier": "1", "name": "web", "protocol": "ssh", "parentIdentifier": "ROOT"}),
    )
    .await;
    gw.stub_connection_parameters(
        "postgresql",
        "1",
        json!({"hostname": "web01", "username": "alice", "password": "secret"}),
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let config = write_config(&dir, &gw.base_url());
    let token_dir = store_token(&dir);

    let output = gua()
        .env("GUA_CONFIG", &config)
        .env("GUA_TOKEN_STORE_DIR", &token_dir)
        .args(["--output", "json", "connection", "get", "1"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"name\": \"web\""));
    assert!(stdout.contains("\"hostname\": \"web01\""));
    assert!(stdout.contains("\"password\": \"***\""));
    assert!(!stdout.contains("secret"));
}
