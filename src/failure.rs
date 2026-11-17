use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// An error report in the shape every Runtime API error endpoint reads.
pub struct Failure<'a> {
    pub error_type: &'a str,
    pub message: &'a str,
}

impl Failure<'_> {
    /// `{"errorType", "errorMessage"}`, the body of `/error` and `/init/error`
    pub fn json(&self) -> Vec<u8> {
        serde_json::json!({ "errorType": self.error_type, "errorMessage": self.message })
            .to_string()
            .into_bytes()
    }

    /// The JSON body in standard base64, the value of a midstream error trailer
    pub fn base64(&self) -> String {
        STANDARD.encode(self.json())
    }
}
