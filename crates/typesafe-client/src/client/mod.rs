//! Async HTTP client for the TypeSafe API, enabled by the `http` feature.
//!
//! ```no_run
//! use typesafe_client::{Client, NoulQuestion, Questions};
//!
//! # async fn run() -> Result<(), typesafe_client::Error> {
//! // Reads TYPESAFE_API_KEY, and optionally TYPESAFE_BASE_URL and TYPESAFE_DEFAULT_MODEL.
//! let client = Client::from_env()?;
//!
//! let mut questions = Questions::new();
//! let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
//!
//! let response = client
//!     .system_one("Help! My payouts have been failing for 3 days.", questions)
//!     .send()
//!     .await?;
//! println!("urgent: {:.2}", response.answer(&urgent)?.noul);
//! # Ok(())
//! # }
//! ```

mod retry_after;

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderValue, USER_AGENT};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use crate::constants::{
    API_KEY_ENV, BASE_URL_ENV, DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_MODEL_ENV, DEFAULT_TIMEOUT,
    REQUEST_ID_HEADER,
};
use crate::constants::{IMAGES_HEADER, LONG_CONTEXT_HEADER, MODELS_PATH, SYSTEM_ONE_PATH};
use crate::{
    ApiError, CallOptions, Content, Error, ModelList, Questions, RetryPolicy, SystemOne,
    SystemOneCall, SystemOneRequest, SystemOneResponse, TransportError,
};

const USER_AGENT_VALUE: &str = concat!("typesafe-client-rust/", env!("CARGO_PKG_VERSION"));

/// A TypeSafe API client. Cheap to clone; clones share one connection pool.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
    timeout: Duration,
    retry: RetryPolicy,
    validate_requests: bool,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("default_model", &self.inner.default_model)
            .field("timeout", &self.inner.timeout)
            .field("retry", &self.inner.retry)
            .field("validate_requests", &self.inner.validate_requests)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client with `api_key`; base URL and default model come from the environment or
    /// the defaults.
    pub fn new(api_key: impl Into<String>) -> Result<Self, Error> {
        Self::builder().api_key(api_key).build()
    }

    /// A client configured entirely from the environment (see [`API_KEY_ENV`]).
    pub fn from_env() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// A builder for configuring the client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The base URL requests are sent to.
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// The model used when a request does not name one.
    pub fn default_model(&self) -> &str {
        &self.inner.default_model
    }

    /// Starts a call that asks `questions` about `state`. The call owns a clone of the
    /// client, so it can be spawned or stored:
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), typesafe_client::Error> {
    /// use typesafe_client::{Client, NoulQuestion, Questions};
    ///
    /// let client = Client::from_env()?;
    /// let mut questions = Questions::new();
    /// let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
    ///
    /// let call = client
    ///     .system_one("Payouts failing for 3 days!", questions)
    ///     .model("jev-latest")
    ///     .timeout(std::time::Duration::from_secs(5));
    /// let response = tokio::spawn(call.send()).await.expect("task ran")?;
    /// println!("urgent: {:.2}", response.answer(&urgent)?.noul);
    /// # Ok(())
    /// # }
    /// ```
    pub fn system_one(
        &self,
        state: impl Into<Content>,
        questions: Questions,
    ) -> SystemOneCall<Self> {
        SystemOneCall::new(self.clone(), SystemOneRequest::new(state, questions))
    }

    /// Sends a prepared request with the client's default timeout and retry policy. Send
    /// the same request repeatedly, changing only `request.state`, to reuse questions
    /// without cloning them.
    pub async fn evaluate(&self, request: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        self.execute(request, &CallOptions::default()).await
    }

    /// Lists the models available to the account.
    pub async fn list_models(&self) -> Result<ModelList, Error> {
        let url = self.endpoint(MODELS_PATH);
        let (models, _) = self
            .send_with_retry(&CallOptions::default(), || self.inner.http.get(&url))
            .await?;
        Ok(models)
    }

    async fn execute(
        &self,
        request: &SystemOneRequest,
        options: &CallOptions,
    ) -> Result<SystemOneResponse, Error> {
        if self.inner.validate_requests {
            request.validate()?;
        }
        let model = request
            .model
            .as_deref()
            .unwrap_or(&self.inner.default_model);
        let body = RequestBody::new(request, model)?;
        let uses_images = request.uses_images();
        let url = self.endpoint(SYSTEM_ONE_PATH);
        let (mut response, request_id): (SystemOneResponse, _) = self
            .send_with_retry(options, || {
                let mut builder = self.inner.http.post(&url);
                if uses_images {
                    builder = builder.header(IMAGES_HEADER, "true");
                }
                if options.long_context {
                    builder = builder.header(LONG_CONTEXT_HEADER, "true");
                }
                body.attach(builder)
            })
            .await?;
        response.request_id = request_id;
        response.verify(&request.questions)?;
        Ok(response)
    }

    async fn send_with_retry<T: DeserializeOwned>(
        &self,
        options: &CallOptions,
        make_request: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<(T, Option<String>), Error> {
        let policy = options.retry.as_ref().unwrap_or(&self.inner.retry);
        let timeout = options.timeout.unwrap_or(self.inner.timeout);
        let started = Instant::now();
        let mut retries_done = 0;
        loop {
            let attempt_timeout = policy
                .remaining(started.elapsed())
                .map_or(timeout, |remaining| timeout.min(remaining));
            let error = match self.attempt(make_request(), attempt_timeout).await {
                Ok(success) => return Ok(success),
                Err(error) => error,
            };
            if retries_done >= policy.max_retries || !policy.should_retry(&error) {
                return Err(error);
            }
            let delay =
                policy.delay_before_retry(retries_done, error.retry_after(), fastrand::f64());
            if policy.exceeds_budget(started.elapsed(), delay) {
                return Err(error);
            }
            tokio::time::sleep(delay).await;
            retries_done += 1;
        }
    }

    async fn attempt<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        timeout: Duration,
    ) -> Result<(T, Option<String>), Error> {
        let response = request
            .bearer_auth(&self.inner.api_key)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .timeout(timeout)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let retry_after = retry_after::retry_after(response.headers());
        let body = response.bytes().await.map_err(transport_error)?;

        if status.is_success() {
            match serde_json::from_slice(&body) {
                Ok(value) => Ok((value, request_id)),
                Err(source) => Err(Error::Decode {
                    source,
                    body_excerpt: crate::api_error::excerpt(&String::from_utf8_lossy(&body)),
                    request_id,
                }),
            }
        } else {
            Err(ApiError::from_response(status.as_u16(), &body, request_id, retry_after).into())
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.inner.base_url)
    }
}

