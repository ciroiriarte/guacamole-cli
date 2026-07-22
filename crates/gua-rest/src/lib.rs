//! Apache Guacamole REST management-API client.
//!
//! Wraps the gateway's management plane under `/api/session/data/{dataSource}/`
//! (connections, groups, users, permissions, sharing profiles, active sessions,
//! history) plus `/api/tokens` authentication.

#![forbid(unsafe_code)]

use gua_core::credentials::{CredentialStore, Token};
use gua_core::{Error, Result};
use secrecy::{ExposeSecret, SecretString};
use std::collections::BTreeMap;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use url::Url;

#[cfg(feature = "test-support")]
pub mod testing;

/// Successful `/api/tokens` response from Guacamole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    /// Opaque bearer token for subsequent REST calls.
    pub auth_token: String,
    /// Authenticated username.
    pub username: String,
    /// Selected data source / auth backend.
    pub data_source: String,
    /// Data sources available to the user.
    #[serde(default)]
    pub available_data_sources: Vec<String>,
}

/// A Guacamole connection summary/detail object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    /// Connection identifier.
    #[serde(default)]
    pub identifier: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Parent connection-group identifier.
    #[serde(default)]
    pub parent_identifier: Option<String>,
    /// Protocol name, e.g. `ssh`, `rdp`, `vnc`.
    #[serde(default)]
    pub protocol: String,
    /// Number of active connections if returned by the gateway.
    #[serde(default)]
    pub active_connections: Option<u64>,
    /// Arbitrary Guacamole attributes.
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

/// A connection plus its parameter map from `/parameters`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionDetail {
    /// Connection metadata.
    #[serde(flatten)]
    pub connection: Connection,
    /// Connection parameters.
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

/// Blocking Guacamole REST client.
#[derive(Debug, Clone)]
pub struct Client {
    base_url: Url,
    http: reqwest::blocking::Client,
}

impl Client {
    /// Build a client rooted at the Guacamole servlet context, e.g.
    /// `https://host/guacamole` (not `.../api`).
    pub fn new(base_url: &str) -> Result<Self> {
        Self::builder(base_url)?.build()
    }

    /// Start configuring a client.
    pub fn builder(base_url: &str) -> Result<ClientBuilder> {
        let base_url = normalize_base_url(base_url)?;
        Ok(ClientBuilder {
            base_url,
            tls_insecure: false,
        })
    }

    /// Log in with username/password via `POST /api/tokens`.
    pub fn login(&self, username: &str, password: &SecretString) -> Result<AuthResponse> {
        let url = self.url(&["api", "tokens"])?;
        let params = [
            ("username", username),
            ("password", password.expose_secret().as_str()),
        ];
        let resp = self
            .http
            .post(url)
            .form(&params)
            .send()
            .map_err(|e| Error::Http(format!("POST /api/tokens: {e}")))?;
        decode_response(resp, "POST /api/tokens")
    }

    /// Log out by invalidating the token via `DELETE /api/tokens/{token}`.
    pub fn logout(&self, token: &Token) -> Result<()> {
        let url = self.url(&["api", "tokens", token.value.expose_secret()])?;
        let resp = self
            .http
            .delete(url)
            .send()
            .map_err(|e| Error::Http(format!("DELETE /api/tokens/<token>: {e}")))?;
        expect_empty_success(resp, "DELETE /api/tokens/<token>")
    }

    /// List connections in `data_source`.
    pub fn list_connections(&self, data_source: &str, token: &Token) -> Result<Vec<Connection>> {
        let path = ["api", "session", "data", data_source, "connections"];
        let value: serde_json::Value = self.get_json(&path, token)?;
        parse_connections(value)
    }

    /// Fetch one connection and its parameters.
    pub fn get_connection(
        &self,
        data_source: &str,
        id: &str,
        token: &Token,
    ) -> Result<ConnectionDetail> {
        let connection_path = ["api", "session", "data", data_source, "connections", id];
        let parameters_path = [
            "api",
            "session",
            "data",
            data_source,
            "connections",
            id,
            "parameters",
        ];
        let mut connection: Connection = self.get_json(&connection_path, token)?;
        if connection.identifier.is_empty() {
            connection.identifier = id.to_string();
        }
        let parameters = self.get_json(&parameters_path, token)?;
        Ok(ConnectionDetail {
            connection,
            parameters,
        })
    }

