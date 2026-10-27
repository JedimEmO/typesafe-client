use std::time::Duration;

use crate::constants::API_KEY_ENV;
#[cfg(feature = "images")]
use crate::images::{MAX_IMAGE_BYTES, MAX_REFERENCE_BYTES, MAX_TOTAL_REFERENCE_BYTES};
use crate::request::{MAX_CHOICE_OPTIONS, MAX_SCORE_LEVELS, MIN_SCORE_LEVELS};
use crate::{ApiError, QuestionKind, retry};

/// A request that breaks a documented API limit, caught before it is sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ValidationError {
    /// The request has no questions.
    #[error("the request has no questions")]
    NoQuestions,
    /// The model name is empty or whitespace.
    #[error("the model name is empty")]
    EmptyModel,
    /// Two questions were added under the same id.
    #[error("question id `{id}` is used more than once")]
    DuplicateQuestionId {
        /// The repeated id.
        id: String,
    },
    /// A Choice question has no options.
    #[error("choice question `{id}` has no options")]
    ChoiceWithoutOptions {
        /// The question's id.
        id: String,
    },
    /// A Choice question has more than [`MAX_CHOICE_OPTIONS`] options.
    #[error("choice question `{id}` has {count} options; at most {MAX_CHOICE_OPTIONS} are allowed")]
    TooManyChoiceOptions {
        /// The question's id.
        id: String,
        /// How many options it has.
        count: usize,
    },
    /// A Score question has fewer than [`MIN_SCORE_LEVELS`] or more than
    /// [`MAX_SCORE_LEVELS`] levels.
    #[error(
        "score question `{id}` has {count} levels; {MIN_SCORE_LEVELS} to {MAX_SCORE_LEVELS} are required"
    )]
    ScoreLevelCount {
        /// The question's id.
        id: String,
        /// How many levels it has.
        count: usize,
    },
    /// A Box question was asked in a request without images.
    #[cfg(feature = "images")]
    #[error("box question `{id}` needs an image in the request")]
    BoxWithoutImage {
        /// The question's id.
        id: String,
    },
    /// An image's reference appears nowhere in the state, instructions or criteria.
    #[cfg(feature = "images")]
    #[error("image `{reference}` is not referenced in the state, instructions or criteria")]
    UnreferencedImage {
        /// The unused reference.
        reference: String,
    },
    /// An image reference is empty or whitespace.
    #[cfg(feature = "images")]
    #[error("an image reference is empty")]
    EmptyImageReference,
    /// An image reference is longer than [`MAX_REFERENCE_BYTES`].
    #[cfg(feature = "images")]
    #[error("an image reference is {bytes} bytes; at most {MAX_REFERENCE_BYTES} are allowed")]
    ImageReferenceTooLong {
        /// The reference's length in bytes.
        bytes: usize,
    },
    /// The request's image references are longer than [`MAX_TOTAL_REFERENCE_BYTES`] together.
    #[cfg(feature = "images")]
    #[error(
        "the image references total {bytes} bytes; at most {MAX_TOTAL_REFERENCE_BYTES} are allowed"
    )]
    ImageReferencesTooLong {
        /// The references' total length in bytes.
        bytes: usize,
    },
    /// An image's bytes are not a PNG or JPEG file.
    #[cfg(feature = "images")]
    #[error("image `{reference}` is not a PNG or JPEG file")]
    NotAnImage {
        /// The image's reference.
        reference: String,
    },
    /// An image file is larger than [`MAX_IMAGE_BYTES`].
    #[cfg(feature = "images")]
    #[error("image `{reference}` is {bytes} bytes; at most {MAX_IMAGE_BYTES} are allowed")]
    ImageTooLarge {
        /// The image's reference.
        reference: String,
        /// The file's size in bytes.
        bytes: usize,
    },
}

/// A response that does not answer a question the way its question or key expects.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AnswerError {
    /// The response has no answer for the question.
    #[error("the response has no answer for question `{id}`")]
    Missing {
        /// The question's id.
        id: String,
    },
    /// The answer's kind differs from the question's (or the key's).
    #[error("question `{id}` was answered as a {found} question, expected {expected}")]
    WrongKind {
        /// The question's id.
        id: String,
        /// The kind the question or key expects.
        expected: QuestionKind,
        /// The kind the response contains.
        found: QuestionKind,
    },
    /// A Choice answer names an option the question does not have.
    #[error(
        "question `{id}` was answered with option `{option}`, which the question does not have"
    )]
    UnknownOption {
        /// The question's id.
        id: String,
        /// The unexpected option name.
        option: String,
    },
    /// A Score answer names a level the question does not have.
    #[error("question `{id}` was answered with level `{level}`, which the question does not have")]
    UnknownLevel {
        /// The question's id.
        id: String,
        /// The unexpected level key.
        level: String,
    },
}

