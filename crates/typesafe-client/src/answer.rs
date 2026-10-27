use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{AnswerError, Content, Question, QuestionKey, QuestionKind, Questions};

/// A typed answer, returned under the id of the question it answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum Answer {
    /// Answer to a Noul question.
    Noul(NoulAnswer),
    /// Answer to a Choice question.
    Choice(ChoiceAnswer),
    /// Answer to a Score question.
    Score(ScoreAnswer),
    /// Answer to a Box question (feature `images`).
    #[cfg(feature = "images")]
    Box(BoxAnswer),
}

impl Answer {
    /// The kind of question this answers.
    pub const fn kind(&self) -> QuestionKind {
        match self {
            Self::Noul(_) => QuestionKind::Noul,
            Self::Choice(_) => QuestionKind::Choice,
            Self::Score(_) => QuestionKind::Score,
            #[cfg(feature = "images")]
            Self::Box(_) => QuestionKind::Box,
        }
    }
}

impl From<NoulAnswer> for Answer {
    fn from(answer: NoulAnswer) -> Self {
        Self::Noul(answer)
    }
}

impl From<ChoiceAnswer> for Answer {
    fn from(answer: ChoiceAnswer) -> Self {
        Self::Choice(answer)
    }
}

impl From<ScoreAnswer> for Answer {
    fn from(answer: ScoreAnswer) -> Self {
        Self::Score(answer)
    }
}

#[cfg(feature = "images")]
impl From<BoxAnswer> for Answer {
    fn from(answer: BoxAnswer) -> Self {
        Self::Box(answer)
    }
}

/// The answer to a [`NoulQuestion`](crate::NoulQuestion).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NoulAnswer {
    /// Probability of yes, from 0 (no) to 1 (yes). Near 0.5 means uncertain, not "medium".
    pub noul: f64,
}

impl NoulAnswer {
    /// An answer with probability of yes `noul`, for fixtures and tests.
    pub fn new(noul: f64) -> Self {
        Self { noul }
    }
}

/// The answer to a [`ChoiceQuestion`](crate::ChoiceQuestion).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChoiceAnswer {
    /// The option with the highest probability.
    pub choice: String,
    /// How concentrated the distribution is, from 0 to 1.
    pub confidence: f64,
    /// Probability of every option; values sum to approximately 1.
    pub probabilities: IndexMap<String, f64>,
}

impl ChoiceAnswer {
    /// An answer choosing `choice`, for fixtures and tests.
    pub fn new(
        choice: impl Into<String>,
        confidence: f64,
        probabilities: impl IntoIterator<Item = (impl Into<String>, f64)>,
    ) -> Self {
        Self {
            choice: choice.into(),
            confidence,
            probabilities: probabilities
                .into_iter()
                .map(|(option, probability)| (option.into(), probability))
                .collect(),
        }
    }

    /// The probability of `option`, if the answer includes it.
    pub fn probability(&self, option: &str) -> Option<f64> {
        self.probabilities.get(option).copied()
    }

    /// Options from most to least probable.
    pub fn ranked(&self) -> Vec<(&str, f64)> {
        let mut ranked: Vec<_> = self
            .probabilities
            .iter()
            .map(|(option, probability)| (option.as_str(), *probability))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked
    }

    /// The `n` most probable options, most probable first.
    pub fn top(&self, n: usize) -> Vec<(&str, f64)> {
        let mut ranked = self.ranked();
        ranked.truncate(n);
        ranked
    }
}

/// The answer to a [`ScoreQuestion`](crate::ScoreQuestion).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScoreAnswer {
    /// Probability-weighted level, from 0 to the top level; can land between levels.
    pub score: f64,
    /// How concentrated the distribution is, from 0 to 1.
    pub confidence: f64,
    /// Level number (as a string key) to the level's description.
    pub legend: IndexMap<String, Content>,
    /// Level number (as a string key) to its probability.
    pub probabilities: IndexMap<String, f64>,
}

impl ScoreAnswer {
    /// An answer over `levels` (in order) with one probability per level, for fixtures
    /// and tests.
    pub fn new(
        score: f64,
        confidence: f64,
        levels: impl IntoIterator<Item = impl Into<Content>>,
        probabilities: impl IntoIterator<Item = f64>,
    ) -> Self {
        Self {
            score,
            confidence,
            legend: levels
                .into_iter()
                .enumerate()
                .map(|(level, description)| (level.to_string(), description.into()))
                .collect(),
            probabilities: probabilities
                .into_iter()
                .enumerate()
                .map(|(level, probability)| (level.to_string(), probability))
                .collect(),
        }
    }

