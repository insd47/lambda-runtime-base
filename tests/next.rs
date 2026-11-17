mod common;

use common::Peer;
use lambda_runtime_base::Error;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn next_reads_context_and_body() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        let request = peer.request();
        let headers = [
            ("Lambda-Runtime-Aws-Request-Id", "request-1"),
            ("Lambda-Runtime-Deadline-Ms", "1542409706888"),
            (
                "Lambda-Runtime-Invoked-Function-Arn",
                "arn:aws:lambda:ap-northeast-2:123456789012:function:f",
            ),
            ("Lambda-Runtime-Trace-Id", "Root=1-5bef4de7-ad49b0e87f6ef6c87fc2e700"),
            ("lambda-runtime-aws-tenant-id", "tenant-1"),
            ("X-Future-Header", "kept"),
        ];

        peer.reply(&common::next(&headers, b"{\"n\":1}"));
        request
    });

    let invocation = runtime.next().unwrap();
    let request = server.join().unwrap();

    assert_eq!(request.line, "GET /2018-06-01/runtime/invocation/next HTTP/1.1");
    assert_eq!(invocation.id(), "request-1");
    assert_eq!(
        invocation.deadline(),
        UNIX_EPOCH + Duration::from_millis(1_542_409_706_888)
    );
    assert_eq!(
        invocation.function_arn(),
        Some("arn:aws:lambda:ap-northeast-2:123456789012:function:f")
    );
    assert_eq!(invocation.trace_id(), Some("Root=1-5bef4de7-ad49b0e87f6ef6c87fc2e700"));
    assert_eq!(invocation.tenant_id(), Some("tenant-1"));
    assert_eq!(invocation.client_context(), None);
    assert_eq!(invocation.header("x-future-header"), Some("kept"));
    assert_eq!(invocation.body(), b"{\"n\":1}");
}

#[test]
fn chunked_events_survive_single_byte_reads() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        peer.request();
        peer.reply_slowly(
            b"HTTP/1.1 200 OK\r\n\
              Lambda-Runtime-Aws-Request-Id: request-1\r\n\
              Lambda-Runtime-Deadline-Ms: 1\r\n\
              Transfer-Encoding: chunked\r\n\r\n\
              5;ext=1\r\nhello\r\n\
              B\r\n, chunked\r\n\r\n\
              0\r\n\
              X-Trailer: ignored\r\n\r\n",
        );
    });

    let invocation = runtime.next().unwrap();
    server.join().unwrap();

    assert_eq!(invocation.body(), b"hello, chunked\r\n");
}

#[test]
fn events_without_request_id_are_protocol_errors() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        peer.request();
        peer.reply(&common::next(&[("Lambda-Runtime-Deadline-Ms", "1")], b"{}"));
    });

    let result = runtime.next();
    server.join().unwrap();

    assert!(matches!(result, Err(Error::Protocol(_))));
}

#[test]
fn a_body_cut_short_is_a_protocol_error() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        peer.request();
        peer.reply(b"HTTP/1.1 200 OK\r\nLambda-Runtime-Aws-Request-Id: r\r\nContent-Length: 10\r\n\r\nshort");
    });

    let result = runtime.next();
    server.join().unwrap();

    assert!(matches!(result, Err(Error::Protocol(_))));
}

#[test]
fn container_errors_carry_status_and_type() {
    let (runtime, listener) = common::runtime();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        peer.request();
        peer.reply(&common::rejected(500, "Runtime.Unknown"));
    });

    let result = runtime.next();
    server.join().unwrap();

    let Err(Error::Rejected { status, error_type, .. }) = result else {
        panic!("expected a rejection, got {:?}", result.err());
    };

    assert_eq!(status, 500);
    assert_eq!(error_type.as_deref(), Some("Runtime.Unknown"));
}

#[test]
fn runtimes_and_invocations_cross_threads() {
    fn shared<T: Send + Sync>() {}
    fn moved<T: Send>() {}

    shared::<lambda_runtime_base::Runtime>();
    moved::<lambda_runtime_base::Invocation>();
    moved::<lambda_runtime_base::Stream>();
}
