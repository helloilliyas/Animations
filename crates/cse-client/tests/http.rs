//! HTTP behaviour (retries, status handling) against a local stub server.
//! These tests check our client, not the CSE's response format.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cse_client::{CseClient, Endpoint, Error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves the given (status, body) responses in order, one per connection.
async fn stub(responses: Vec<(u16, &'static str)>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        for (status, body) in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            counter.fetch_add(1, Ordering::SeqCst);
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    (format!("http://{addr}/api"), hits)
}

#[tokio::test]
async fn retries_server_errors_then_succeeds() {
    let (base, hits) = stub(vec![(503, "{}"), (200, r#"{"status":"Market Open"}"#)]).await;
    let client = CseClient::new(base).with_retries(2);
    assert_eq!(client.market_status().await.unwrap(), "Market Open");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    let (base, hits) = stub(vec![(404, "{}"), (200, "{}")]).await;
    let client = CseClient::new(base).with_retries(2);
    let err = client.raw(Endpoint::Aspi, &[]).await.unwrap_err();
    assert!(matches!(err, Error::Status(404)));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_json_is_reported() {
    let (base, _) = stub(vec![(200, "<html>")]).await;
    let err = CseClient::new(base)
        .raw(Endpoint::Aspi, &[])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Json(_)));
}