    /// Number of levels the question had.
    pub fn levels(&self) -> usize {
        self.legend.len().max(self.probabilities.len())
    }

    /// Probability of each level, indexed by level number. Levels the response has no
    /// probability for count as 0.
    pub fn level_probabilities(&self) -> Vec<f64> {
        let mut levels = vec![0.0; self.levels()];
        for (key, probability) in &self.probabilities {
            if let Some(slot) = key.parse::<usize>().ok().and_then(|i| levels.get_mut(i)) {
                *slot = *probability;
            }
        }
        levels
    }

    /// The single most probable level, or `None` when there are no levels.
    pub fn most_likely_level(&self) -> Option<usize> {
        self.level_probabilities()
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(level, _)| level)
    }

    /// The score rounded to the nearest level, for code that needs one outcome.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "levels are at most ten and the value is clamped to them before the cast"
    )]
    pub fn nearest_level(&self) -> usize {
        let top = self.levels().saturating_sub(1);
        self.score.round().clamp(0.0, top as f64) as usize
    }

    /// The score divided by the top level, from 0 to 1, so scores with different numbers
    /// of levels can be weighted and combined.
    #[allow(clippy::cast_precision_loss, reason = "levels are at most ten")]
    pub fn normalized(&self) -> f64 {
        match self.levels().saturating_sub(1) {
            0 => 0.0,
            top => (self.score / top as f64).clamp(0.0, 1.0),
        }
    }

    /// The description of `level`, as the question defined it.
    pub fn level_description(&self, level: usize) -> Option<&Content> {
        self.legend.get(&level.to_string())
    }
}

/// The answer to a [`BoxQuestion`](crate::BoxQuestion): where the target is in the image.
///
/// There is no "not found" answer: a Box about something absent still returns a box. Ask a
/// Noul in the same request to check that the target is there.
#[cfg(feature = "images")]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BoxAnswer {
    /// The box's edges as fractions of the image's size. Sent as `box`.
    #[serde(rename = "box")]
    pub bounds: BoxCoordinates,
    /// How sure the model is of the box, from 0 to 1; `None` when the model gives no
    /// confidence. There is no recommended threshold yet, so tune one on your own images.
    pub confidence: Option<f64>,
}

#[cfg(feature = "images")]
impl BoxAnswer {
    /// An answer locating `bounds`, for fixtures and tests.
    pub fn new(bounds: BoxCoordinates, confidence: Option<f64>) -> Self {
        Self { bounds, confidence }
    }
}

/// A box's edges as fractions of the image's size, measured from the top-left corner.
///
/// `left` and `right` are fractions of the width, `top` and `bottom` of the height, so the
/// same box works at any image size. Each edge is from 0 to 1, with `left < right` and
/// `top < bottom`.
///
/// ```
/// use typesafe_client::BoxCoordinates;
///
/// let mug = BoxCoordinates::new(0.25, 0.82, 0.44, 0.84);
/// let pixels = mug.to_pixels(1568, 1176);
/// assert_eq!((pixels.left, pixels.top, pixels.right, pixels.bottom), (690, 294, 1317, 964));
///
/// // Before cropping, move each edge out by 5% of the image, staying inside it.
/// let padded = mug.padded(0.05);
/// assert_eq!(padded.top, 0.2);
/// ```
#[cfg(feature = "images")]
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BoxCoordinates {
    /// Top edge, as a fraction of the image height from the top.
    pub top: f64,
    /// Bottom edge, as a fraction of the image height from the top.
    pub bottom: f64,
    /// Left edge, as a fraction of the image width from the left.
    pub left: f64,
    /// Right edge, as a fraction of the image width from the left.
    pub right: f64,
}

#[cfg(feature = "images")]
impl BoxCoordinates {
    /// The whole image.
    pub const FULL_IMAGE: Self = Self {
        top: 0.0,
        bottom: 1.0,
        left: 0.0,
        right: 1.0,
    };

    /// A box with these edges, in the API's order: top, bottom, left, right.
    pub fn new(top: f64, bottom: f64, left: f64, right: f64) -> Self {
        Self {
            top,
            bottom,
            left,
            right,
        }
    }

