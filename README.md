# lambda-runtime-base

A blocking client for the AWS Lambda Runtime API, for runtimes that answer invocations their own way.

> This is not an AWS crate. The official runtime is [`lambda_runtime`](https://crates.io/crates/lambda_runtime).

`lambda_runtime` runs your handler in its own async loop and builds the response after the handler returns.
`lambda-runtime-base` gives you each invocation as a value instead: you decide when to answer and how, on plain `std`
sockets with no async runtime. It is the base to build on when you need one of these:

- open a streaming response before running code that must not be able to answer in your place
- return bytes exactly as they are, without JSON encoding or a Function URL prelude
- wait in `epoll`, a worker pool, or anything else that a tokio runtime would get in the way of

```rust
use lambda_runtime_base::{Result, Runtime};

fn main() -> Result<()> {
    // AWS_LAMBDA_RUNTIME_API names the endpoint inside Lambda.
    let runtime = Runtime::from_env()?;

    loop {
        // Blocks until the next event; Lambda may freeze the process meanwhile.
        let invocation = runtime.next()?;
        let event: serde_json::Value = match invocation.json() {
            Ok(event) => event,
            Err(error) => {
                invocation.fail("Runtime.UnmarshalError", &error.to_string())?;
                continue;
            }
        };

        // The head is on the socket when `stream` returns, before any code below runs.
        let mut stream = invocation.stream()?;
        stream.send(event.to_string().as_bytes())?;
        stream.finish()?;
    }
}
```

## Installation

```sh
cargo add lambda-runtime-base
```

## Answers

An `Invocation` is answered once. Each answer consumes it, so the compiler rules out a second one.

| Call                                      | Sends                                                                     |
|-------------------------------------------|---------------------------------------------------------------------------|
| `invocation.respond(&bytes)`              | `/response` with the bytes as they are                                    |
| `invocation.respond_json(&value)`         | `/response` with `value` as JSON                                          |
| `invocation.stream()`                     | a streaming `/response`, written with `send`, ended by `finish` or `fail` |
| `invocation.stream_as(content_type)`      | the same, with a `Content-Type` that Lambda passes to the client          |
| `invocation.fail(error_type, message)`    | `/error` with `{"errorType", "errorMessage"}`                             |
| `runtime.init_error(error_type, message)` | `/init/error`, before the first invocation                                |

- `stream()` sends `application/octet-stream`. For a Function URL, call
  `stream_as("application/vnd.awslambda.http-integration-response")` and send the JSON prelude with the status code
  and headers, then 8 NUL bytes, before the body.
- `invocation.json()` deserializes the event without consuming the invocation, so a bad event can still be failed.
- `id()`, `deadline()`, `function_arn()`, `trace_id()`, `client_context()`, `cognito_identity()`, and `tenant_id()`
  read the `/next` headers; `header(name)` reaches any other.
- Answers echo `Lambda-Runtime-Invocation-Id` when `/next` sent one.
- Errors are split by what the caller does next: `Io`, `Protocol`, and a 5xx `Rejected` mean the environment is
  broken, so exit; a 4xx `Rejected`, `Json`, and `Header` end one invocation at most.

## Things Lambda does that you should know

- Every call blocks without a timeout. Lambda freezes the process between invocations, and a timeout would fire on
  the frozen wait.
- A streaming response is sent on its own connection, which closes when the stream ends, as AWS requires.
- `stream()` returning `Ok` means the head was written, not accepted. A rejection shows up at a later `send` or at
  `finish()`, and so does a rejection that arrives before a large `respond` body is fully written.
- An `Invocation` dropped without an answer leaves Lambda waiting until its deadline. A `Stream` dropped without
  `finish()` or `fail(..)` closes the connection without the final chunk; how Lambda reports that has not been
  verified.
- Lambda's own runtimes copy `trace_id()` into `_X_AMZN_TRACE_ID`. This crate does not, because setting an
  environment variable races with other threads.
- With Lambda Managed Instances, several threads may call `next()` on one `Runtime` at once, up to
  `AWS_LAMBDA_MAX_CONCURRENCY`.
- Payload size limits are Lambda's to enforce. This crate reads what Lambda sends and lets Lambda reject what is too
  large.

## Security

Once a runtime has started a streaming response, Lambda refuses every other answer for that request ID with
`403 InvalidStateTransition`: a plain `/response`, a second streaming `/response`, and `/error`. The stream in
progress and the result of the invocation are not affected. Measured on 2026-11-17 on an arm64 container-image
function in ap-northeast-2.

So a runtime that calls `stream()` before running untrusted code keeps that code from forging the answer, even if it
reaches the Runtime API and learns the request ID. The guarantee comes from Lambda; this crate only makes sure the
head is on the socket first.

> Midstream errors from `Stream::fail` follow the AWS documentation on error trailers. How the error reaches the
> invoker has not been verified on Lambda yet.
