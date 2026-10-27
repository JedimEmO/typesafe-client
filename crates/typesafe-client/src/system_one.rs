use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::{
    Content, Error, ModelList, Questions, RetryPolicy, SystemOneRequest, SystemOneResponse,
};

/// Something that answers System One requests: the HTTP `Client` (feature `http`),
/// `fake::FakeSystemOne` in tests (feature `fake`), or your own implementation.
///
/// Hold an `Arc<dyn SystemOne>` in application code so tests can pass a fake. The trait is
/// implemented for `Arc<T>`, so calls read the same on either:
///
/// ```
/// use std::sync::Arc;
/// use typesafe_client::{Error, NoulQuestion, Questions, SystemOne};
///
/// struct Triage {
///     typesafe: Arc<dyn SystemOne>,
/// }
///
/// impl Triage {
///     async fn is_urgent(&self, ticket: &str) -> Result<bool, Error> {
///         let mut questions = Questions::new();
///         let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
///         let response = self.typesafe.system_one(ticket, questions).send().await?;
///         Ok(response.answer(&urgent)?.noul > 0.7)
///     }
/// }
/// ```
///
/// Implementors outside this crate can use the re-exported
/// [`async_trait`](crate::async_trait) attribute.
#[async_trait]
pub trait SystemOne: Send + Sync {
    /// Sends `request`, using the transport's default model when `request.model` is `None`.
    ///
    /// Implementations verify the response against `request.questions` (see
    /// [`SystemOneResponse::verify`]), so a successful response has an answer of the right
    /// kind for every question.
    async fn send(
        &self,
        request: &SystemOneRequest,
        options: &CallOptions,
    ) -> Result<SystemOneResponse, Error>;

    /// Lists the models available to the account.
    async fn list_models(&self) -> Result<ModelList, Error>;

    /// Starts a call that asks `questions` about `state`.
    fn system_one(&self, state: impl Into<Content>, questions: Questions) -> SystemOneCall<Self>
    where
        Self: Sized + Clone,
    {
        SystemOneCall::new(self.clone(), SystemOneRequest::new(state, questions))
    }
}

#[async_trait]
impl<T: SystemOne + ?Sized> SystemOne for Arc<T> {
    async fn send(
        &self,
        request: &SystemOneRequest,
        options: &CallOptions,
    ) -> Result<SystemOneResponse, Error> {
        (**self).send(request, options).await
    }

    async fn list_models(&self) -> Result<ModelList, Error> {
        (**self).list_models().await
    }
}

/// Per-call transport settings. Transports without timeouts or retries, such as the fake,
/// ignore them.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct CallOptions {
    /// Timeout for each attempt; `None` uses the transport's default.
    pub timeout: Option<Duration>,
    /// Retry policy; `None` uses the transport's default.
    pub retry: Option<RetryPolicy>,
    /// Raises the per-request and per-question context to 262k tokens by sending
    /// `TypeSafe-Danger-Zone-Long-Context: true`. It costs the same per token but is
    /// slower and less accurate, so turn it on only for requests that need it.
    pub long_context: bool,
}

impl CallOptions {
    /// Sets the timeout for each attempt.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the retry policy.
    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = Some(retry);
        self
    }

    /// Turns on long context; see [`CallOptions::long_context`].
    #[must_use]
    pub fn with_long_context(mut self) -> Self {
        self.long_context = true;
        self
    }
}

/// A prepared call. It owns its transport (a cheap clone of a `Client` or an `Arc`), so it
/// can be stored, cloned, returned from functions and spawned.
///
/// ```
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), typesafe_client::Error> {
/// use std::sync::Arc;
/// use typesafe_client::fake::FakeSystemOne;
/// use typesafe_client::{NoulQuestion, Questions, SystemOne};
///
/// let typesafe = Arc::new(FakeSystemOne::new());
/// let mut questions = Questions::new();
/// let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
///
/// let call = typesafe.system_one("Payouts failing for 3 days!", questions).model("jev-latest");
/// let second_attempt = call.clone();
///
/// let response = tokio::spawn(call.send()).await.expect("task ran")?;
/// assert_eq!(response.answer(&urgent)?.noul, 0.5);
/// assert!(second_attempt.send().await.is_ok());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
#[must_use = "a SystemOneCall does nothing until `send` is awaited"]
pub struct SystemOneCall<T> {
    transport: T,
    request: SystemOneRequest,
    options: CallOptions,
}

impl<T: SystemOne> SystemOneCall<T> {
    /// A call that sends `request` through `transport`.
    pub fn new(transport: T, request: SystemOneRequest) -> Self {
        Self {
            transport,
            request,
            options: CallOptions::default(),
        }
    }

    /// Uses `model` instead of the transport's default model.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.request.model = Some(model.into());
        self
    }

    /// Timeout for each attempt of this call.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    /// Retry policy for this call.
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.options.retry = Some(retry);
        self
    }

    /// Raises the context limits to 262k tokens for this call. Slower and less accurate
    /// than staying within the default limits; see [`CallOptions::long_context`].
    pub fn long_context(mut self) -> Self {
        self.options.long_context = true;
        self
    }

    /// Sends `assets` with the request: the images its state, instructions and criteria
    /// reference (feature `images`).
    #[cfg(feature = "images")]
    pub fn assets(mut self, assets: crate::Assets) -> Self {
        self.request.assets = assets;
        self
    }

    /// The request that will be sent.
    pub fn request(&self) -> &SystemOneRequest {
        &self.request
    }

    /// The timeout and retry settings of this call.
    pub fn options(&self) -> &CallOptions {
        &self.options
    }

    /// The request, without sending it (for logging, caching or sending later).
    pub fn into_request(self) -> SystemOneRequest {
        self.request
    }

    /// Sends the call. Clone the call first to send it more than once.
    pub async fn send(self) -> Result<SystemOneResponse, Error> {
        self.transport.send(&self.request, &self.options).await
    }
}