/// Everything that can go wrong when asking TypeSafe questions.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No API key was given, and none was found in the environment.
    #[error("no API key: pass one to the client builder or set {API_KEY_ENV}")]
    MissingApiKey,
    /// The key contains characters that cannot be sent in an HTTP header, such as a newline.
    #[error("the API key contains characters that are not allowed in an HTTP header")]
    InvalidApiKey,
    /// The base URL does not parse, or is not `http` or `https`.
    #[error("invalid base URL `{url}`: {reason}")]
    InvalidBaseUrl {
        /// The rejected URL.
        url: String,
        /// Why it was rejected.
        reason: String,
    },
    /// The underlying HTTP client could not be created.
    #[error("failed to build the HTTP client")]
    HttpClient(#[source] TransportError),
    /// The request breaks a documented limit and was not sent.
    #[error("invalid request: {0}")]
    InvalidRequest(#[from] ValidationError),
    /// Serializing JSON failed, for example in [`Content::json`](crate::Content::json).
    #[error("failed to encode JSON")]
    Encode(#[from] serde_json::Error),
    /// The API answered with a non-success status.
    #[error(transparent)]
    Api(Box<ApiError>),
    /// An attempt exceeded its timeout, or the call exceeded its total budget.
    #[error("the request timed out")]
    Timeout(#[source] TransportError),
    /// The server could not be reached, or the connection failed mid-response.
    #[error("could not reach the TypeSafe API")]
    Connection(#[source] TransportError),
    /// The HTTP request could not be built. Retrying would fail the same way.
    #[error("the HTTP request could not be built")]
    Request(#[source] TransportError),
    /// A success response had a body that is not the expected JSON.
    #[error("failed to decode the response body: {body_excerpt}")]
    Decode {
        /// The JSON error.
        #[source]
        source: serde_json::Error,
        /// The start of the body, for diagnosis.
        body_excerpt: String,
        /// The response's request id.
        request_id: Option<String>,
    },
    /// The response did not answer the questions that were sent, or an answer was read
    /// with a key from a different request.
    #[error(transparent)]
    Answer(#[from] AnswerError),
}

impl Error {
    /// The API error, if the server answered with a non-success status.
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Self::Api(error) => Some(error),
            _ => None,
        }
    }

    /// The HTTP status of an API error.
    pub fn status(&self) -> Option<u16> {
        self.api().map(|api| api.status)
    }

    /// How long the server asked to wait before retrying.
    pub fn retry_after(&self) -> Option<Duration> {
        self.api().and_then(|api| api.retry_after)
    }

    /// The `x-typesafe-request-id` of the failed response, useful when reporting issues.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Api(error) => error.request_id.as_deref(),
            Self::Decode { request_id, .. } => request_id.as_deref(),
            _ => None,
        }
    }

    /// Whether the default [`RetryPolicy`](crate::RetryPolicy) retries this error: 408, 429
    /// and 5xx responses, timeouts and connection failures.
    pub fn is_retryable(&self) -> bool {
        retry::retryable_by_default(self)
    }
}

impl From<ApiError> for Error {
    fn from(error: ApiError) -> Self {
        Self::Api(Box::new(error))
    }
}

/// A failure below the API level: DNS, TCP, TLS, timeouts or request construction.
///
/// Wraps the underlying error without exposing the HTTP library's types. Build one with
/// [`TransportError::new`] to script transport failures in a fake.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct TransportError(Box<dyn std::error::Error + Send + Sync>);

impl TransportError {
    /// Wraps `error`, which can be any error or a message such as `"connection refused"`.
    pub fn new(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self(error.into())
    }

    /// The wrapped error, for downcasting.
    pub fn get_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        &*self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_status_retry_after_and_request_id_of_api_errors() {
        let error = Error::from(
            ApiError::new(429, "slow down")
                .with_retry_after(Duration::from_secs(2))
                .with_request_id("req_1"),
        );

        assert_eq!(error.status(), Some(429));
        assert_eq!(error.retry_after(), Some(Duration::from_secs(2)));
        assert_eq!(error.request_id(), Some("req_1"));
        assert!(error.is_retryable());
    }

    #[test]
    fn classifies_retryable_errors_like_the_default_policy() {
        assert!(Error::Timeout(TransportError::new("timed out")).is_retryable());
        assert!(Error::Connection(TransportError::new("refused")).is_retryable());
        assert!(Error::from(ApiError::new(529, "overloaded")).is_retryable());
        assert!(!Error::from(ApiError::new(401, "bad key")).is_retryable());
        assert!(!Error::Request(TransportError::new("bad header")).is_retryable());
        assert!(!Error::InvalidRequest(ValidationError::NoQuestions).is_retryable());
    }

    #[test]
    fn converts_content_encoding_errors_with_question_mark() {
        fn encode() -> Result<crate::Content, Error> {
            // JSON object keys must be strings, so a map keyed by tuples cannot be encoded.
            let state: std::collections::HashMap<(i32, i32), i32> = [((1, 2), 3)].into();
            Ok(crate::Content::json(&state)?)
        }

        assert!(matches!(encode(), Err(Error::Encode(_))));
    }

    #[test]
    fn displays_transport_errors_as_the_wrapped_error() {
        let error = Error::Connection(TransportError::new("connection refused"));

        assert_eq!(error.to_string(), "could not reach the TypeSafe API");
        let source = std::error::Error::source(&error).map(ToString::to_string);
        assert_eq!(source.as_deref(), Some("connection refused"));
    }
}
