use crate::error::{Error, Result};
use crate::failure::Failure;
use crate::http::{Connection, Response};
use crate::protocol;
use crate::stream::Stream;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One invocation, answered exactly once.
///
/// - [`respond`](Self::respond), [`stream`](Self::stream), and [`fail`](Self::fail) consume the invocation, so it
///   gets one answer. Lambda waits for the answer of an invocation dropped without one until its deadline.
/// - Answers echo `Lambda-Runtime-Invocation-Id` when `/next` sent one.
pub struct Invocation {
    endpoint: String,
    id: String,
    deadline: SystemTime,
    response: Response,
}

impl Invocation {
    pub(crate) fn new(endpoint: String, response: Response) -> Result<Self> {
        let protocol = |reason: &str| Error::Protocol(reason.to_owned());

        let id = response
            .header(protocol::REQUEST_ID)
            .ok_or_else(|| protocol("no request ID"))?;
        let deadline_ms = response
            .header(protocol::DEADLINE_MS)
            .ok_or_else(|| protocol("no deadline"))?;
        let deadline_ms: u64 = deadline_ms.parse().map_err(|_| protocol("deadline is not a number"))?;

        Ok(Self {
            endpoint,
            id: id.to_owned(),
            deadline: UNIX_EPOCH + Duration::from_millis(deadline_ms),
            response,
        })
    }

    /// The request ID, `Lambda-Runtime-Aws-Request-Id`
    pub fn id(&self) -> &str {
        &self.id
    }

    /// When Lambda stops the invocation, `Lambda-Runtime-Deadline-Ms`
    pub fn deadline(&self) -> SystemTime {
        self.deadline
    }

    /// The ARN of the function, version, or alias that was invoked
    pub fn function_arn(&self) -> Option<&str> {
        self.header(protocol::FUNCTION_ARN)
    }

    /// The X-Ray tracing header.
    ///
    /// - Lambda's own runtimes copy it into `_X_AMZN_TRACE_ID`. This crate does not, because setting an environment
    ///   variable races with other threads; set it yourself while no other thread runs, if a tracer reads it.
    pub fn trace_id(&self) -> Option<&str> {
        self.header(protocol::TRACE_ID)
    }

    /// Client application and device data as JSON, sent by the AWS Mobile SDK
    pub fn client_context(&self) -> Option<&str> {
        self.header(protocol::CLIENT_CONTEXT)
    }

    /// The Amazon Cognito identity as JSON, sent by the AWS Mobile SDK
    pub fn cognito_identity(&self) -> Option<&str> {
        self.header(protocol::COGNITO_IDENTITY)
    }

    /// The tenant of a function in tenant isolation mode
    pub fn tenant_id(&self) -> Option<&str> {
        self.header(protocol::TENANT_ID)
    }

    /// Any header of the `/next` response, matched case-insensitively.
    ///
    /// - Reaches headers Lambda adds after this release.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.response.header(name)
    }

    /// The event payload as Lambda sent it
    pub fn body(&self) -> &[u8] {
        &self.response.body
    }

    /// Deserializes the event payload.
    ///
    /// - The invocation stays open on failure; report it with [`fail`](Self::fail).
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.response.body)?)
    }

    /// Answers with `body`, byte for byte.
    pub fn respond(self, body: &[u8]) -> Result<()> {
        let path = self.path("response");

        Connection::send(&self.endpoint, "POST", &path, &self.echo(), body)?.accepted()?;

        Ok(())
    }

    /// Answers with `value` serialized as JSON.
    ///
    /// - When `value` does not serialize, the invocation fails as `Runtime.MarshalError` and the serde error is
    ///   returned as [`Error::Json`].
    pub fn respond_json<T: Serialize + ?Sized>(self, value: &T) -> Result<()> {
        let body = match serde_json::to_vec(value) {
            Ok(body) => body,
            Err(error) => {
                self.fail("Runtime.MarshalError", &error.to_string())?;
                return Err(error.into());
            }
        };

        self.respond(&body)
    }

    /// Starts a streaming response.
    ///
    /// - When this returns `Ok`, the whole request head has been written to the socket. Lambda rejects any other
    ///   answer for an invocation whose stream has started (see the README's Security section), so code that must
    ///   not answer in your place can run after this call.
    /// - Written is not accepted: a rejection of the stream surfaces at a later [`Stream::send`] or at
    ///   [`Stream::finish`].
    /// - The stream travels on its own connection, which closes when the stream ends, as Lambda requires.
    /// - The content type is `application/octet-stream`; [`stream_as`](Self::stream_as) sets another.
    pub fn stream(self) -> Result<Stream> {
        self.stream_as("application/octet-stream")
    }

    /// Starts a streaming response with `content_type`, which Lambda passes on to the client.
    ///
    /// - A Function URL reads `application/vnd.awslambda.http-integration-response` as a JSON prelude with the
    ///   status code and headers, then 8 NUL bytes, then the body; send the prelude yourself as the first chunks.
    /// - A control character in `content_type` fails with [`Error::Header`] before anything is sent.
    /// - Otherwise the same as [`stream`](Self::stream).
    pub fn stream_as(self, content_type: &str) -> Result<Stream> {
        let path = self.path("response");
        let mut headers = self.echo();
        headers.extend([
            (protocol::RESPONSE_MODE, "streaming"),
            ("Transfer-Encoding", "chunked"),
            (
                "Trailer",
                "Lambda-Runtime-Function-Error-Type, Lambda-Runtime-Function-Error-Body",
            ),
            ("Content-Type", content_type),
        ]);

        let connection = Connection::start(&self.endpoint, "POST", &path, &headers)?;

        Ok(Stream::new(connection))
    }

    /// Reports that the invocation failed.
    ///
    /// - `error_type` is a `Category.Reason` such as `Function.Rejected`. It travels in a header, so a control
    ///   character in it fails with [`Error::Header`] before anything is sent.
    pub fn fail(self, error_type: &str, message: &str) -> Result<()> {
        let failure = Failure { error_type, message };
        let path = self.path("error");
        let mut headers = self.echo();
        headers.extend([(protocol::ERROR_TYPE, error_type), ("Content-Type", "application/json")]);

        Connection::send(&self.endpoint, "POST", &path, &headers, &failure.json())?.accepted()?;

        Ok(())
    }

    fn path(&self, action: &str) -> String {
        format!("{}/invocation/{}/{action}", protocol::PREFIX, self.id)
    }

    /// The invocation ID to echo, when `/next` sent one
    fn echo(&self) -> Vec<(&str, &str)> {
        let id = self.header(protocol::INVOCATION_ID);

        id.map(|id| (protocol::INVOCATION_ID, id)).into_iter().collect()
    }
}