    /// Perform an authenticated JSON GET, sending the Guacamole token in the
    /// `Guacamole-Token` header rather than in the URL.
    pub fn get_json<T>(&self, path_segments: &[&str], token: &Token) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = self.url(path_segments)?;
        let resp = self
            .http
            .get(url)
            .header("Guacamole-Token", token.value.expose_secret())
            .send()
            .map_err(|e| Error::Http(format!("GET /{}: {e}", path_segments.join("/"))))?;
        decode_response(resp, &format!("GET /{}", path_segments.join("/")))
    }

    fn url(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| Error::InvalidInput("base URL cannot be a base".into()))?;
            path.pop_if_empty();
            path.extend(segments);
        }
        Ok(url)
    }
}

/// Builder for [`Client`].
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    base_url: Url,
    tls_insecure: bool,
}

impl ClientBuilder {
    /// Disable TLS certificate verification. Intended for development only.
    pub fn tls_insecure(mut self, yes: bool) -> Self {
        self.tls_insecure = yes;
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<Client> {
        let http = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(self.tls_insecure)
            .build()
            .map_err(|e| Error::Http(format!("building HTTP client: {e}")))?;
        Ok(Client {
            base_url: self.base_url,
            http,
        })
    }
}

/// Log in and persist the returned `authToken` in `store`.
pub fn login_and_store(
    client: &Client,
    store: &dyn CredentialStore,
    profile: &str,
    username: &str,
    password: &SecretString,
) -> Result<AuthResponse> {
    let auth = client.login(username, password)?;
    store.store_token(profile, &Token::new(auth.auth_token.clone()))?;
    Ok(auth)
}

/// Load a stored token, log it out remotely when present, and remove it locally.
pub fn logout_stored(client: &Client, store: &dyn CredentialStore, profile: &str) -> Result<bool> {
    let Some(token) = store.load_token(profile)? else {
        return Ok(false);
    };
    let logout_result = client.logout(&token);
    store.delete_token(profile)?;
    logout_result.map(|()| true)
}

fn parse_connections(value: serde_json::Value) -> Result<Vec<Connection>> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| Error::Http(format!("decoding connections response: {e}")))
            })
            .collect(),
        serde_json::Value::Object(map) => {
            let mut out = Vec::with_capacity(map.len());
            for (id, value) in map {
                let mut connection: Connection = serde_json::from_value(value)
                    .map_err(|e| Error::Http(format!("decoding connection {id:?}: {e}")))?;
                if connection.identifier.is_empty() {
                    connection.identifier = id;
                }
                out.push(connection);
            }
            out.sort_by(|a, b| {
                a.name
                    .cmp(&b.name)
                    .then_with(|| a.identifier.cmp(&b.identifier))
            });
            Ok(out)
        }
        other => Err(Error::Http(format!(
            "decoding connections response: expected object or array, got {other}"
        ))),
    }
}

fn normalize_base_url(base_url: &str) -> Result<Url> {
    let mut url =
        Url::parse(base_url).map_err(|e| Error::InvalidInput(format!("server URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidInput(format!(
            "server URL must use http or https, got {:?}",
            url.scheme()
        )));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn decode_response<T>(resp: reqwest::blocking::Response, context: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        let body = resp.text().unwrap_or_default();
        return Err(Error::Auth(format!("{context} returned {status}: {body}")));
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(Error::Http(format!("{context} returned {status}: {body}")));
    }
    resp.json()
        .map_err(|e| Error::Http(format!("decoding {context} response: {e}")))
}

fn expect_empty_success(resp: reqwest::blocking::Response, context: &str) -> Result<()> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        let body = resp.text().unwrap_or_default();
        return Err(Error::Auth(format!("{context} returned {status}: {body}")));
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(Error::Http(format!("{context} returned {status}: {body}")));
    }
    Ok(())
}
