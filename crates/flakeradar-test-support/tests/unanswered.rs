//! The loopback endpoint that reads a request and then answers nothing.
//!
//! The tests that hold a call open depend on it counting the call it read, so what
//! it does with a connection that never carries a whole request is pinned here:
//! the reader stops when the connection does, rather than waiting for bytes that
//! will not come and leaving the endpoint with no signal for its test.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the endpoint it drives does not answer"
)]

use std::io::Write as _;
use std::net::{Shutdown, TcpStream};

use flakeradar_test_support::Unanswered;

/// Writes `sent` to the endpoint as it arrived on a socket, and closes only the
/// writing half so the endpoint sees the end of the request.
fn send_raw(endpoint: &Unanswered, sent: &[u8]) {
    let address = endpoint.base.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).expect("a connection to the endpoint");
    stream
        .write_all(sent)
        .expect("the bytes reach the endpoint");
    stream
        .shutdown(Shutdown::Write)
        .expect("the writing half closes");
}

#[tokio::test]
async fn a_connection_that_carries_no_request_is_read_to_its_end() {
    // A connection pool opens a socket it never writes to, or writes a fragment
    // and gives up. The reader returns at the end of the connection, so the next
    // request finds the endpoint listening.
    let endpoint = Unanswered::start().await;
    send_raw(&endpoint, b"GET /hook HTTP/1.1\r\n");
    endpoint.arrived().await;
    assert_eq!(endpoint.requests(), 1, "the connection it read is counted");
}

#[tokio::test]
async fn a_request_whose_body_never_fully_arrives_is_read_to_its_end() {
    // The endpoint reads as much body as the request announced. One that announces
    // more than it sends ends with the connection, which is the same answer: the
    // reader is not left waiting for the rest of a body that will not come.
    let endpoint = Unanswered::start().await;
    send_raw(
        &endpoint,
        b"POST /hook HTTP/1.1\r\ncontent-length: 99\r\n\r\nshort",
    );
    endpoint.arrived().await;
    assert_eq!(endpoint.requests(), 1, "the request it read is counted");
}
