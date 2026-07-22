use gua_core::credentials::{CredentialStore, FileStore, Token};
use gua_rest::testing::MockGateway;
use gua_rest::{login_and_store, logout_stored, Client};
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;

#[tokio::test]
async fn login_success_stores_token_and_logout_deletes_it() {
    let gw = MockGateway::start().await;
    gw.stub_login("guacadmin", "postgresql").await;
    gw.stub_logout("TESTTOKEN0000000000000000000000000000000000000000000000000000000")
        .await;
    let base_url = gw.base_url();

    tokio::task::spawn_blocking(move || {
        let client = Client::new(&base_url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::with_dir(dir.path().join("tokens"));

        let auth = login_and_store(
            &client,
            &store,
            "dev",
            "guacadmin",
            &SecretString::new("secret".into()),
        )
        .unwrap();
        assert_eq!(auth.username, "guacadmin");
        assert_eq!(auth.data_source, "postgresql");

        let token = store.load_token("dev").unwrap().unwrap();
        assert_eq!(
            token.value.expose_secret(),
            "TESTTOKEN0000000000000000000000000000000000000000000000000000000"
        );

        assert!(logout_stored(&client, &store, "dev").unwrap());
        assert!(store.load_token("dev").unwrap().is_none());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn login_failure_is_auth_error_and_does_not_store_token() {
    let gw = MockGateway::start().await;
    gw.stub_login_failure().await;
    let base_url = gw.base_url();

    tokio::task::spawn_blocking(move || {
        let client = Client::new(&base_url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::with_dir(dir.path().join("tokens"));

        let err = login_and_store(
            &client,
            &store,
            "dev",
            "guacadmin",
            &SecretString::new("wrong".into()),
        )
        .unwrap_err();
        assert!(matches!(err, gua_core::Error::Auth(_)));
        assert!(store.load_token("dev").unwrap().is_none());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn authenticated_get_sends_header_not_query_token() {
    let gw = MockGateway::start().await;
    let token = "HEADERONLY";
    gw.stub_authenticated_get("postgresql", "connections", token, json!({"ok": true}))
        .await;
    let base_url = gw.base_url();

    tokio::task::spawn_blocking(move || {
        let client = Client::new(&base_url).unwrap();
        let body: serde_json::Value = client
            .get_json(
                &["api", "session", "data", "postgresql", "connections"],
                &Token::new(token),
            )
            .unwrap();
        assert_eq!(body, json!({"ok": true}));
    })
    .await
    .unwrap();
}
