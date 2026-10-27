#![cfg_attr(feature = "http", doc = include_str!("../README.md"))]
#![cfg_attr(
    not(feature = "http"),
    doc = "Typed Rust client for the TypeSafe System One API (unofficial).\n\n\
           This build has no `http` feature: it provides the request and response types, question \
           builders, validation and the [`SystemOne`] trait. See the \
           [README](https://github.com/JedimEmO/typesafe-client) for the full guide."
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

mod answer;
mod api_error;
mod choice_options;
pub mod constants;
mod content;
mod error;
#[cfg(feature = "images")]
pub mod images;
mod question;
mod request;
mod retry;
mod system_one;

#[cfg(feature = "http")]
pub mod client;
#[cfg(feature = "fake")]
pub mod fake;

pub use answer::{
    Answer, ChoiceAnswer, ModelList, ModelMetadata, NoulAnswer, ReadAnswer, ScoreAnswer,
    SystemOneResponse, Usage,
};
#[cfg(feature = "images")]
pub use answer::{BoxAnswer, BoxCoordinates, PixelBox};
pub use api_error::{ApiError, ApiErrorKind, FieldError};
#[doc(hidden)]
pub use choice_options::__str_eq;
pub use choice_options::{ChoiceOptions, EnumChoice, TypedChoice, ValueChoice};
pub use content::Content;
pub use error::{AnswerError, Error, TransportError, ValidationError};
#[cfg(feature = "images")]
pub use images::{Assets, Image, ImageFormat, ImageRef};
#[cfg(feature = "images")]
pub use question::BoxQuestion;
pub use question::{
    ChoiceQuestion, NoulCriteria, NoulQuestion, Question, QuestionKind, ScoreQuestion,
};
pub use request::{
    IntoQuestion, MAX_CHOICE_OPTIONS, MAX_SCORE_LEVELS, MIN_SCORE_LEVELS, QuestionKey, Questions,
    SystemOneRequest,
};
pub use retry::RetryPolicy;
pub use system_one::{CallOptions, SystemOne, SystemOneCall};

#[cfg(feature = "http")]
pub use client::{Client, ClientBuilder};
/// Re-exported so a custom client for [`ClientBuilder::http_client`] uses the same version.
#[cfg(feature = "http")]
pub use reqwest;

/// Re-exported for implementing [`SystemOne`] without adding the dependency yourself.
pub use async_trait::async_trait;
/// Re-exported because [`ChoiceQuestion::criteria`] and answer maps are [`indexmap::IndexMap`]s.
pub use indexmap;