#[async_trait]
impl SystemOne for Client {
    async fn send(
        &self,
        request: &SystemOneRequest,
        options: &CallOptions,
    ) -> Result<SystemOneResponse, Error> {
        self.execute(request, options).await
    }

    async fn list_models(&self) -> Result<ModelList, Error> {
        Self::list_models(self).await
    }
}

#[derive(Serialize)]
struct WireRequest<'a> {
    state: &'a Content,
    model: &'a str,
    questions: &'a Questions,
    #[cfg(feature = "images")]
    #[serde(skip_serializing_if = "Option::is_none")]
    assets: Option<indexmap::IndexMap<&'a str, WirePartAsset>>,
}

/// An image asset whose bytes are in the multipart part `part`.
#[cfg(feature = "images")]
#[derive(Serialize)]
struct WirePartAsset {
    r#type: &'static str,
    format: crate::ImageFormat,
    part: String,
}

/// A request body, rebuilt into each attempt's HTTP request.
enum RequestBody<'a> {
    /// The JSON body. The marker keeps the lifetime used when `images` is off.
    Json(Vec<u8>, std::marker::PhantomData<&'a ()>),
    /// The JSON request as the first part, named `request`, then one file part per image.
    #[cfg(feature = "images")]
    Multipart {
        request: Vec<u8>,
        images: Vec<(String, &'a crate::Image)>,
    },
}

impl<'a> RequestBody<'a> {
    fn new(request: &'a SystemOneRequest, model: &'a str) -> Result<Self, Error> {
        #[cfg(feature = "images")]
        if !request.assets.is_empty() {
            let images: Vec<(String, &crate::Image)> = request
                .assets
                .iter()
                .enumerate()
                .map(|(index, (_, image))| (format!("image-{index}"), image))
                .collect();
            let assets = request
                .assets
                .iter()
                .zip(&images)
                .map(|((reference, image), (part, _))| {
                    let asset = WirePartAsset {
                        r#type: "image",
                        format: image.format(),
                        part: part.clone(),
                    };
                    (reference.as_str(), asset)
                })
                .collect();
            let request = serde_json::to_vec(&WireRequest {
                state: &request.state,
                model,
                questions: &request.questions,
                assets: Some(assets),
            })?;
            return Ok(Self::Multipart { request, images });
        }
        let body = serde_json::to_vec(&WireRequest {
            state: &request.state,
            model,
            questions: &request.questions,
            #[cfg(feature = "images")]
            assets: None,
        })?;
        Ok(Self::Json(body, std::marker::PhantomData))
    }

    fn attach(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::Json(body, _) => builder
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone()),
            #[cfg(feature = "images")]
            Self::Multipart { request, images } => {
                use reqwest::multipart::{Form, Part};

                let json = Part::bytes(request.clone())
                    .mime_str("application/json")
                    .expect("application/json is a valid MIME type");
                let form =
                    images
                        .iter()
                        .fold(Form::new().part("request", json), |form, (name, image)| {
                            let format = image.format();
                            let part = Part::bytes(image.data().to_vec())
                                .file_name(format!("{name}.{format}"))
                                .mime_str(format.mime_type())
                                .expect("image MIME types are valid");
                            form.part(name.clone(), part)
                        });
                builder.multipart(form)
            }
        }
    }
}

