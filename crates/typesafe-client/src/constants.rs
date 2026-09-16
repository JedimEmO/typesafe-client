//! Protocol names and defaults, shared by every transport.

use std::time::Duration;

/// Environment variable holding the API key.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";
/// Environment variable overriding the API base URL.
pub const BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
/// Environment variable overriding the default model.
pub const DEFAULT_MODEL_ENV: &str = "TYPESAFE_DEFAULT_MODEL";
/// Base URL of the hosted API.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Model used when neither the request nor the client names one.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// Timeout for each HTTP attempt.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Response header carrying the server's request id.
pub const REQUEST_ID_HEADER: &str = "x-typesafe-request-id";
/// Path of the System One evaluation endpoint.
pub const SYSTEM_ONE_PATH: &str = "/v1/systemone";
/// Path of the model listing endpoint.
pub const MODELS_PATH: &str = "/v1/models";
