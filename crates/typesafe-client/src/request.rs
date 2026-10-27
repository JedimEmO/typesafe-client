use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(feature = "images")]
use crate::{Assets, BoxAnswer, BoxQuestion};
use crate::{
    ChoiceAnswer, ChoiceQuestion, Content, NoulAnswer, NoulQuestion, Question, ReadAnswer,
    ScoreAnswer, ScoreQuestion, ValidationError,
};

/// Most options a single Choice question accepts.
pub const MAX_CHOICE_OPTIONS: usize = 255;
/// Fewest levels a Score question needs.
pub const MIN_SCORE_LEVELS: usize = 2;
/// Most levels a Score question accepts.
pub const MAX_SCORE_LEVELS: usize = 10;

/// A handle to a question in a [`Questions`] set, typed by how its answer is read.
///
/// Returned by [`Questions::add`] and passed to
/// [`SystemOneResponse::answer`](crate::SystemOneResponse::answer), so reading a Choice
/// answer as a Noul is a compile error rather than a runtime surprise.
pub struct QuestionKey<A> {
    id: String,
    answer: PhantomData<fn() -> A>,
}

impl<A> QuestionKey<A> {
    /// A key for an existing question id, for example when reading a stored response.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            answer: PhantomData,
        }
    }

    /// The question id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl<A> AsRef<str> for QuestionKey<A> {
    fn as_ref(&self) -> &str {
        &self.id
    }
}

impl<A> Clone for QuestionKey<A> {
    fn clone(&self) -> Self {
        Self::new(self.id.clone())
    }
}

impl<A> fmt::Debug for QuestionKey<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("QuestionKey").field(&self.id).finish()
    }
}

impl<A> PartialEq for QuestionKey<A> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<A> Eq for QuestionKey<A> {}

/// Serialized as the question id, so keys can be stored next to cached responses.
impl<A> Serialize for QuestionKey<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.id.serialize(serializer)
    }
}

impl<'de, A> Deserialize<'de> for QuestionKey<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}

impl<A> Hash for QuestionKey<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Something that can be added to [`Questions`], with the way its answer is read.
pub trait IntoQuestion {
    /// How the answer is read; the type parameter of the returned [`QuestionKey`].
    type Answer: ReadAnswer;

    /// The question as sent on the wire.
    fn into_question(self) -> Question;
}

impl IntoQuestion for NoulQuestion {
    type Answer = NoulAnswer;

    fn into_question(self) -> Question {
        Question::Noul(self)
    }
}

impl IntoQuestion for ChoiceQuestion {
    type Answer = ChoiceAnswer;

    fn into_question(self) -> Question {
        Question::Choice(self)
    }
}

impl IntoQuestion for ScoreQuestion {
    type Answer = ScoreAnswer;

    fn into_question(self) -> Question {
        Question::Score(self)
    }
}

#[cfg(feature = "images")]
impl IntoQuestion for BoxQuestion {
    type Answer = BoxAnswer;

    fn into_question(self) -> Question {
        Question::Box(self)
    }
}

/// The named questions of one request.
///
/// All questions see the same state and are answered independently and in parallel, so
/// ask every question about a state in one request, including speculative ones.
///
/// ```
/// use typesafe_client::{NoulQuestion, Questions, ScoreQuestion};
///
/// let mut questions = Questions::new();
/// let refund = questions.add("refund_requested", NoulQuestion::new("Is the user asking for a refund?"));
/// let anger = questions.add("frustration", ScoreQuestion::new("How frustrated is the user?", ["Calm", "Frustrated", "Very angry"]));
///
/// assert_eq!(questions.len(), 2);
/// assert_eq!(refund.id(), "refund_requested");
/// # let _ = anger;
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Questions {
    entries: IndexMap<String, Question>,
    duplicate_ids: Vec<String>,
}

impl Questions {
    /// An empty set of questions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `question` under `id` and returns a key that reads its answer with the
    /// matching type.
    ///
    /// The id is only used to match answers to questions; it is not sent to the model,
    /// so the question itself must carry its full meaning. Ids must be unique: adding a
    /// second question with the same id replaces the first, and [`Questions::validate`]
    /// reports the duplicate.
    pub fn add<Q: IntoQuestion>(
        &mut self,
        id: impl Into<String>,
        question: Q,
    ) -> QuestionKey<Q::Answer> {
        let id = id.into();
        if self
            .entries
            .insert(id.clone(), question.into_question())
            .is_some()
        {
            self.duplicate_ids.push(id.clone());
        }
        QuestionKey::new(id)
    }

    /// The question with `id`.
    pub fn get(&self, id: &str) -> Option<&Question> {
        self.entries.get(id)
    }