fn transport_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout(TransportError::new(error))
    } else if error.is_builder() {
        Error::Request(TransportError::new(error))
    } else {
        Error::Connection(TransportError::new(error))
    }
}

/// Configures a [`Client`]. Values not set explicitly fall back to the environment,
/// then to the defaults.
#[derive(Clone)]
pub struct ClientBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    default_model: Option<String>,
    timeout: Duration,
    retry: RetryPolicy,
    validate_requests: bool,
    http_client: Option<reqwest::Client>,
    read_env: bool,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            api_key: None,
            base_url: None,
            default_model: None,
            timeout: DEFAULT_TIMEOUT,
            retry: RetryPolicy::default(),
            validate_requests: true,
            http_client: None,
            read_env: true,
        }
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("default_model", &self.default_model)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .field("validate_requests", &self.validate_requests)
            .field("read_env", &self.read_env)
            .finish_non_exhaustive()
    }
}

impl ClientBuilder {
    /// The API key, sent as a bearer token.
    #[must_use]
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// The API base URL, for example a proxy. Defaults to [`DEFAULT_BASE_URL`].
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// The model used when a request does not name one. Pin a concrete model (rather than
    /// `jev-latest`) when answers must stay comparable over time.
    #[must_use]
    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Timeout for each HTTP attempt, further limited by [`RetryPolicy::total_timeout`].
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The retry policy for every call; calls can override it.
    #[must_use]
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Whether requests are checked against documented limits before sending (default
    /// `true`). Turn off if the API's limits change before this crate does.
    #[must_use]
    pub fn validate_requests(mut self, validate: bool) -> Self {
        self.validate_requests = validate;
        self
    }

    /// Uses a preconfigured `reqwest` 0.13 client, for example with a proxy.
    #[must_use]
    pub fn http_client(mut self, http_client: reqwest::Client) -> Self {
        self.http_client = Some(http_client);
        self
    }

    /// Ignores `TYPESAFE_*` environment variables, so only explicit settings and defaults
    /// apply. Use this when your application has its own configuration.
    #[must_use]
    pub fn ignore_env(mut self) -> Self {
        self.read_env = false;
        self
    }

    /// Builds the client. Fails when no API key is set (explicitly or, unless
    /// [`ClientBuilder::ignore_env`] was called, in `TYPESAFE_API_KEY`), or when the key or
    /// base URL is invalid.
    pub fn build(self) -> Result<Client, Error> {
        let read_env = self.read_env;
        self.build_with_env(|name| {
            if read_env {
                std::env::var(name).ok()
            } else {
                None
            }
        })
    }

    fn build_with_env(self, env: impl Fn(&str) -> Option<String>) -> Result<Client, Error> {
        let setting = |explicit: Option<String>, name: &str| {
            explicit
                .or_else(|| env(name))
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };

        let api_key = setting(self.api_key, API_KEY_ENV).ok_or(Error::MissingApiKey)?;
        if HeaderValue::from_str(&format!("Bearer {api_key}")).is_err() {
            return Err(Error::InvalidApiKey);
        }
        let base_url = setting(self.base_url, BASE_URL_ENV)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        check_base_url(&base_url)?;
        let default_model = setting(self.default_model, DEFAULT_MODEL_ENV)
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let http = match self.http_client {
            Some(http) => http,
            None => reqwest::Client::builder()
                .build()
                .map_err(|error| Error::HttpClient(TransportError::new(error)))?,
        };

        Ok(Client {
            inner: Arc::new(Inner {
                http,
                base_url,
                api_key,
                default_model,
                timeout: self.timeout,
                retry: self.retry,
                validate_requests: self.validate_requests,
            }),
        })
    }
}

fn check_base_url(base_url: &str) -> Result<(), Error> {
    let invalid = |reason: String| Error::InvalidBaseUrl {
        url: base_url.to_owned(),
        reason,
    };
    let url = reqwest::Url::parse(base_url).map_err(|error| invalid(error.to_string()))?;
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(invalid(format!("unsupported scheme `{other}`"))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn build(builder: ClientBuilder, env: &[(&str, &str)]) -> Result<Client, Error> {
        let env: HashMap<_, _> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        builder.build_with_env(|name| env.get(name).cloned())
    }

    #[test]
    fn uses_defaults_when_only_the_key_is_in_the_environment() {
        let client = build(Client::builder(), &[(API_KEY_ENV, "env-key")]).unwrap();

        assert_eq!(client.base_url(), DEFAULT_BASE_URL);
        assert_eq!(client.default_model(), DEFAULT_MODEL);
        assert_eq!(client.inner.api_key, "env-key");
    }

    #[test]
    fn prefers_explicit_settings_over_the_environment() {
        let env = [
            (API_KEY_ENV, "env-key"),
            (BASE_URL_ENV, "https://env.example"),
            (DEFAULT_MODEL_ENV, "env-model"),
        ];
        let builder = Client::builder()
            .api_key("explicit-key")
            .base_url("http://localhost:9000/")
            .default_model("jev-1.12");

        let client = build(builder, &env).unwrap();

        assert_eq!(client.inner.api_key, "explicit-key");
        assert_eq!(client.base_url(), "http://localhost:9000");
        assert_eq!(client.default_model(), "jev-1.12");
    }

    #[test]
    fn reads_base_url_and_model_from_the_environment_when_not_set() {
        let env = [
            (API_KEY_ENV, "k"),
            (BASE_URL_ENV, "https://proxy.example/typesafe"),
            (DEFAULT_MODEL_ENV, "jev-1.12"),
        ];

        let client = build(Client::builder(), &env).unwrap();

        assert_eq!(
            client.endpoint(SYSTEM_ONE_PATH),
            "https://proxy.example/typesafe/v1/systemone"
        );
        assert_eq!(client.default_model(), "jev-1.12");
    }

    #[test]
    fn fails_when_api_key_is_missing_or_blank() {
        assert!(matches!(
            build(Client::builder(), &[]),
            Err(Error::MissingApiKey)
        ));
        assert!(matches!(
            build(Client::builder().api_key("  "), &[(API_KEY_ENV, "env-key")]),
            Err(Error::MissingApiKey)
        ));
    }

    #[test]
    fn fails_when_api_key_is_not_a_valid_header_value() {
        assert!(matches!(
            build(Client::builder().api_key("sk-live\n-123"), &[]),
            Err(Error::InvalidApiKey)
        ));
    }

    #[test]
    fn fails_when_base_url_is_not_http() {
        for url in ["not a url", "ftp://example.com"] {
            assert!(matches!(
                build(Client::builder().api_key("k").base_url(url), &[]),
                Err(Error::InvalidBaseUrl { .. })
            ));
        }
    }

    #[test]
    fn ignores_the_environment_when_asked() {
        // Holds whether or not TYPESAFE_API_KEY is set in the environment running the tests.
        assert!(matches!(
            Client::builder().ignore_env().build(),
            Err(Error::MissingApiKey)
        ));

        let client = Client::builder().ignore_env().api_key("k").build().unwrap();
        assert_eq!(client.base_url(), DEFAULT_BASE_URL);
        assert_eq!(client.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn redacts_the_api_key_in_debug_output() {
        let builder = Client::builder().api_key("super-secret");
        let client = build(builder.clone(), &[]).unwrap();

        assert!(!format!("{builder:?}").contains("super-secret"));
        assert!(!format!("{client:?}").contains("super-secret"));
    }

    #[test]
    fn classifies_request_building_failures_as_not_retryable() {
        let builder_error = reqwest::Client::new()
            .get("not a url")
            .build()
            .expect_err("an invalid URL cannot be built");

        let error = transport_error(builder_error);

        assert!(matches!(error, Error::Request(_)), "got {error:?}");
        assert!(!error.is_retryable());
    }

    #[test]
    fn leaves_model_to_the_client_default_unless_the_call_overrides_it() {
        let client = build(Client::builder().api_key("k"), &[]).unwrap();

        let default_call = client.system_one("state", Questions::new());
        assert_eq!(default_call.request().model, None);
        let overridden = client
            .system_one("state", Questions::new())
            .model("jev-1.12");
        assert_eq!(overridden.into_request().model.as_deref(), Some("jev-1.12"));
    }
}
