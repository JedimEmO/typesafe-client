use std::fmt;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::Content;

/// The three TypeSafe question types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum QuestionKind {
    /// Yes/no; answered with the probability of yes.
    Noul,
    /// One of a set of options.
    Choice,
    /// A position on ordered levels.
    Score,
}

impl QuestionKind {
    /// The `type` value used on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }
}

impl fmt::Display for QuestionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A typed question, as sent in the `questions` map of a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum Question {
    /// A yes/no question.
    Noul(NoulQuestion),
    /// A question that picks one option.
    Choice(ChoiceQuestion),
    /// A question that rates on ordered levels.
    Score(ScoreQuestion),
}

impl Question {
    /// The kind of this question.
    pub const fn kind(&self) -> QuestionKind {
        match self {
            Self::Noul(_) => QuestionKind::Noul,
            Self::Choice(_) => QuestionKind::Choice,
            Self::Score(_) => QuestionKind::Score,
        }
    }
}

impl From<NoulQuestion> for Question {
    fn from(question: NoulQuestion) -> Self {
        Self::Noul(question)
    }
}

impl From<ChoiceQuestion> for Question {
    fn from(question: ChoiceQuestion) -> Self {
        Self::Choice(question)
    }
}

impl From<ScoreQuestion> for Question {
    fn from(question: ScoreQuestion) -> Self {
        Self::Score(question)
    }
}

/// A yes/no question. The answer is the probability that the answer is yes.
///
/// ```
/// use typesafe_client::NoulQuestion;
///
/// let question = NoulQuestion::new("Does this convey urgency?")
///     .with_criteria("Explicitly time-sensitive", "No urgency expressed");
/// # let _ = question;
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NoulQuestion {
    /// The yes/no question or statement to evaluate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Content>,
    /// What counts as a yes or a no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

impl NoulQuestion {
    /// A yes/no question asking `instructions`.
    pub fn new(instructions: impl Into<Content>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria: None,
        }
    }

    /// Describes what a yes (near 1) and a no (near 0) mean.
    #[must_use]
    pub fn with_criteria(mut self, yes: impl Into<Content>, no: impl Into<Content>) -> Self {
        self.criteria = Some(NoulCriteria::new(yes, no));
        self
    }
}

/// What counts as a yes or a no for a [`NoulQuestion`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NoulCriteria {
    /// What a yes (near 1) means. Sent as `true`.
    #[serde(rename = "true", default, skip_serializing_if = "Option::is_none")]
    pub yes: Option<Content>,
    /// What a no (near 0) means. Sent as `false`.
    #[serde(rename = "false", default, skip_serializing_if = "Option::is_none")]
    pub no: Option<Content>,
}

impl NoulCriteria {
    /// Criteria describing both a yes and a no.
    pub fn new(yes: impl Into<Content>, no: impl Into<Content>) -> Self {
        Self {
            yes: Some(yes.into()),
            no: Some(no.into()),
        }
    }
}

/// Picks one option from a set you define.
///
/// Options keep the order they were added in. Adding an option with a name that already
/// exists replaces its description. For options that map to a Rust enum, prefer
/// [`EnumChoice`](crate::EnumChoice).
///
/// ```
/// use typesafe_client::ChoiceQuestion;
///
/// let question = ChoiceQuestion::new("Which team should handle this?")
///     .with_option("billing", "Payments, invoicing, refunds")
///     .with_option("technical", "Bugs, outages, integrations")
///     .with_bare_option("sales");
/// assert_eq!(question.criteria.len(), 3);
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChoiceQuestion {
    /// What the model should decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Content>,
    /// Option name to description; `None` is sent as `null` (the name speaks for itself).
    #[serde(default)]
    pub criteria: IndexMap<String, Option<Content>>,
}

impl ChoiceQuestion {
    /// A Choice asking `instructions`, with no options yet.
    pub fn new(instructions: impl Into<Content>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria: IndexMap::new(),
        }
    }

    /// Adds an option with a description of when it applies.
    #[must_use]
    pub fn with_option(mut self, name: impl Into<String>, description: impl Into<Content>) -> Self {
        self.criteria.insert(name.into(), Some(description.into()));
        self
    }

    /// Adds an option without a description, for names that are self-explanatory or whose
    /// meaning lives in the state (for example line or element ids).
    #[must_use]
    pub fn with_bare_option(mut self, name: impl Into<String>) -> Self {
        self.criteria.insert(name.into(), None);
        self
    }

    /// Adds several options without descriptions.
    #[must_use]
    pub fn with_bare_options<I>(mut self, names: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.criteria
            .extend(names.into_iter().map(|name| (name.into(), None)));
        self
    }
}

/// Rates the state against ordered levels, from the low end of the scale to the high end.
///
/// ```
/// use typesafe_client::ScoreQuestion;
///
/// let question = ScoreQuestion::new(
///     "How frustrated is the customer?",
///     ["Calm", "Frustrated", "Very angry"],
/// );
/// assert_eq!(question.criteria.len(), 3);
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScoreQuestion {
    /// What the model should rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Content>,
    /// Level descriptions; each level's position is its score, starting at zero.
    #[serde(default)]
    pub criteria: Vec<Content>,
}

impl ScoreQuestion {
    /// A Score asking `instructions`, with `levels` ordered from low to high.
    pub fn new<I>(instructions: impl Into<Content>, levels: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Content>,
    {
        Self {
            instructions: Some(instructions.into()),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn omits_optional_fields_when_serializing_a_bare_noul() {
        let question = Question::from(NoulQuestion::new("Is this spam?"));

        assert_eq!(
            serde_json::to_value(&question).unwrap(),
            json!({"type": "noul", "instructions": "Is this spam?"})
        );
    }

    #[test]
    fn renames_noul_criteria_to_true_and_false() {
        let question = Question::from(NoulQuestion::new("Urgent?").with_criteria("Yes", "No"));

        assert_eq!(
            serde_json::to_value(&question).unwrap(),
            json!({"type": "noul", "instructions": "Urgent?", "criteria": {"true": "Yes", "false": "No"}})
        );
    }

    #[test]
    fn sends_null_for_bare_choice_options_in_insertion_order() {
        let question = Question::from(
            ChoiceQuestion::new("Pick")
                .with_bare_options(["zulu", "alpha"])
                .with_option("mike", json!({"what": "middle"})),
        );

        assert_eq!(
            serde_json::to_string(&question).unwrap(),
            r#"{"type":"choice","instructions":"Pick","criteria":{"zulu":null,"alpha":null,"mike":{"what":"middle"}}}"#
        );
    }

    #[test]
    fn replaces_description_when_option_name_is_added_twice() {
        let question = ChoiceQuestion::new("Pick")
            .with_option("a", "first")
            .with_option("a", "second");

        assert_eq!(question.criteria.len(), 1);
        assert_eq!(question.criteria["a"], Some(Content::from("second")));
    }

    #[test]
    fn deserializes_every_question_kind_by_type_tag() {
        let questions: Vec<Question> = serde_json::from_value(json!([
            {"type": "noul", "instructions": "Q", "criteria": {"true": "y", "false": null}},
            {"type": "choice", "instructions": "Q", "criteria": {"a": null, "b": "B"}},
            {"type": "score", "instructions": "Q", "criteria": ["low", {"level": "high"}]}
        ]))
        .unwrap();

        let kinds: Vec<_> = questions.iter().map(Question::kind).collect();
        assert_eq!(
            kinds,
            [
                QuestionKind::Noul,
                QuestionKind::Choice,
                QuestionKind::Score
            ]
        );
    }
}