    /// Width as a fraction of the image width.
    pub fn width(&self) -> f64 {
        (self.right - self.left).max(0.0)
    }

    /// Height as a fraction of the image height.
    pub fn height(&self) -> f64 {
        (self.bottom - self.top).max(0.0)
    }

    /// Area as a fraction of the image area.
    pub fn area(&self) -> f64 {
        self.width() * self.height()
    }

    /// The centre point as `(x, y)` fractions of the image width and height.
    pub fn center(&self) -> (f64, f64) {
        (
            f64::midpoint(self.left, self.right),
            f64::midpoint(self.top, self.bottom),
        )
    }

    /// The box with each edge moved out by `fraction` of the image size, clamped to the
    /// image. Pad a box before cropping when the crop must not cut off part of the target.
    #[must_use]
    pub fn padded(&self, fraction: f64) -> Self {
        Self {
            top: (self.top - fraction).clamp(0.0, 1.0),
            bottom: (self.bottom + fraction).clamp(0.0, 1.0),
            left: (self.left - fraction).clamp(0.0, 1.0),
            right: (self.right + fraction).clamp(0.0, 1.0),
        }
    }

    /// The box in pixels of an image `width` by `height` pixels, each edge rounded to the
    /// nearest pixel and clamped to the image.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each edge is clamped to the image size before the cast"
    )]
    pub fn to_pixels(&self, width: u32, height: u32) -> PixelBox {
        let scale = |fraction: f64, size: u32| {
            (fraction * f64::from(size))
                .round()
                .clamp(0.0, f64::from(size)) as u32
        };
        PixelBox {
            left: scale(self.left, width),
            top: scale(self.top, height),
            right: scale(self.right, width),
            bottom: scale(self.bottom, height),
        }
    }
}

/// A box in pixels, from [`BoxCoordinates::to_pixels`]. `right` and `bottom` are exclusive,
/// as crop functions expect.
#[cfg(feature = "images")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct PixelBox {
    /// Left edge, in pixels from the left.
    pub left: u32,
    /// Top edge, in pixels from the top.
    pub top: u32,
    /// Right edge, in pixels from the left.
    pub right: u32,
    /// Bottom edge, in pixels from the top.
    pub bottom: u32,
}

#[cfg(feature = "images")]
impl PixelBox {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.right.saturating_sub(self.left)
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }
}

/// Token usage for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Usage {
    /// Billable input tokens: the state and the questions.
    pub input_tokens: u64,
    /// Output tokens; currently free of charge.
    pub output_tokens: u64,
}

impl Usage {
    /// Token counts, for fixtures and tests.
    pub fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }
}

