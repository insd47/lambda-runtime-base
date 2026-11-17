mod common;

use common::{ACCEPTED, Peer, Request};
use lambda_runtime_base::{Error, Invocation, Runtime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpListener;
use std::thread;

/// Delivers one invocation, then records the request that answers it and replies with `answer`.
fn answer(
    headers: &'static [(&'static str, &'static str)],
    event: &'static [u8],
    answer: Vec<u8>,
) -> (Invocation, thread::JoinHandle<Request>) {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        deliver(&listener, headers, event);

        let mut peer = Peer::accept(&listener);
        let request = peer.request();
        peer.reply(&answer);
        request
    });

    (runtime.next().unwrap(), server)
}

fn deliver(listener: &TcpListener, headers: &[(&str, &str)], event: &[u8]) {
    let mut peer = Peer::accept(listener);
    peer.request();
    peer.reply(&common::next(headers, event));
}

const WITH_INVOCATION_ID: &[(&str, &str)] = &[
    ("Lambda-Runtime-Aws-Request-Id", "request-1"),
    ("Lambda-Runtime-Deadline-Ms", "1"),
    ("Lambda-Runtime-Invocation-Id", "attempt-1"),
];
const WITHOUT_INVOCATION_ID: &[(&str, &str)] = &[
    ("Lambda-Runtime-Aws-Request-Id", "request-1"),
    ("Lambda-Runtime-Deadline-Ms", "1"),
];

#[test]
fn responses_are_sent_byte_for_byte_with_the_invocation_id() {
    let (invocation, server) = answer(WITH_INVOCATION_ID, b"{}", ACCEPTED.to_vec());
    let wasm = [0x00, 0x61, 0x73, 0x6d, 0xff, 0x00];

    invocation.respond(&wasm).unwrap();
    let request = server.join().unwrap();

    assert_eq!(
        request.line,
        "POST /2018-06-01/runtime/invocation/request-1/response HTTP/1.1"
    );
    assert_eq!(request.header("Lambda-Runtime-Invocation-Id"), Some("attempt-1"));
    assert_eq!(request.body, wasm);
}

#[test]
fn the_invocation_id_is_omitted_when_next_sent_none() {
    let (invocation, server) = answer(WITHOUT_INVOCATION_ID, b"{}", ACCEPTED.to_vec());

    invocation.respond(b"ok").unwrap();
    let request = server.join().unwrap();

    assert_eq!(request.header("Lambda-Runtime-Invocation-Id"), None);
}

#[test]
fn rejected_responses_name_the_state_transition() {
    let (invocation, server) = answer(
        WITHOUT_INVOCATION_ID,
        b"{}",
        common::rejected(403, "InvalidStateTransition"),
    );

    let result = invocation.respond(b"forged");
    server.join().unwrap();

    let Err(Error::Rejected { status, error_type, .. }) = result else {
        panic!("expected a rejection, got {:?}", result.err());
    };

    assert_eq!(status, 403);
    assert_eq!(error_type.as_deref(), Some("InvalidStateTransition"));
}

#[test]
fn failures_send_the_error_type_in_header_and_body() {
    let (invocation, server) = answer(WITH_INVOCATION_ID, b"{}", ACCEPTED.to_vec());

    invocation.fail("Builder.Rejected", "line 1: \"x\" undeclared").unwrap();
    let request = server.join().unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();

    assert_eq!(
        request.line,
        "POST /2018-06-01/runtime/invocation/request-1/error HTTP/1.1"
    );
    assert_eq!(
        request.header("Lambda-Runtime-Function-Error-Type"),
        Some("Builder.Rejected")
    );
    assert_eq!(request.header("Lambda-Runtime-Invocation-Id"), Some("attempt-1"));
    assert_eq!(
        body,
        serde_json::json!({ "errorType": "Builder.Rejected", "errorMessage": "line 1: \"x\" undeclared" })
    );
}

#[test]
fn error_types_with_line_breaks_are_refused_before_sending() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || deliver(&listener, WITHOUT_INVOCATION_ID, b"{}"));
    let invocation = runtime.next().unwrap();
    server.join().unwrap();

    let result = invocation.fail("Injected\r\nX-Evil: 1", "message");

    assert!(matches!(result, Err(Error::Header(_))));
}

#[derive(Deserialize)]
struct Order {
    count: u32,
}

#[derive(Serialize)]
struct Total {
    total: u32,
}

#[test]
fn json_events_and_responses_round_trip() {
    let (invocation, server) = answer(WITHOUT_INVOCATION_ID, br#"{"count":3}"#, ACCEPTED.to_vec());

    let order: Order = invocation.json().unwrap();
    invocation.respond_json(&Total { total: order.count * 2 }).unwrap();
    let request = server.join().unwrap();

    assert_eq!(request.body, br#"{"total":6}"#);
}

#[test]
fn unserializable_responses_fail_as_marshal_errors() {
    let (invocation, server) = answer(WITHOUT_INVOCATION_ID, b"{}", ACCEPTED.to_vec());
    let tuple_keys = HashMap::from([((1, 2), 3)]);

    let result = invocation.respond_json(&tuple_keys);
    let request = server.join().unwrap();

    assert!(matches!(result, Err(Error::Json(_))));
    assert!(request.line.ends_with("/invocation/request-1/error HTTP/1.1"));
    assert_eq!(
        request.header("Lambda-Runtime-Function-Error-Type"),
        Some("Runtime.MarshalError")
    );
}

#[test]
fn init_errors_go_to_the_init_endpoint() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        let request = peer.request();
        peer.reply(ACCEPTED);
        request
    });

    Runtime::init_error(runtime, "Runtime.ConfigInvalid", "missing table").unwrap();
    let request = server.join().unwrap();

    assert_eq!(request.line, "POST /2018-06-01/runtime/init/error HTTP/1.1");
    assert_eq!(
        request.header("Lambda-Runtime-Function-Error-Type"),
        Some("Runtime.ConfigInvalid")
    );
}

#[test]
fn an_early_rejection_of_a_large_body_is_reported() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        deliver(&listener, WITHOUT_INVOCATION_ID, b"{}");

        let mut peer = Peer::accept(&listener);
        peer.head();
        peer.reply(&common::rejected(413, "RequestEntityTooLarge"));
    });
    let invocation = runtime.next().unwrap();
    let body = vec![0u8; 8 << 20];

    let result = invocation.respond(&body);
    server.join().unwrap();

    assert!(matches!(result, Err(Error::Rejected { status: 413, .. })), "{result:?}");
}
