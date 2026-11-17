mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::{ACCEPTED, Peer, Request};
use lambda_runtime_base::{Error, Invocation};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Delivers one invocation and returns the listener its answer will connect to.
fn invocation() -> (Invocation, TcpListener) {
    let (runtime, listener) = common::runtime();
    let listener = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        peer.request();
        peer.reply(&common::next(
            &[
                ("Lambda-Runtime-Aws-Request-Id", "request-1"),
                ("Lambda-Runtime-Deadline-Ms", "1"),
            ],
            b"{}",
        ));
        listener
    });
    let invocation = runtime.next().unwrap();

    (invocation, listener.join().unwrap())
}

/// Records the streamed request and replies with `answer` once the body ends.
fn record(listener: TcpListener, answer: Vec<u8>) -> thread::JoinHandle<(Request, Vec<u8>)> {
    thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        let head = peer.head();
        let body = peer.chunked();
        peer.reply(&answer);

        (head, body)
    })
}

#[test]
fn the_head_is_on_the_socket_before_stream_returns() {
    let (invocation, listener) = invocation();
    let (heads, received) = mpsc::channel();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        heads.send(peer.head()).unwrap();
        peer.chunked();
        peer.reply(ACCEPTED);
    });

    let stream = invocation.stream().unwrap();

    // Nothing else is written until the head has been read in full on the other side.
    let head = received
        .recv_timeout(Duration::from_secs(5))
        .expect("head not on the socket");

    stream.finish().unwrap();
    server.join().unwrap();

    assert_eq!(
        head.line,
        "POST /2018-06-01/runtime/invocation/request-1/response HTTP/1.1"
    );
    assert_eq!(head.header("Lambda-Runtime-Function-Response-Mode"), Some("streaming"));
    assert_eq!(head.header("Transfer-Encoding"), Some("chunked"));
}

#[test]
fn chunks_are_sent_verbatim_without_a_prelude() {
    let (invocation, listener) = invocation();
    let server = record(listener, ACCEPTED.to_vec());

    let mut stream = invocation.stream().unwrap();
    stream.send(b"first").unwrap();
    stream.send(b"").unwrap();
    stream.send(&[0u8; 17]).unwrap();
    stream.finish().unwrap();
    let (_, body) = server.join().unwrap();

    let mut expected = b"5\r\nfirst\r\n11\r\n".to_vec();
    expected.extend_from_slice(&[0u8; 17]);
    expected.extend_from_slice(b"\r\n0\r\n\r\n");
    assert_eq!(body, expected);
}

#[test]
fn midstream_failures_end_with_error_trailers() {
    let (invocation, listener) = invocation();
    let server = record(listener, ACCEPTED.to_vec());

    let mut stream = invocation.stream().unwrap();
    stream.send(b"partial").unwrap();
    stream.fail("Runner.Crashed", "worker died").unwrap();
    let (head, body) = server.join().unwrap();

    let error = STANDARD.encode(r#"{"errorMessage":"worker died","errorType":"Runner.Crashed"}"#);
    let expected = format!(
        "7\r\npartial\r\n0\r\n\
         Lambda-Runtime-Function-Error-Type: Runner.Crashed\r\n\
         Lambda-Runtime-Function-Error-Body: {error}\r\n\r\n"
    );

    assert_eq!(
        head.header("Trailer"),
        Some("Lambda-Runtime-Function-Error-Type, Lambda-Runtime-Function-Error-Body")
    );
    assert_eq!(String::from_utf8(body).unwrap(), expected);
}

#[test]
fn a_rejected_stream_surfaces_at_finish() {
    let (invocation, listener) = invocation();
    let server = record(listener, common::rejected(403, "InvalidStateTransition"));

    let mut stream = invocation.stream().unwrap();
    stream.send(b"late").unwrap();
    let result = stream.finish();
    server.join().unwrap();

    assert!(matches!(result, Err(Error::Rejected { status: 403, .. })));
}

#[test]
fn an_early_rejection_wins_over_the_broken_pipe() {
    let (invocation, listener) = invocation();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        peer.head();
        peer.reply(&common::rejected(403, "InvalidStateTransition"));
    });

    let mut stream = invocation.stream().unwrap();
    server.join().unwrap();
    let mut result = Ok(());

    for _ in 0..64 {
        result = stream.send(&[0u8; 64 * 1024]);

        if result.is_err() {
            break;
        }
    }

    let result = match result {
        Ok(()) => stream.finish(),
        Err(error) => Err(error),
    };

    assert!(matches!(result, Err(Error::Rejected { status: 403, .. })), "{result:?}");
}

#[test]
fn stream_as_sends_the_content_type() {
    let (invocation, listener) = invocation();
    let server = record(listener, ACCEPTED.to_vec());

    let mut stream = invocation.stream_as("text/event-stream").unwrap();
    stream.send(b"data: 1\n\n").unwrap();
    stream.finish().unwrap();
    let (head, _) = server.join().unwrap();

    assert_eq!(head.header("Content-Type"), Some("text/event-stream"));
}

#[test]
fn content_types_with_line_breaks_are_refused_before_connecting() {
    let (invocation, listener) = invocation();

    let result = invocation.stream_as("text/plain\r\nX-Evil: 1");
    listener.set_nonblocking(true).unwrap();

    assert!(matches!(result, Err(Error::Header(_))));
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}