/// The response to a System One request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SystemOneResponse {
    /// The model that answered; may differ from the alias that was requested.
    pub model: String,
    /// Answers keyed by question id.
    pub answers: IndexMap<String, Answer>,
    /// Token usage for the request.
    pub usage: Usage,
    /// The `x-typesafe-request-id` header, when the response was received over HTTP.
    /// Serialized only when present, so stored responses keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl SystemOneResponse {
    /// An empty response from `model`, for fixtures and tests. Add answers with
    /// [`SystemOneResponse::with_answer`].
    ///
    /// ```
    /// use typesafe_client::{NoulAnswer, NoulQuestion, Questions, SystemOneResponse, Usage};
    ///
    /// let mut questions = Questions::new();
    /// let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
    ///
    /// let response = SystemOneResponse::new("jev-latest", Usage::new(312, 48))
    ///     .with_answer(&urgent, NoulAnswer::new(0.92));
    /// assert_eq!(response.answer(&urgent)?.noul, 0.92);
    /// # Ok::<(), typesafe_client::AnswerError>(())
    /// ```
    pub fn new(model: impl Into<String>, usage: Usage) -> Self {
        Self {
            model: model.into(),
            answers: IndexMap::new(),
            usage,
            request_id: None,
        }
    }

    /// Adds or replaces the answer for question `id` (an id string or a [`QuestionKey`]).
    #[must_use]
    pub fn with_answer(mut self, id: impl AsRef<str>, answer: impl Into<Answer>) -> Self {
        self.answers.insert(id.as_ref().to_owned(), answer.into());
        self
    }

    /// Sets the request id, as a transport would from the response headers.
    #[must_use]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    /// Reads the answer for `key` with the type its question promises.
    ///
    /// Noul answers are copied, Choice and Score answers are borrowed, and typed choices
    /// are parsed into their Rust type.
    ///
    /// ```
    /// use typesafe_client::{NoulQuestion, Questions, SystemOneResponse};
    ///
    /// let mut questions = Questions::new();
    /// let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
    ///
    /// let response: SystemOneResponse = serde_json::from_str(
    ///     r#"{"model":"jev-latest","answers":{"is_urgent":{"type":"noul","noul":0.92}},
    ///        "usage":{"input_tokens":312,"output_tokens":48}}"#,
    /// )?;
    /// assert_eq!(response.answer(&urgent)?.noul, 0.92);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn answer<A: ReadAnswer>(
        &self,
        key: &QuestionKey<A>,
    ) -> Result<A::Output<'_>, AnswerError> {
        let answer = self
            .answers
            .get(key.id())
            .ok_or_else(|| AnswerError::Missing {
                id: key.id().to_owned(),
            })?;
        A::read(key.id(), answer)
    }

    /// Checks that every question has an answer of its kind, naming only options and levels
    /// the question has. Transports run this before returning a response, so reading an
    /// answer can then only fail when a key belongs to a different request.
    pub fn verify(&self, questions: &Questions) -> Result<(), AnswerError> {
        for (id, question) in questions {
            let answer = self
                .answers
                .get(id)
                .ok_or_else(|| AnswerError::Missing { id: id.clone() })?;
            if answer.kind() != question.kind() {
                return Err(AnswerError::WrongKind {
                    id: id.clone(),
                    expected: question.kind(),
                    found: answer.kind(),
                });
            }
            match (question, answer) {
                (Question::Choice(question), Answer::Choice(answer)) => {
                    let named = std::iter::once(&answer.choice).chain(answer.probabilities.keys());
                    if let Some(option) = named
                        .into_iter()
                        .find(|option| !question.criteria.contains_key(*option))
                    {
                        return Err(AnswerError::UnknownOption {
                            id: id.clone(),
                            option: option.clone(),
                        });
                    }
                }
                (Question::Score(question), Answer::Score(answer)) => {
                    let levels = question.criteria.len();
                    if let Some(level) = answer
                        .probabilities
                        .keys()
                        .find(|level| level.parse::<usize>().map_or(true, |level| level >= levels))
                    {
                        return Err(AnswerError::UnknownLevel {
                            id: id.clone(),
                            level: level.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// A model available to the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelMetadata {
    /// Name or alias accepted by a request's `model` field.
    pub name: String,
    /// Human-readable description of the model.
    pub description: String,
    /// When the model was released, as sent by the server.
    ///
    /// The OpenAPI spec documents `YYYY-MM-DD` (for example `2026-09-15`), but the live
    /// API returns an RFC 3339 timestamp (`2026-09-10T18:38:01.391457+00:00`, observed
    /// 2026-09-16). Both begin with the calendar date, so read that prefix rather than
    /// relying on either format.
    pub release_date: String,
}

impl ModelMetadata {
    /// Model metadata, for fixtures and tests.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        release_date: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            release_date: release_date.into(),
        }
    }
}

/// The response to `GET /v1/models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelList {
    /// Available models and aliases.
    pub models: Vec<ModelMetadata>,
}

impl ModelList {
    /// A model list, for fixtures and tests.
    pub fn new(models: impl IntoIterator<Item = ModelMetadata>) -> Self {
        Self {
            models: models.into_iter().collect(),
        }
    }
}

/// How the answer behind a [`QuestionKey`] is read from a response.
///
/// Implemented for [`NoulAnswer`], [`ChoiceAnswer`], [`ScoreAnswer`], `BoxAnswer`, [`Answer`],
/// [`TypedChoice`](crate::TypedChoice) and [`ValueChoice`](crate::ValueChoice). Implement it
/// for your own question types together with [`IntoQuestion`](crate::IntoQuestion).
pub trait ReadAnswer {
    /// What [`SystemOneResponse::answer`] returns for this key.
    type Output<'a>;

    /// Reads the answer to question `id`, failing when it has the wrong kind or content.
    fn read<'a>(id: &str, answer: &'a Answer) -> Result<Self::Output<'a>, AnswerError>;
}

impl ReadAnswer for Answer {
    type Output<'a> = &'a Answer;

    fn read<'a>(_id: &str, answer: &'a Answer) -> Result<&'a Answer, AnswerError> {
        Ok(answer)
    }
}

