use std::fmt;
use std::time::Duration;

use serde_json::Value;

const EXCERPT_CHARS: usize = 500;
const CONTEXT_EXCEEDED: &str = "max_tokens_exceeded";

/// A non-success response from the API.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ApiError {
    /// The HTTP status code.
    pub status: u16,
    /// The category of the status.
    pub kind: ApiErrorKind,
    /// The server's `detail.error_type`, for example `authentication_error`.
    pub error_type: Option<String>,
    /// A human-readable description taken from the response body.
    pub message: String,
    /// The response body, when it was JSON.
    pub body: Option<Value>,
    /// The `x-typesafe-request-id` response header.
    pub request_id: Option<String>,
    /// How long the server asked to wait, from `retry-after-ms` or `Retry-After`.
    pub retry_after: Option<Duration>,
}

impl ApiError {
    /// An error with `status` and `message`, for example to script a failure in a fake.
    ///
    /// ```
    /// use std::time::Duration;
    /// use typesafe_client::{ApiError, ApiErrorKind};
    ///
    /// let error = ApiError::new(429, "Too many requests").with_retry_after(Duration::from_secs(1));
    /// assert_eq!(error.kind, ApiErrorKind::RateLimited);
    /// ```
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            kind: ApiErrorKind::from_status(status),
            error_type: None,
            message: message.into(),
            body: None,
            request_id: None,
            retry_after: None,
        }
    }

    /// Sets the server's error type, such as `authentication_error`.
    #[must_use]
    pub fn with_error_type(mut self, error_type: impl Into<String>) -> Self {
        self.error_type = Some(error_type.into());
        self
    }

    /// Sets the request id.
    #[must_use]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    /// Sets how long the server asked to wait.
    #[must_use]
    pub fn with_retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = Some(retry_after);
        self
    }

    /// Builds an error from a raw HTTP response, for transports other than the built-in
    /// client. Reads `{"detail": {"error_type", "message"}}`, `{"detail": "..."}` and
    /// validation lists (`{"detail": [{"loc", "msg", "type"}]}`).
    pub fn from_response(
        status: u16,
        body: &[u8],
        request_id: Option<String>,
        retry_after: Option<Duration>,
    ) -> Self {
        let json = serde_json::from_slice::<Value>(body).ok();
        let (error_type, message) = json.as_ref().map_or((None, None), describe_body);
        Self {
            status,
            kind: ApiErrorKind::from_status(status),
            error_type,
            message: message.unwrap_or_else(|| fallback_message(status, body)),
            body: json,
            request_id,
            retry_after,
        }
    }

    /// The fields a validation (422) response rejected. Empty for other errors.
    pub fn field_errors(&self) -> Vec<FieldError> {
        self.body.as_ref().map(field_errors_in).unwrap_or_default()
    }

    /// Whether the request was longer than the model's context length (a 400 with error
    /// type `max_tokens_exceeded`). Shorten the state, ask fewer questions, send fewer or
    /// smaller images, or turn on [long context](crate::CallOptions::long_context).
    pub fn is_context_length_exceeded(&self) -> bool {
        self.status == 400 && self.error_type.as_deref() == Some(CONTEXT_EXCEEDED)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TypeSafe API returned {} ({}): {}",
            self.status, self.kind, self.message
        )?;
        if let Some(request_id) = &self.request_id {
            write!(f, " [request {request_id}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

/// One rejected field from a validation response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldError {
    /// Path to the value, for example `["body", "questions", "q", "criteria"]`.
    pub location: Vec<String>,
    /// Why the value was rejected.
    pub message: String,
    /// Machine-readable code, for example `missing`.
    pub kind: Option<String>,
}

impl FieldError {
    /// A rejected field at `location` (for example `["body", "model"]`).
    pub fn new(
        location: impl IntoIterator<Item = impl Into<String>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            location: location.into_iter().map(Into::into).collect(),
            message: message.into(),
            kind: None,
        }
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.location.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.location.join("."), self.message)
        }
    }
}

