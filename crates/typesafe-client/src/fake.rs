//! An in-memory [`SystemOne`] for tests, enabled by the `fake` feature.
//!
//! [`FakeSystemOne`] answers every question it receives, so code under test gets a
//! complete, well-typed response without network access. Unconfigured questions get
//! maximally uncertain answers; set the answers a test cares about, script responses and
//! failures, and inspect the requests that were sent.
//!
//! ```
//! use std::sync::Arc;
//! use std::time::Duration;
//! use typesafe_client::fake::FakeSystemOne;
//! use typesafe_client::{ApiError, Error, NoulQuestion, Questions, SystemOne};
//!
//! struct Triage {
//!     typesafe: Arc<dyn SystemOne>,
//! }
//!
//! impl Triage {
//!     async fn is_urgent(&self, ticket: &str) -> Result<bool, Error> {
//!         let mut questions = Questions::new();
//!         let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
//!         let response = self.typesafe.system_one(ticket, questions).send().await?;
//!         Ok(response.answer(&urgent)?.noul > 0.7)
//!     }
//! }
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Error> {
//! let fake = Arc::new(FakeSystemOne::new());
//! let triage = Triage { typesafe: fake.clone() };
//!
//! fake.set_noul("is_urgent", 0.92);
//! assert!(triage.is_urgent("Payouts have failed for 3 days!").await?);
//! assert_eq!(fake.last_request().unwrap().state.as_text(), Some("Payouts have failed for 3 days!"));
//!
//! fake.push_error(ApiError::new(429, "Too many requests").with_retry_after(Duration::from_secs(1)));
//! let error = triage.is_urgent("again").await.unwrap_err();
//! assert_eq!(error.retry_after(), Some(Duration::from_secs(1)));
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;

use crate::constants::DEFAULT_MODEL;
use crate::{
    Answer, CallOptions, ChoiceAnswer, Content, Error, ModelList, ModelMetadata, NoulAnswer,
    Question, ScoreAnswer, SystemOne, SystemOneRequest, SystemOneResponse, Usage,
};

/// The probability given to Noul questions without a configured answer.
pub const UNCERTAIN_NOUL: f64 = 0.5;

/// An in-memory [`SystemOne`] that answers from configuration instead of a model.
///
/// Without configuration, a Noul answers [`UNCERTAIN_NOUL`], a Choice spreads probability
/// evenly across its options (choosing the first), and a Score spreads it evenly across
/// its levels. Confidence is derived from the distribution's entropy: 0 for an even
/// spread, 1 for a certain answer. The real API computes confidence its own way.
///
/// Responses are checked against the request like the HTTP client checks them, so a
/// configured option or answer kind that doesn't match the question fails the call with
/// [`Error::Answer`] instead of producing an impossible response.
///
/// Share it as `Arc<FakeSystemOne>`; `Arc<FakeSystemOne>` implements [`SystemOne`] and
/// converts to `Arc<dyn SystemOne>`.
#[derive(Default)]
pub struct FakeSystemOne {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    configured: HashMap<String, Configured>,
    scripted: VecDeque<Result<SystemOneResponse, Error>>,
    requests: Vec<SystemOneRequest>,
    models: Option<ModelList>,
}

#[derive(Clone)]
enum Configured {
    Noul(f64),
    Choice(Vec<(String, f64)>),
    Score(f64),
    Exact(Answer),
}

impl FakeSystemOne {
    /// A fake with no configured answers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers the Noul question `id` with `probability` from now on.
    pub fn set_noul(&self, id: impl AsRef<str>, probability: f64) {
        self.configure(id, Configured::Noul(probability));
    }

    /// Answers the Choice question `id` with `option`, certainly, from now on.
    pub fn set_choice(&self, id: impl AsRef<str>, option: impl Into<String>) {
        self.configure(id, Configured::Choice(vec![(option.into(), 1.0)]));
    }

    /// Answers the Choice question `id` with these probabilities from now on. Options not
    /// listed get 0; the most probable option is the choice.
    pub fn set_choice_probabilities<I, O>(&self, id: impl AsRef<str>, probabilities: I)
    where
        I: IntoIterator<Item = (O, f64)>,
        O: Into<String>,
    {
        let probabilities = probabilities
            .into_iter()
            .map(|(option, probability)| (option.into(), probability))
            .collect();
        self.configure(id, Configured::Choice(probabilities));
    }

    /// Answers the Score question `id` with `score` from now on, splitting probability
    /// between the two nearest levels.
    pub fn set_score(&self, id: impl AsRef<str>, score: f64) {
        self.configure(id, Configured::Score(score));
    }

    /// Answers question `id` with exactly `answer` from now on.
    pub fn set_answer(&self, id: impl AsRef<str>, answer: Answer) {
        self.configure(id, Configured::Exact(answer));
    }

    /// Forgets every configured answer.
    pub fn clear_answers(&self) {
        self.state().configured.clear();
    }

    /// Returns `response` for the next request, before configured answers apply. Scripted
    /// responses and errors are used in the order they were pushed.
    pub fn push_response(&self, response: SystemOneResponse) {
        self.state().scripted.push_back(Ok(response));
    }

    /// Fails the next request with `error`, for example an [`ApiError`](crate::ApiError) or
    /// `Error::Timeout(TransportError::new("timed out"))`.
    pub fn push_error(&self, error: impl Into<Error>) {
        self.state().scripted.push_back(Err(error.into()));
    }

    /// The models [`SystemOne::list_models`] returns. Defaults to one model named
    /// [`DEFAULT_MODEL`].
    pub fn set_models(&self, models: ModelList) {
        self.state().models = Some(models);
    }

    /// Every request received that passed validation, oldest first.
    pub fn requests(&self) -> Vec<SystemOneRequest> {
        self.state().requests.clone()
    }

    /// The most recent request received that passed validation.
    pub fn last_request(&self) -> Option<SystemOneRequest> {
        self.state().requests.last().cloned()
    }

    /// How many requests passed validation.
    pub fn request_count(&self) -> usize {
        self.state().requests.len()
    }

    fn configure(&self, id: impl AsRef<str>, configured: Configured) {
        self.state()
            .configured
            .insert(id.as_ref().to_owned(), configured);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn respond(&self, request: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        request.validate()?;
        let mut state = self.state();
        state.requests.push(request.clone());
        let call_number = state.requests.len();
        let mut response = match state.scripted.pop_front() {
            Some(scripted) => scripted?,
            None => generate(&state.configured, request),
        };
        drop(state);

        response
            .request_id
            .get_or_insert_with(|| format!("fake-request-{call_number}"));
        response.verify(&request.questions)?;
        Ok(response)
    }
}

impl fmt::Debug for FakeSystemOne {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        f.debug_struct("FakeSystemOne")
            .field("configured_answers", &state.configured.len())
            .field("scripted", &state.scripted.len())
            .field("requests", &state.requests.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl SystemOne for FakeSystemOne {
    async fn send(
        &self,
        request: &SystemOneRequest,
        _options: &CallOptions,
    ) -> Result<SystemOneResponse, Error> {
        self.respond(request)
    }

    async fn list_models(&self) -> Result<ModelList, Error> {
        let configured = self.state().models.clone();
        Ok(configured.unwrap_or_else(|| ModelList {
            models: vec![ModelMetadata {
                name: DEFAULT_MODEL.to_owned(),
                description: "Fake System One model".to_owned(),
                release_date: "2026-01-01".to_owned(),
            }],
        }))
    }
}

fn generate(
    configured: &HashMap<String, Configured>,
    request: &SystemOneRequest,
) -> SystemOneResponse {
    let answers = request
        .questions
        .iter()
        .map(|(id, question)| {
            let answer = configured.get(id).map_or_else(
                || uncertain_answer(question),
                |configured| configured_answer(configured, question),
            );
            (id.clone(), answer)
        })
        .collect();
    SystemOneResponse {
        model: request
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
        answers,
        usage: Usage::default(),
        request_id: None,
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "option and level counts are small"
)]
fn uncertain_answer(question: &Question) -> Answer {
    match question {
        Question::Noul(_) => Answer::Noul(NoulAnswer {
            noul: UNCERTAIN_NOUL,
        }),
        Question::Choice(choice) => {
            let share = 1.0 / choice.criteria.len().max(1) as f64;
            choice_answer(
                choice
                    .criteria
                    .keys()
                    .map(|option| (option.clone(), share))
                    .collect(),
            )
        }
        Question::Score(score) => {
            let share = 1.0 / score.criteria.len().max(1) as f64;
            score_answer(&score.criteria, vec![share; score.criteria.len()])
        }
    }
}

fn configured_answer(configured: &Configured, question: &Question) -> Answer {
    match (configured, question) {
        (Configured::Exact(answer), _) => answer.clone(),
        (Configured::Noul(probability), _) => Answer::Noul(NoulAnswer { noul: *probability }),
        (Configured::Choice(set), Question::Choice(choice)) => {
            let probability_of = |option: &str| {
                set.iter()
                    .find(|(configured, _)| configured == option)
                    .map_or(0.0, |(_, probability)| *probability)
            };
            // Every option of the question, then any configured option it lacks, which
            // verification reports.
            let mut probabilities: Vec<(String, f64)> = choice
                .criteria
                .keys()
                .map(|option| (option.clone(), probability_of(option)))
                .collect();
            probabilities.extend(
                set.iter()
                    .filter(|(option, _)| !choice.criteria.contains_key(option))
                    .cloned(),
            );
            choice_answer(probabilities)
        }
        (Configured::Choice(set), _) => choice_answer(set.clone()),
        (Configured::Score(score), Question::Score(question)) => score_answer(
            &question.criteria,
            split_score(*score, question.criteria.len()),
        ),
        (Configured::Score(score), _) => Answer::Score(ScoreAnswer {
            score: *score,
            confidence: 1.0,
            legend: indexmap::IndexMap::new(),
            probabilities: indexmap::IndexMap::new(),
        }),
    }
}

fn choice_answer(probabilities: Vec<(String, f64)>) -> Answer {
    let mut best: Option<&(String, f64)> = None;
    for entry in &probabilities {
        if best.is_none_or(|(_, top)| entry.1 > *top) {
            best = Some(entry);
        }
    }
    Answer::Choice(ChoiceAnswer {
        choice: best.map(|(option, _)| option.clone()).unwrap_or_default(),
        confidence: confidence(probabilities.iter().map(|(_, probability)| *probability)),
        probabilities: probabilities.into_iter().collect(),
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "levels are at most ten and the score is clamped to them before the cast"
)]
fn split_score(score: f64, levels: usize) -> Vec<f64> {
    let mut probabilities = vec![0.0; levels];
    if levels == 0 {
        return probabilities;
    }
    let clamped = score.clamp(0.0, (levels - 1) as f64);
    let lower = clamped.floor();
    let upper_share = clamped - lower;
    let lower_level = lower as usize;
    probabilities[lower_level] = 1.0 - upper_share;
    if upper_share > 0.0 {
        probabilities[lower_level + 1] = upper_share;
    }
    probabilities
}

#[allow(clippy::cast_precision_loss, reason = "levels are at most ten")]
fn score_answer(levels: &[Content], probabilities: Vec<f64>) -> Answer {
    let score = probabilities
        .iter()
        .enumerate()
        .map(|(level, probability)| level as f64 * probability)
        .sum();
    Answer::Score(ScoreAnswer {
        score,
        confidence: confidence(probabilities.iter().copied()),
        legend: levels
            .iter()
            .enumerate()
            .map(|(level, description)| (level.to_string(), description.clone()))
            .collect(),
        probabilities: probabilities
            .into_iter()
            .enumerate()
            .map(|(level, probability)| (level.to_string(), probability))
            .collect(),
    })
}

/// 1 minus the distribution's entropy relative to an even spread: exactly 0 when even,
/// exactly 1 when certain.
#[allow(clippy::cast_precision_loss, reason = "option counts are at most 255")]
fn confidence(probabilities: impl Iterator<Item = f64>) -> f64 {
    const SNAP: f64 = 1e-9;
    let probabilities: Vec<f64> = probabilities.collect();
    if probabilities.len() < 2 {
        return 1.0;
    }
    let entropy: f64 = probabilities
        .iter()
        .filter(|probability| **probability > 0.0)
        .map(|probability| -probability * probability.ln())
        .sum();
    let confidence = (1.0 - entropy / (probabilities.len() as f64).ln()).clamp(0.0, 1.0);
    if confidence < SNAP {
        0.0
    } else if confidence > 1.0 - SNAP {
        1.0
    } else {
        confidence
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "the fake snaps confidence to exact endpoints"
)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_fractional_score_between_neighbouring_levels() {
        let probabilities = split_score(1.25, 4);

        assert_eq!(probabilities, [0.0, 0.75, 0.25, 0.0]);
        assert_eq!(split_score(7.0, 3), [0.0, 0.0, 1.0]);
        assert_eq!(split_score(-1.0, 2), [1.0, 0.0]);
    }

    #[test]
    fn derives_confidence_from_entropy() {
        assert_eq!(confidence([0.25, 0.25, 0.25, 0.25].into_iter()), 0.0);
        assert_eq!(confidence([1.0 / 3.0; 3].into_iter()), 0.0);
        assert_eq!(confidence([1.0, 0.0, 0.0].into_iter()), 1.0);
        let spread = confidence([0.8, 0.2].into_iter());
        assert!(spread > 0.0 && spread < 1.0);
    }
}