    /// Questions with their ids, in the order they were added.
    pub fn iter(&self) -> indexmap::map::Iter<'_, String, Question> {
        self.entries.iter()
    }

    /// Number of questions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no questions.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Checks the documented limits: at least one question, unique ids, 1 to 255 Choice
    /// options, and 2 to 10 Score levels.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(id) = self.duplicate_ids.first() {
            return Err(ValidationError::DuplicateQuestionId { id: id.clone() });
        }
        if self.entries.is_empty() {
            return Err(ValidationError::NoQuestions);
        }
        for (id, question) in &self.entries {
            match question {
                Question::Noul(_) => {}
                #[cfg(feature = "images")]
                Question::Box(_) => {}
                Question::Choice(choice) => match choice.criteria.len() {
                    0 => return Err(ValidationError::ChoiceWithoutOptions { id: id.clone() }),
                    count if count > MAX_CHOICE_OPTIONS => {
                        return Err(ValidationError::TooManyChoiceOptions {
                            id: id.clone(),
                            count,
                        });
                    }
                    _ => {}
                },
                Question::Score(score) => {
                    let count = score.criteria.len();
                    if !(MIN_SCORE_LEVELS..=MAX_SCORE_LEVELS).contains(&count) {
                        return Err(ValidationError::ScoreLevelCount {
                            id: id.clone(),
                            count,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &'a Questions {
    type Item = (&'a String, &'a Question);
    type IntoIter = indexmap::map::Iter<'a, String, Question>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Serialize for Questions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.entries.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Questions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        IndexMap::deserialize(deserializer).map(|entries| Self {
            entries,
            duplicate_ids: Vec::new(),
        })
    }
}

/// A System One request: the state and the questions to ask about it.
///
/// ```
/// use typesafe_client::{NoulQuestion, Questions, SystemOneRequest};
///
/// let mut questions = Questions::new();
/// questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
///
/// let request = SystemOneRequest::new("Help! My payouts have been failing.", questions)
///     .with_model("jev-latest");
/// request.validate()?;
/// # Ok::<(), typesafe_client::ValidationError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SystemOneRequest {
    /// The content every question refers to.
    pub state: Content,
    /// Model name or alias; `None` uses the transport's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The questions to ask about the state.
    pub questions: Questions,
    /// Images referenced from the state, instructions or criteria (feature `images`).
    #[cfg(feature = "images")]
    #[serde(default, skip_serializing_if = "Assets::is_empty")]
    pub assets: Assets,
}

impl SystemOneRequest {
    /// A request asking `questions` about `state`, using the transport's default model.
    pub fn new(state: impl Into<Content>, questions: Questions) -> Self {
        Self {
            state: state.into(),
            model: None,
            questions,
            #[cfg(feature = "images")]
            assets: Assets::new(),
        }
    }

    /// Sends `assets` with the request (feature `images`).
    #[cfg(feature = "images")]
    #[must_use]
    pub fn with_assets(mut self, assets: Assets) -> Self {
        self.assets = assets;
        self
    }

    /// Whether the request has images or Box questions, which the API only accepts with
    /// the `TypeSafe-Danger-Zone-Images: true` header. The HTTP client sends the header
    /// when this is true; custom transports should too.
    pub fn uses_images(&self) -> bool {
        #[cfg(feature = "images")]
        {
            !self.assets.is_empty()
                || self
                    .questions
                    .iter()
                    .any(|(_, question)| matches!(question, Question::Box(_)))
        }
        #[cfg(not(feature = "images"))]
        {
            false
        }
    }

    /// Uses `model` instead of the transport's default model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Checks the request against the documented API limits. With the `images` feature it
    /// also checks the images: each one is referenced, its reference is within the length
    /// limits, and its file is a PNG or JPEG of at most 16 MiB. Box questions need an image.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self
            .model
            .as_deref()
            .is_some_and(|model| model.trim().is_empty())
        {
            return Err(ValidationError::EmptyModel);
        }
        self.questions.validate()?;
        #[cfg(feature = "images")]
        {
            if self.assets.is_empty() {
                if let Some((id, _)) = self
                    .questions
                    .iter()
                    .find(|(_, question)| matches!(question, Question::Box(_)))
                {
                    return Err(ValidationError::BoxWithoutImage { id: id.clone() });
                }
            }
            self.assets.validate(&self.state, &self.questions)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn choice_with(count: usize) -> ChoiceQuestion {
        ChoiceQuestion::new("Which line?").with_bare_options((0..count).map(|i| format!("L{i:03}")))
    }

    fn score_with(count: usize) -> ScoreQuestion {
        ScoreQuestion::new("How much?", (0..count).map(|i| format!("level {i}")))
    }

    fn request_with(id: &str, question: impl IntoQuestion) -> SystemOneRequest {
        let mut questions = Questions::new();
        questions.add(id, question);
        SystemOneRequest::new("state", questions)
    }

    #[test]
    fn accepts_requests_at_documented_limits() {
        assert_eq!(request_with("c", choice_with(1)).validate(), Ok(()));
        assert_eq!(
            request_with("c", choice_with(MAX_CHOICE_OPTIONS)).validate(),
            Ok(())
        );
        assert_eq!(
            request_with("s", score_with(MIN_SCORE_LEVELS)).validate(),
            Ok(())
        );
        assert_eq!(
            request_with("s", score_with(MAX_SCORE_LEVELS)).validate(),
            Ok(())
        );
    }

    #[test]
    fn rejects_request_when_there_are_no_questions() {
        let request = SystemOneRequest::new("state", Questions::new());
        assert_eq!(request.validate(), Err(ValidationError::NoQuestions));
    }

    #[test]
    fn rejects_request_when_model_is_blank() {
        let request = request_with("q", NoulQuestion::new("?")).with_model("  ");
        assert_eq!(request.validate(), Err(ValidationError::EmptyModel));
    }

    #[test]
    fn omits_model_until_one_is_chosen() {
        let request = request_with("q", NoulQuestion::new("?"));
        assert_eq!(serde_json::to_value(&request).unwrap().get("model"), None);

        let pinned = request.with_model("jev-1.12");
        assert_eq!(
            serde_json::to_value(&pinned).unwrap()["model"],
            json!("jev-1.12")
        );
    }

    #[test]
    fn rejects_choice_when_it_has_no_options_or_too_many() {
        assert_eq!(
            request_with("c", choice_with(0)).validate(),
            Err(ValidationError::ChoiceWithoutOptions { id: "c".into() })
        );
        assert_eq!(
            request_with("c", choice_with(256)).validate(),
            Err(ValidationError::TooManyChoiceOptions {
                id: "c".into(),
                count: 256
            })
        );
    }

    #[test]
    fn rejects_score_when_level_count_is_out_of_range() {
        for count in [0, 1, 11] {
            assert_eq!(
                request_with("s", score_with(count)).validate(),
                Err(ValidationError::ScoreLevelCount {
                    id: "s".into(),
                    count
                })
            );
        }
    }

    #[test]
    fn reports_duplicate_when_question_id_is_added_twice() {
        let mut questions = Questions::new();
        questions.add("same", NoulQuestion::new("first"));
        questions.add("same", NoulQuestion::new("second"));

        assert_eq!(questions.len(), 1);
        assert_eq!(
            questions.validate(),
            Err(ValidationError::DuplicateQuestionId { id: "same".into() })
        );
    }

    #[test]
    fn serializes_questions_as_map_in_insertion_order() {
        let mut questions = Questions::new();
        questions.add("z_last_alphabetically", NoulQuestion::new("a"));
        questions.add("a_first_alphabetically", NoulQuestion::new("b"));

        let ids: Vec<_> = questions.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["z_last_alphabetically", "a_first_alphabetically"]);
        assert_eq!(
            serde_json::to_string(&questions).unwrap(),
            r#"{"z_last_alphabetically":{"type":"noul","instructions":"a"},"a_first_alphabetically":{"type":"noul","instructions":"b"}}"#
        );
    }

    #[test]
    fn serializes_keys_as_their_question_id() {
        let key = QuestionKey::<ChoiceAnswer>::new("department");

        let json = serde_json::to_string(&key).unwrap();
        let restored: QuestionKey<ChoiceAnswer> = serde_json::from_str(&json).unwrap();

        assert_eq!(json, r#""department""#);
        assert_eq!(restored, key);
    }

    #[test]
    fn keys_compare_by_id_and_expose_it() {
        let key = QuestionKey::<NoulAnswer>::new("q");
        assert_eq!(key.clone(), QuestionKey::new("q"));
        assert_eq!(key.as_ref(), "q");
        assert_eq!(format!("{key:?}"), r#"QuestionKey("q")"#);
    }

    #[cfg(feature = "images")]
    mod images {
        use super::*;
        use crate::{BoxQuestion, Image};

        const PNG: &[u8] = include_bytes!("../tests/fixtures/red-rectangle.png");

        #[test]
        fn rejects_box_question_when_request_has_no_image() {
            let request = request_with("mug", BoxQuestion::new("the mug in `photo`"));

            assert!(request.uses_images());
            assert_eq!(
                request.validate(),
                Err(ValidationError::BoxWithoutImage { id: "mug".into() })
            );
        }

        #[test]
        fn sends_assets_only_when_there_are_some() {
            let plain = request_with("q", NoulQuestion::new("?"));
            assert!(!plain.uses_images());
            assert_eq!(serde_json::to_value(&plain).unwrap().get("assets"), None);

            let mut assets = Assets::new();
            let photo = assets.insert("img-1", Image::png(PNG.to_vec()));
            let mut questions = Questions::new();
            questions.add("mug", BoxQuestion::new("the mug in `photo`"));
            let request =
                SystemOneRequest::new(json!({"photo": photo}), questions).with_assets(assets);

            assert!(request.uses_images());
            assert_eq!(request.validate(), Ok(()));
            let wire = serde_json::to_value(&request).unwrap();
            assert_eq!(wire["assets"]["img-1"]["format"], "png");
            let restored: SystemOneRequest = serde_json::from_value(wire).unwrap();
            assert_eq!(restored, request);
        }
    }
}