impl ReadAnswer for NoulAnswer {
    type Output<'a> = NoulAnswer;

    fn read(id: &str, answer: &Answer) -> Result<NoulAnswer, AnswerError> {
        match answer {
            Answer::Noul(noul) => Ok(*noul),
            other => Err(wrong_kind(id, QuestionKind::Noul, other)),
        }
    }
}

impl ReadAnswer for ChoiceAnswer {
    type Output<'a> = &'a ChoiceAnswer;

    fn read<'a>(id: &str, answer: &'a Answer) -> Result<&'a ChoiceAnswer, AnswerError> {
        match answer {
            Answer::Choice(choice) => Ok(choice),
            other => Err(wrong_kind(id, QuestionKind::Choice, other)),
        }
    }
}

impl ReadAnswer for ScoreAnswer {
    type Output<'a> = &'a ScoreAnswer;

    fn read<'a>(id: &str, answer: &'a Answer) -> Result<&'a ScoreAnswer, AnswerError> {
        match answer {
            Answer::Score(score) => Ok(score),
            other => Err(wrong_kind(id, QuestionKind::Score, other)),
        }
    }
}

#[cfg(feature = "images")]
impl ReadAnswer for BoxAnswer {
    type Output<'a> = BoxAnswer;

    fn read(id: &str, answer: &Answer) -> Result<BoxAnswer, AnswerError> {
        match answer {
            Answer::Box(found) => Ok(*found),
            other => Err(wrong_kind(id, QuestionKind::Box, other)),
        }
    }
}

