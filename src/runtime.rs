use crate::error::{Error, Result};
use crate::failure::Failure;
use crate::http::Connection;
use crate::invocation::Invocation;
use crate::protocol;

/// A client for the Runtime API of one execution environment.
///
/// - Calls are independent and open their own connections, so one `Runtime` can serve several threads. Lambda
///   Managed Instances accept concurrent `/next` calls up to `AWS_LAMBDA_MAX_CONCURRENCY`.
/// - Every call blocks without a timeout, as the Runtime API requires.
#[derive(Debug, Clone)]
pub struct Runtime {
    endpoint: String,
}

impl Runtime {
    /// A runtime that talks to `endpoint`, a `host:port` such as `127.0.0.1:9001`.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    /// A runtime that talks to the endpoint Lambda sets in `AWS_LAMBDA_RUNTIME_API`.
    pub fn from_env() -> Result<Self> {
        let endpoint = std::env::var(protocol::ENDPOINT_VAR).map_err(Error::Endpoint)?;

        Ok(Self::new(endpoint))
    }

    /// Waits for the next invocation.
    ///
    /// - Lambda may freeze the process while this call waits, for any length of time.
    pub fn next(&self) -> Result<Invocation> {
        let path = format!("{}/invocation/next", protocol::PREFIX);
        let response = Connection::send(&self.endpoint, "GET", &path, &[], &[])?.accepted()?;

        Invocation::new(self.endpoint.clone(), response)
    }

    /// Reports that initialization failed. Exit the process afterwards.
    ///
    /// - `error_type` is a `Category.Reason` such as `Runtime.ConfigInvalid`. Lambda normalizes other shapes to
    ///   `Runtime.Unknown`.
    pub fn init_error(self, error_type: &str, message: &str) -> Result<()> {
        let failure = Failure { error_type, message };
        let path = format!("{}/init/error", protocol::PREFIX);

        let headers = [(protocol::ERROR_TYPE, error_type), ("Content-Type", "application/json")];

        Connection::send(&self.endpoint, "POST", &path, &headers, &failure.json())?.accepted()?;

        Ok(())
    }
}
