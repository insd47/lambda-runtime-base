//! Paths and header names of Runtime API version `2018-06-01`.

pub const ENDPOINT_VAR: &str = "AWS_LAMBDA_RUNTIME_API";
pub const PREFIX: &str = "/2018-06-01/runtime";

pub const REQUEST_ID: &str = "Lambda-Runtime-Aws-Request-Id";
pub const DEADLINE_MS: &str = "Lambda-Runtime-Deadline-Ms";
pub const FUNCTION_ARN: &str = "Lambda-Runtime-Invoked-Function-Arn";
pub const TRACE_ID: &str = "Lambda-Runtime-Trace-Id";
pub const CLIENT_CONTEXT: &str = "Lambda-Runtime-Client-Context";
pub const COGNITO_IDENTITY: &str = "Lambda-Runtime-Cognito-Identity";
pub const TENANT_ID: &str = "Lambda-Runtime-Aws-Tenant-Id";
pub const INVOCATION_ID: &str = "Lambda-Runtime-Invocation-Id";

pub const RESPONSE_MODE: &str = "Lambda-Runtime-Function-Response-Mode";
pub const ERROR_TYPE: &str = "Lambda-Runtime-Function-Error-Type";
pub const ERROR_BODY: &str = "Lambda-Runtime-Function-Error-Body";
