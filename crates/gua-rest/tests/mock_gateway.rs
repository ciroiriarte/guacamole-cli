//! Smoke test proving the mock-gateway harness (issue #9) works end-to-end.

use gua_rest::testing::MockGateway;

#[tokio::test]
async fn login_stub_serves_canonical_body() {
    let gw = MockGateway::start().await;
    gw.stub_login("guacadmin", "postgresql").await;

    let resp = reqwest_min_post(&format!("{}/api/tokens", gw.base_url())).await;
    assert!(resp.contains("\"dataSource\":\"postgresql\""));
    assert!(resp.contains("\"authToken\""));
}

/// Tiny dependency-free POST using the std TCP stack, so this test does not pull
/// an HTTP client into `gua-rest` before issue #11 introduces one.
async fn reqwest_min_post(url: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let url = url.strip_prefix("http://").expect("http url");
    let (authority, path) = url.split_once('/').expect("path");
    let path = format!("/{path}");

    let mut stream = TcpStream::connect(authority).expect("connect");
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write");
    let mut buf = String::new();
    stream.read_to_string(&mut buf).expect("read");
    buf
}