fn wrong_kind(id: &str, expected: QuestionKind, found: &Answer) -> AnswerError {
    AnswerError::WrongKind {
        id: id.to_owned(),
        expected,
        found: found.kind(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{ChoiceQuestion, NoulQuestion, ScoreQuestion};

    fn response(answers: &serde_json::Value) -> SystemOneResponse {
        serde_json::from_value(json!({
            "model": "jev-latest",
            "answers": answers,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }))
        .unwrap()
    }

    fn score(score: f64, probabilities: &[f64]) -> ScoreAnswer {
        ScoreAnswer {
            score,
            confidence: 0.5,
            legend: (0..probabilities.len())
                .map(|level| (level.to_string(), Content::from(format!("level {level}"))))
                .collect(),
            probabilities: probabilities
                .iter()
                .enumerate()
                .map(|(level, p)| (level.to_string(), *p))
                .collect(),
        }
    }

    fn triage_questions() -> Questions {
        let mut questions = Questions::new();
        questions.add("urgent", NoulQuestion::new("Urgent?"));
        questions.add(
            "team",
            ChoiceQuestion::new("Which team?").with_bare_options(["billing", "technical"]),
        );
        questions.add("anger", ScoreQuestion::new("How angry?", ["calm", "angry"]));
        questions
    }

    #[test]
    fn returns_missing_error_when_response_has_no_answer_for_key() {
        let response = response(&json!({"other": {"type": "noul", "noul": 0.1}}));
        let key = QuestionKey::<NoulAnswer>::new("is_urgent");

        assert_eq!(
            response.answer(&key),
            Err(AnswerError::Missing {
                id: "is_urgent".into()
            })
        );
    }

    #[test]
    fn returns_wrong_kind_error_when_answer_type_differs_from_key() {
        let response = response(&json!({"q": {"type": "noul", "noul": 0.1}}));
        let key = QuestionKey::<ChoiceAnswer>::new("q");

        assert_eq!(
            response.answer(&key),
            Err(AnswerError::WrongKind {
                id: "q".into(),
                expected: QuestionKind::Choice,
                found: QuestionKind::Noul
            })
        );
    }

    #[test]
    fn borrows_choice_answers_so_rankings_can_be_chained() {
        let response = response(&json!({"line": {
            "type": "choice",
            "choice": "L2",
            "confidence": 0.8,
            "probabilities": {"L0": 0.05, "L1": 0.15, "L2": 0.8}
        }}));
        let key = QuestionKey::<ChoiceAnswer>::new("line");

        let top: Vec<&str> = response
            .answer(&key)
            .unwrap()
            .top(2)
            .into_iter()
            .map(|(option, _)| option)
            .collect();

        assert_eq!(top, ["L2", "L1"]);
    }

    #[test]
    fn ranks_choice_options_by_descending_probability() {
        let answer = ChoiceAnswer {
            choice: "technical".into(),
            confidence: 0.82,
            probabilities: [("billing", 0.08), ("technical", 0.85), ("sales", 0.07)]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        };

        assert_eq!(
            answer.ranked(),
            [("technical", 0.85), ("billing", 0.08), ("sales", 0.07)]
        );
        assert_eq!(answer.top(1), [("technical", 0.85)]);
    }

    #[test]
    fn orders_score_probabilities_by_level_number() {
        let response = response(&json!({"f": {
            "type": "score",
            "score": 1.6,
            "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
            "probabilities": {"2": 0.65, "0": 0.05, "1": 0.3},
            "confidence": 0.78
        }}));
        let score = response
            .answer(&QuestionKey::<ScoreAnswer>::new("f"))
            .unwrap();

        assert_eq!(score.level_probabilities(), [0.05, 0.3, 0.65]);
        assert_eq!(score.most_likely_level(), Some(2));
        assert_eq!(
            score.level_description(1),
            Some(&Content::from("Frustrated"))
        );
    }

    #[test]
    fn treats_absent_levels_as_zero_probability() {
        let mut answer = score(0.0, &[1.0, 0.0]);
        answer.probabilities.shift_remove("1");

        assert_eq!(answer.level_probabilities(), [1.0, 0.0]);
    }

    #[test]
    fn rounds_and_normalizes_scores_against_the_top_level() {
        let four_levels = score(1.3, &[0.0, 0.7, 0.3, 0.0]);

        assert_eq!(four_levels.nearest_level(), 1);
        assert!((four_levels.normalized() - 1.3 / 3.0).abs() < 1e-12);
        assert_eq!(score(2.6, &[0.0, 0.0, 0.4, 0.6]).nearest_level(), 3);
        assert_eq!(score(9.0, &[0.5, 0.5]).nearest_level(), 1);
        assert!((score(9.0, &[0.5, 0.5]).normalized() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn verifies_a_complete_matching_response() {
        let response = response(&json!({
            "urgent": {"type": "noul", "noul": 0.9},
            "team": {"type": "choice", "choice": "billing", "confidence": 0.9, "probabilities": {"billing": 0.95, "technical": 0.05}},
            "anger": {"type": "score", "score": 0.2, "confidence": 0.6, "legend": {"0": "calm", "1": "angry"}, "probabilities": {"0": 0.8, "1": 0.2}},
            "unasked": {"type": "noul", "noul": 0.5}
        }));

        assert_eq!(response.verify(&triage_questions()), Ok(()));
    }

    #[test]
    fn rejects_responses_that_do_not_answer_the_questions() {
        let missing = response(&json!({"urgent": {"type": "noul", "noul": 0.9}}));
        assert_eq!(
            missing.verify(&triage_questions()),
            Err(AnswerError::Missing { id: "team".into() })
        );

        let unknown_option = response(&json!({
            "urgent": {"type": "noul", "noul": 0.9},
            "team": {"type": "choice", "choice": "sales", "confidence": 1.0, "probabilities": {"sales": 1.0}},
            "anger": {"type": "score", "score": 0.0, "confidence": 1.0, "legend": {}, "probabilities": {"0": 1.0}}
        }));
        assert_eq!(
            unknown_option.verify(&triage_questions()),
            Err(AnswerError::UnknownOption {
                id: "team".into(),
                option: "sales".into()
            })
        );

        let unknown_level = response(&json!({
            "urgent": {"type": "noul", "noul": 0.9},
            "team": {"type": "choice", "choice": "billing", "confidence": 1.0, "probabilities": {"billing": 1.0}},
            "anger": {"type": "score", "score": 2.0, "confidence": 1.0, "legend": {}, "probabilities": {"2": 1.0}}
        }));
        assert_eq!(
            unknown_level.verify(&triage_questions()),
            Err(AnswerError::UnknownLevel {
                id: "anger".into(),
                level: "2".into()
            })
        );
    }

    #[test]
    fn keeps_the_request_id_when_a_stored_response_round_trips() {
        let response = SystemOneResponse::new("jev-latest", Usage::new(10, 2))
            .with_answer("q", NoulAnswer::new(0.3))
            .with_request_id("req_42");

        let stored = serde_json::to_string(&response).unwrap();
        let restored: SystemOneResponse = serde_json::from_str(&stored).unwrap();

        assert_eq!(restored, response);
        assert_eq!(restored.request_id.as_deref(), Some("req_42"));
    }

    #[test]
    fn builds_fixture_answers_that_read_like_api_answers() {
        let score = ScoreAnswer::new(1.3, 0.6, ["low", "mid", "high"], [0.0, 0.7, 0.3]);
        let choice = ChoiceAnswer::new("b", 0.8, [("a", 0.1), ("b", 0.9)]);

        assert_eq!(score.level_description(2), Some(&Content::from("high")));
        assert_eq!(score.level_probabilities(), [0.0, 0.7, 0.3]);
        assert_eq!(choice.top(1), [("b", 0.9)]);
        assert_eq!(Answer::from(choice).kind(), QuestionKind::Choice);
    }

    #[test]
    fn accepts_model_list_with_timestamp_release_dates_as_returned_by_live_api() {
        let models: ModelList = serde_json::from_value(json!({"models": [{
            "name": "jev-latest",
            "description": "The latest iteration of TypeSafe's System One Model: Jev",
            "release_date": "2026-09-10T18:38:01.391457+00:00"
        }]}))
        .unwrap();

        assert_eq!(
            models.models[0].release_date,
            "2026-09-10T18:38:01.391457+00:00"
        );
    }

    #[test]
    fn ignores_unknown_response_fields() {
        let response: SystemOneResponse = serde_json::from_value(json!({
            "model": "jev-latest",
            "answers": {"q": {"type": "noul", "noul": 1, "extra": true}},
            "usage": {"input_tokens": 5, "output_tokens": 2, "cached_tokens": 0},
            "trace": "ignored"
        }))
        .unwrap();

        let noul = response
            .answer(&QuestionKey::<NoulAnswer>::new("q"))
            .unwrap()
            .noul;
        assert!((noul - 1.0).abs() < f64::EPSILON);
    }

    #[cfg(feature = "images")]
    #[test]
    fn reads_box_answers_with_and_without_confidence() {
        let response = response(&json!({
            "mug": {"type": "box", "box": {"top": 0.25, "bottom": 0.82, "left": 0.44, "right": 0.84}, "confidence": 0.92},
            "red": {"type": "box", "box": {"top": 0.2, "bottom": 0.66, "left": 0.19, "right": 0.68}, "confidence": null}
        }));

        let mug = response
            .answer(&QuestionKey::<BoxAnswer>::new("mug"))
            .unwrap();
        let red = response
            .answer(&QuestionKey::<BoxAnswer>::new("red"))
            .unwrap();

        assert_eq!(mug.bounds, BoxCoordinates::new(0.25, 0.82, 0.44, 0.84));
        assert_eq!(mug.confidence, Some(0.92));
        assert_eq!(red.confidence, None);
        assert_eq!(
            serde_json::to_value(Answer::from(mug)).unwrap()["box"]["left"],
            json!(0.44)
        );
    }

    #[cfg(feature = "images")]
    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "edges of quarters and halves are exact in binary"
    )]
    fn measures_boxes_and_converts_them_to_pixels() {
        let mug = BoxCoordinates::new(0.25, 0.75, 0.5, 1.0);

        assert_eq!(mug.width(), 0.5);
        assert_eq!(mug.height(), 0.5);
        assert_eq!(mug.area(), 0.25);
        assert_eq!(mug.center(), (0.75, 0.5));
        let pixels = mug.to_pixels(400, 300);
        assert_eq!(
            pixels,
            PixelBox {
                left: 200,
                top: 75,
                right: 400,
                bottom: 225
            }
        );
        assert_eq!((pixels.width(), pixels.height()), (200, 150));
    }

    #[cfg(feature = "images")]
    #[test]
    fn keeps_padded_and_pixel_boxes_inside_the_image() {
        let edge = BoxCoordinates::new(0.02, 0.99, 0.0, 0.5);

        assert_eq!(edge.padded(0.05), BoxCoordinates::new(0.0, 1.0, 0.0, 0.55));
        let overshoot = BoxCoordinates::new(-0.1, 1.2, -0.1, 1.2);
        assert_eq!(
            overshoot.to_pixels(10, 20),
            PixelBox {
                left: 0,
                top: 0,
                right: 10,
                bottom: 20
            }
        );
    }
}
