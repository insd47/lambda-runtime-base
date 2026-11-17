/// Failure modes of Runtime API calls, split by what the caller does next.
///
/// - [`Io`](Error::Io), [`Protocol`](Error::Protocol), and a 5xx [`Rejected`](Error::Rejected) mean the execution
///   environment is unusable: exit the process and let Lambda start a new one.
/// - A 4xx [`Rejected`](Error::Rejected), [`Json`](Error::Json), and [`Header`](Error::Header) end one
///   invocation at most: move on to the next one.
/// - New variants may be added in minor releases, so a `match` needs a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `AWS_LAMBDA_RUNTIME_API` is not set, so the process is not running inside Lambda.
    #[error("AWS_LAMBDA_RUNTIME_API is not set")]
    Endpoint(#[source] std::env::VarError),
    /// The Runtime API could not be reached, or the connection broke mid-request.
    #[error("Runtime API connection failed")]
    Io(#[from] std::io::Error),
    /// The Runtime API answered with an error status.
    ///
    /// - 400 `InvalidInvocationId`: the echoed invocation ID does not match the active invocation.
    /// - 403 `InvalidStateTransition`: the invocation already has a response, such as once a stream has started.
    /// - 500: the container is in a non-recoverable state; exit promptly.
    #[error(
        "Runtime API rejected the request with {status}: {}: {}",
        .error_type.as_deref().unwrap_or("no error type"),
        .message.as_deref().unwrap_or("no message")
    )]
    #[non_exhaustive]
    Rejected {
        status: u16,
        /// `errorType` from the response body, such as `InvalidStateTransition`
        error_type: Option<String>,
        /// `errorMessage` from the response body
        message: Option<String>,
    },
    /// The Runtime API sent a response this crate cannot read, such as one without a request ID.
    #[error("Runtime API sent a malformed response: {0}")]
    Protocol(String),
    /// An event did not deserialize into the requested type, or a response did not serialize.
    #[error("invalid JSON")]
    Json(#[from] serde_json::Error),
    /// A header value, such as an error type or a content type, contains a control character. Nothing was sent.
    #[error("header value {0:?} contains a control character")]
    Header(String),
}

/// Convenience alias for results produced by this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
