//! Reusable mock Guacamole gateway for tests (issue #9).
//!
//! Wraps a [`wiremock`] server and offers helpers to stub the endpoints the
//! REST client talks to, so the whole workspace tests against one fixture shape.
//!
//! ```no_run
//! # async fn demo() {
//! use gua_rest::testing::MockGateway;
//! let gw = MockGateway::start().await;
//! gw.stub_login("guacadmin", "postgresql").await;
//! let base = gw.base_url(); // point the client at this
//! # let _ = base;
//! # }
//! ```

use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A running mock gateway. Drop it to shut the server down.
#[derive(Debug)]
pub struct MockGateway {
    server: MockServer,
}

impl MockGateway {
    /// Start a fresh mock gateway on an ephemeral port.
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// Base URL (e.g. `http://127.0.0.1:PORT`) to point a client at.
    pub fn base_url(&self) -> String {
        self.server.uri()
    }

    /// Borrow the underlying server for advanced/custom stubbing.
    pub fn server(&self) -> &MockServer {
        &self.server
    }

    /// Stub `POST /api/tokens` to return a canonical successful login body.
    ///
    /// Mirrors the real gateway response shape:
    /// `{ authToken, username, dataSource, availableDataSources }`.
    pub async fn stub_login(&self, username: &str, data_source: &str) {
        let body = json!({
            "authToken": "TESTTOKEN0000000000000000000000000000000000000000000000000000000",
            "username": username,
            "dataSource": data_source,
            "availableDataSources": [data_source],
        });
        Mock::given(method("POST"))
            .and(path("/api/tokens"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `POST /api/tokens` to reject credentials.
    pub async fn stub_login_failure(&self) {
        Mock::given(method("POST"))
            .and(path("/api/tokens"))
            .respond_with(ResponseTemplate::new(403).set_body_string("invalid login"))
            .mount(&self.server)
            .await;
    }

    /// Stub an authenticated management GET that requires the Guacamole-Token header.
    pub async fn stub_authenticated_get(
        &self,
        data_source: &str,
        resource: &str,
        token: &str,
        body: serde_json::Value,
    ) {
        let p = format!("/api/session/data/{data_source}/{resource}");
        Mock::given(method("GET"))
            .and(path(p))
            .and(header("Guacamole-Token", token))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `DELETE /api/tokens/{token}` to return `204 No Content`.
    pub async fn stub_logout(&self, token: &str) {
        Mock::given(method("DELETE"))
            .and(path(format!("/api/tokens/{token}")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&self.server)
            .await;
    }

    /// Stub `GET /api/session/data/{dataSource}/connections`.
    pub async fn stub_connections(&self, data_source: &str, body: serde_json::Value) {
        let p = format!("/api/session/data/{data_source}/connections");
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `GET /api/session/data/{dataSource}/connections/{id}`.
    pub async fn stub_connection(&self, data_source: &str, id: &str, body: serde_json::Value) {
        let p = format!("/api/session/data/{data_source}/connections/{id}");
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub `GET /api/session/data/{dataSource}/connections/{id}/parameters`.
    pub async fn stub_connection_parameters(
        &self,
        data_source: &str,
        id: &str,
        body: serde_json::Value,
    ) {
        let p = format!("/api/session/data/{data_source}/connections/{id}/parameters");
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Stub an arbitrary management GET under
    /// `/api/session/data/{dataSource}/{resource}` returning `body`.
    pub async fn stub_data_get(&self, data_source: &str, resource: &str, body: serde_json::Value) {
        let p = format!("/api/session/data/{data_source}/{resource}");
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }
}
