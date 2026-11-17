use crate::error::Result;
use crate::failure::Failure;
use crate::http::Connection;
use crate::protocol;

/// A streaming response in progress, from [`Invocation::stream`](crate::Invocation::stream).
///
/// - End it with [`finish`](Self::finish) or [`fail`](Self::fail). Dropping it closes the connection without the
///   final chunk; how Lambda reports such a response has not been verified.
pub struct Stream {
    connection: Connection,
}

impl Stream {
    pub(crate) fn new(connection: Connection) -> Self {
        Self { connection }
    }

    /// Writes `chunk` to the socket as one HTTP chunk.
    ///
    /// - The bytes have left the process when this returns. An empty chunk writes nothing, since a zero-length chunk
    ///   would end the body.
    /// - When the Runtime API has already rejected the stream and closed it, the rejection is returned as
    ///   [`Error::Rejected`](crate::Error::Rejected).
    pub fn send(&mut self, chunk: &[u8]) -> Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }

        let mut frame = format!("{:x}\r\n", chunk.len()).into_bytes();
        frame.extend_from_slice(chunk);
        frame.extend_from_slice(b"\r\n");

        self.connection.write(&frame)
    }

    /// Ends the body and waits for the Runtime API to accept the response.
    pub fn finish(mut self) -> Result<()> {
        self.connection.end(&[])?;
        self.connection.response()?.accepted()?;

        Ok(())
    }

    /// Ends the body with error trailers, reporting a failure after part of the response was sent.
    ///
    /// - Lambda counts the response as successful and passes the error to the client. The body trailer is the
    ///   `{"errorType", "errorMessage"}` JSON in base64.
    /// - A control character in `error_type` fails with [`Error::Header`](crate::Error::Header) and drops the
    ///   connection mid-body.
    pub fn fail(mut self, error_type: &str, message: &str) -> Result<()> {
        let body = Failure { error_type, message }.base64();

        self.connection
            .end(&[(protocol::ERROR_TYPE, error_type), (protocol::ERROR_BODY, &body)])?;
        self.connection.response()?.accepted()?;

        Ok(())
    }
}