/// The category of an [`ApiError`], derived from its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// 400
    BadRequest,
    /// 401: missing or invalid API key.
    Authentication,
    /// 403
    PermissionDenied,
    /// 404
    NotFound,
    /// 408
    RequestTimeout,
    /// 413: the body, an image, or the request with its images expanded is over a size
    /// limit.
    PayloadTooLarge,
    /// 415: an unsupported `Content-Encoding`.
    UnsupportedMediaType,
    /// 422: the body failed validation; see [`ApiError::field_errors`].
    UnprocessableEntity,
    /// 429
    RateLimited,
    /// 529: temporarily overloaded.
    Overloaded,
    /// Any other 5xx.
    InternalServer,
    /// Any other status.
    Other,
}

impl ApiErrorKind {
    /// The kind for an HTTP status code.
    pub const fn from_status(status: u16) -> Self {
        match status {
            400 => Self::BadRequest,
            401 => Self::Authentication,
            403 => Self::PermissionDenied,
            404 => Self::NotFound,
            408 => Self::RequestTimeout,
            413 => Self::PayloadTooLarge,
            415 => Self::UnsupportedMediaType,
            422 => Self::UnprocessableEntity,
            429 => Self::RateLimited,
            529 => Self::Overloaded,
            500..=599 => Self::InternalServer,
            _ => Self::Other,
        }
    }
}

impl fmt::Display for ApiErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BadRequest => "bad request",
            Self::Authentication => "authentication failed",
            Self::PermissionDenied => "permission denied",
            Self::NotFound => "not found",
            Self::RequestTimeout => "request timeout",
            Self::PayloadTooLarge => "payload too large",
            Self::UnsupportedMediaType => "unsupported media type",
            Self::UnprocessableEntity => "invalid request body",
            Self::RateLimited => "rate limited",
            Self::Overloaded => "overloaded",
            Self::InternalServer => "server error",
            Self::Other => "unexpected status",
        })
    }
}

fn describe_body(body: &Value) -> (Option<String>, Option<String>) {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    match body.get("detail") {
        Some(Value::Object(detail)) => {
            let error_type = text(detail.get("error_type"));
            let message = text(detail.get("message")).or_else(|| {
                error_type.as_deref().map(|error_type| match error_type {
                    CONTEXT_EXCEEDED => {
                        "the request is longer than the model's context length".to_owned()
                    }
                    other => other.to_owned(),
                })
            });
            (error_type, message)
        }
        Some(Value::String(message)) => (None, Some(message.clone())),
        Some(Value::Array(_)) => {
            let fields = field_errors_in(body);
            let message = (!fields.is_empty()).then(|| {
                fields
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            });
            (None, message)
        }
        _ => (text(body.get("error_type")), text(body.get("message"))),
    }
}

fn field_errors_in(body: &Value) -> Vec<FieldError> {
    let Some(issues) = body.get("detail").and_then(Value::as_array) else {
        return Vec::new();
    };
    issues
        .iter()
        .map(|issue| FieldError {
            location: issue
                .get("loc")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .map(|part| {
                            part.as_str()
                                .map_or_else(|| part.to_string(), str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            message: issue
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("invalid value")
                .to_owned(),
            kind: issue.get("type").and_then(Value::as_str).map(str::to_owned),
        })
        .collect()
}

fn fallback_message(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if !text.is_empty() {
        return excerpt(text);
    }
    canonical_reason(status).map_or_else(|| format!("HTTP {status}"), str::to_owned)
}

fn canonical_reason(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => return None,
    })
}

pub(crate) fn excerpt(text: &str) -> String {
    match text.char_indices().nth(EXCERPT_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_error_type_and_message_from_detail_object() {
        let body = br#"{"detail":{"error_type":"authentication_error","message":"Cannot authenticate with the server."}}"#;

        let error = ApiError::from_response(401, body, Some("req_1".into()), None);

        assert_eq!(error.kind, ApiErrorKind::Authentication);
        assert_eq!(error.error_type.as_deref(), Some("authentication_error"));
        assert_eq!(error.message, "Cannot authenticate with the server.");
        assert_eq!(error.field_errors(), []);
        assert_eq!(
            error.to_string(),
            "TypeSafe API returned 401 (authentication failed): Cannot authenticate with the server. [request req_1]"
        );
    }

    #[test]
    fn exposes_rejected_fields_when_detail_is_a_validation_list() {
        let body = br#"{"detail":[
            {"loc":["body","questions","q","criteria"],"msg":"Field required","type":"missing"},
            {"loc":["body","questions",0],"msg":"Input should be a valid string","type":"string_type"}
        ]}"#;

        let error = ApiError::from_response(422, body, None, None);

        assert_eq!(error.kind, ApiErrorKind::UnprocessableEntity);
        assert_eq!(
            error.field_errors(),
            [
                FieldError {
                    location: ["body", "questions", "q", "criteria"]
                        .map(String::from)
                        .into(),
                    message: "Field required".into(),
                    kind: Some("missing".into()),
                },
                FieldError {
                    location: ["body", "questions", "0"].map(String::from).into(),
                    message: "Input should be a valid string".into(),
                    kind: Some("string_type".into()),
                },
            ]
        );
        assert_eq!(
            error.message,
            "body.questions.q.criteria: Field required; body.questions.0: Input should be a valid string"
        );
    }

    #[test]
    fn formats_field_errors_with_their_location() {
        assert_eq!(
            FieldError::new(["body", "model"], "Field required").to_string(),
            "body.model: Field required"
        );
        assert_eq!(
            FieldError::new(Vec::<String>::new(), "Invalid body").to_string(),
            "Invalid body"
        );
    }

    #[test]
    fn uses_plain_body_or_status_reason_when_body_is_not_json() {
        assert_eq!(
            ApiError::from_response(502, b"upstream connect error", None, None).message,
            "upstream connect error"
        );
        assert_eq!(
            ApiError::from_response(503, b"", None, None).message,
            "Service Unavailable"
        );
        assert_eq!(
            ApiError::from_response(529, b"", None, None).message,
            "HTTP 529"
        );
    }

    #[test]
    fn recognises_context_length_errors_that_have_no_message() {
        let body = br#"{"detail":{"error_type":"max_tokens_exceeded"}}"#;

        let error = ApiError::from_response(400, body, None, None);

        assert!(error.is_context_length_exceeded());
        assert_eq!(
            error.message,
            "the request is longer than the model's context length"
        );
        assert!(!ApiError::new(400, "bad").is_context_length_exceeded());
    }

    #[test]
    fn uses_the_error_type_as_message_when_the_detail_has_no_message() {
        let error = ApiError::from_response(400, br#"{"detail":{"error_type":"odd"}}"#, None, None);

        assert_eq!(error.message, "odd");
    }

    #[test]
    fn maps_statuses_to_kinds() {
        assert_eq!(
            ApiErrorKind::from_status(413),
            ApiErrorKind::PayloadTooLarge
        );
        assert_eq!(
            ApiErrorKind::from_status(415),
            ApiErrorKind::UnsupportedMediaType
        );
        assert_eq!(
            ApiErrorKind::from_status(403),
            ApiErrorKind::PermissionDenied
        );
        assert_eq!(ApiErrorKind::from_status(529), ApiErrorKind::Overloaded);
        assert_eq!(ApiErrorKind::from_status(500), ApiErrorKind::InternalServer);
        assert_eq!(ApiErrorKind::from_status(418), ApiErrorKind::Other);
    }

    #[test]
    fn truncates_long_bodies_on_a_char_boundary() {
        let long = "å".repeat(EXCERPT_CHARS + 10);

        let shortened = excerpt(&long);

        assert_eq!(shortened.chars().count(), EXCERPT_CHARS + 1);
        assert!(shortened.ends_with('…'));
    }
}
