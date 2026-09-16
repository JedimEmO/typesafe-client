use std::fmt::Display;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{
    Answer, AnswerError, ChoiceAnswer, ChoiceQuestion, Content, IntoQuestion, Question, ReadAnswer,
};

/// A fixed set of Choice options backed by a Rust type, usually a fieldless enum.
///
/// Implement it with [`choice_options!`](crate::choice_options) or by hand. The option
/// names are the strings sent to the API and returned in answers, and must be unique.
pub trait ChoiceOptions: Copy + Eq + 'static {
    /// Every option, in the order it is sent.
    const ALL: &'static [Self];

    /// The option name used on the wire.
    fn name(self) -> &'static str;

    /// When this option applies; `None` sends the name alone.
    fn description(self) -> Option<Content>;

    /// Parses an option name from an answer.
    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|option| option.name() == name)
    }
}

/// A Choice question whose options are the values of an enum.
///
/// ```
/// use typesafe_client::{choice_options, EnumChoice, Questions};
///
/// choice_options! {
///     pub enum Department {
///         Billing = "billing" => "Payments, invoicing, refunds",
///         Technical = "technical" => "Bugs, outages, integrations",
///         Sales = "sales",
///     }
/// }
///
/// let mut questions = Questions::new();
/// let department = questions.add(
///     "department",
///     EnumChoice::<Department>::new("Which team should handle this?"),
/// );
/// # let _ = department;
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct EnumChoice<T> {
    instructions: Content,
    options: PhantomData<fn() -> T>,
}

impl<T: ChoiceOptions> EnumChoice<T> {
    /// A Choice over every value of `T`, asking `instructions`.
    pub fn new(instructions: impl Into<Content>) -> Self {
        Self {
            instructions: instructions.into(),
            options: PhantomData,
        }
    }
}

impl<T: ChoiceOptions> IntoQuestion for EnumChoice<T> {
    type Answer = TypedChoice<T>;

    fn into_question(self) -> Question {
        Question::Choice(ChoiceQuestion {
            instructions: Some(self.instructions),
            criteria: T::ALL
                .iter()
                .map(|option| (option.name().to_owned(), option.description()))
                .collect(),
        })
    }
}

/// A Choice question over values known only at runtime, such as line or record ids.
///
/// Each value is sent as its `Display` text and parsed back with `FromStr`, so the two
/// must round-trip. Options carry no descriptions; put what they refer to in the state.
///
/// ```
/// use typesafe_client::{Questions, ValueChoice};
///
/// let lines = ["Refunds take 5 days.", "Contact support by email.", "Cancel anytime."];
/// let document: String = lines
///     .iter()
///     .enumerate()
///     .map(|(i, line)| format!("{i}| {line}\n"))
///     .collect();
///
/// let mut questions = Questions::new();
/// let line = questions.add(
///     "line",
///     ValueChoice::new("Which line says how long refunds take?", 0..lines.len()),
/// );
/// // `response.answer(&line)?` returns a `TypedChoice<usize>`.
/// # let _ = (line, document);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ValueChoice<T> {
    question: ChoiceQuestion,
    values: PhantomData<fn() -> T>,
}

impl<T: Display> ValueChoice<T> {
    /// A Choice over `values`, asking `instructions`.
    pub fn new<I>(instructions: impl Into<Content>, values: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            question: ChoiceQuestion::new(instructions)
                .with_bare_options(values.into_iter().map(|value| value.to_string())),
            values: PhantomData,
        }
    }
}

impl<T> IntoQuestion for ValueChoice<T>
where
    T: FromStr + Clone + PartialEq,
{
    type Answer = Self;

    fn into_question(self) -> Question {
        Question::Choice(self.question)
    }
}

impl<T> ReadAnswer for ValueChoice<T>
where
    T: FromStr + Clone + PartialEq,
{
    type Output<'a> = TypedChoice<T>;

    fn read(id: &str, answer: &Answer) -> Result<TypedChoice<T>, AnswerError> {
        let raw = ChoiceAnswer::read(id, answer)?;
        let parse = |option: &str| {
            option.parse::<T>().map_err(|_| AnswerError::UnknownOption {
                id: id.to_owned(),
                option: option.to_owned(),
            })
        };
        Ok(TypedChoice {
            choice: parse(&raw.choice)?,
            confidence: raw.confidence,
            probabilities: raw
                .probabilities
                .iter()
                .map(|(option, probability)| Ok((parse(option)?, *probability)))
                .collect::<Result<_, AnswerError>>()?,
        })
    }
}

/// A Choice answer with its options parsed into `T`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TypedChoice<T> {
    /// The option with the highest probability.
    pub choice: T,
    /// How concentrated the distribution is, from 0 to 1.
    pub confidence: f64,
    /// Probability of each option: in declaration order for [`EnumChoice`], in response
    /// order for [`ValueChoice`].
    pub probabilities: Vec<(T, f64)>,
}

impl<T> TypedChoice<T> {
    /// A typed choice, for fixtures and tests.
    pub fn new(
        choice: T,
        confidence: f64,
        probabilities: impl IntoIterator<Item = (T, f64)>,
    ) -> Self {
        Self {
            choice,
            confidence,
            probabilities: probabilities.into_iter().collect(),
        }
    }
}

impl<T: PartialEq> TypedChoice<T> {
    /// The probability of `option`; 0 if the response did not include it.
    pub fn probability(&self, option: &T) -> f64 {
        self.probabilities
            .iter()
            .find(|(candidate, _)| candidate == option)
            .map_or(0.0, |(_, probability)| *probability)
    }
}

impl<T: Clone> TypedChoice<T> {
    /// Options from most to least probable.
    pub fn ranked(&self) -> Vec<(T, f64)> {
        let mut ranked = self.probabilities.clone();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked
    }

    /// The `n` most probable options, most probable first.
    pub fn top(&self, n: usize) -> Vec<(T, f64)> {
        let mut ranked = self.ranked();
        ranked.truncate(n);
        ranked
    }
}

impl<T: ChoiceOptions> ReadAnswer for TypedChoice<T> {
    type Output<'a> = Self;

    /// Fails with [`AnswerError::UnknownOption`] when the answer names an option `T` does
    /// not have, which means the response does not belong to this question.
    fn read(id: &str, answer: &Answer) -> Result<Self, AnswerError> {
        let raw = ChoiceAnswer::read(id, answer)?;
        let unknown = |option: &str| AnswerError::UnknownOption {
            id: id.to_owned(),
            option: option.to_owned(),
        };

        let choice = T::from_name(&raw.choice).ok_or_else(|| unknown(&raw.choice))?;
        if let Some(name) = raw
            .probabilities
            .keys()
            .find(|name| T::from_name(name).is_none())
        {
            return Err(unknown(name));
        }
        let probabilities = T::ALL
            .iter()
            .map(|option| (*option, raw.probability(option.name()).unwrap_or(0.0)))
            .collect();

        Ok(Self {
            choice,
            confidence: raw.confidence,
            probabilities,
        })
    }
}

/// Declares a fieldless enum and implements [`ChoiceOptions`] for it.
///
/// Each variant names its wire option and, optionally, a description after `=>`. The
/// description can be anything that converts into [`Content`], such as a string or a
/// `serde_json::json!` object.
///
/// The enum always derives `Debug, Clone, Copy, PartialEq, Eq, Hash`; add other derives
/// (such as `serde::Serialize`) yourself. Option names must be unique:
///
/// ```compile_fail
/// typesafe_client::choice_options! {
///     enum Tone {
///         Calm = "calm",
///         Relaxed = "calm",
///     }
/// }
/// ```
///
/// ```
/// use typesafe_client::{choice_options, ChoiceOptions};
///
/// choice_options! {
///     /// Which team handles a support ticket.
///     #[allow(dead_code)]
///     pub enum Department {
///         Billing = "billing" => "Payments, invoicing, refunds",
///         Technical = "technical" => serde_json::json!({
///             "what": "Bugs, outages, integrations",
///             "not_for": "Charges or refunds",
///         }),
///         Sales = "sales",
///     }
/// }
///
/// assert_eq!(Department::Billing.name(), "billing");
/// assert_eq!(Department::from_name("sales"), Some(Department::Sales));
/// assert_eq!(Department::Sales.description(), None);
/// ```
#[macro_export]
macro_rules! choice_options {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $option:literal $(=> $description:expr)?
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant, )+
        }

        const _: () = {
            let names: &[&str] = &[ $( $option, )+ ];
            let mut i = 0;
            while i < names.len() {
                let mut j = i + 1;
                while j < names.len() {
                    if $crate::__str_eq(names[i], names[j]) {
                        panic!(concat!("choice_options!: `", stringify!($name), "` has duplicate option names"));
                    }
                    j += 1;
                }
                i += 1;
            }
        };

        impl $crate::ChoiceOptions for $name {
            const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];

            fn name(self) -> &'static str {
                match self {
                    $( Self::$variant => $option, )+
                }
            }

            fn description(self) -> ::core::option::Option<$crate::Content> {
                match self {
                    $( Self::$variant => $crate::__choice_description!($($description)?), )+
                }
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __choice_description {
    () => {
        ::core::option::Option::None
    };
    ($description:expr) => {
        ::core::option::Option::Some($crate::Content::from($description))
    };
}

/// String equality usable in `const` context, for [`choice_options!`].
#[doc(hidden)]
pub const fn __str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Questions;

    crate::choice_options! {
        enum Tone {
            Calm = "calm" => "A neutral or polite message",
            Angry = "angry" => json!({"what": "An upset or hostile message"}),
            Excited = "excited",
        }
    }

    fn choice_answer(choice: &str, probabilities: &[(&str, f64)]) -> Answer {
        Answer::Choice(ChoiceAnswer {
            choice: choice.to_owned(),
            confidence: 0.7,
            probabilities: probabilities
                .iter()
                .map(|(name, p)| ((*name).to_owned(), *p))
                .collect(),
        })
    }

    #[test]
    fn builds_choice_criteria_from_variants_in_declaration_order() {
        let mut questions = Questions::new();
        questions.add("tone", EnumChoice::<Tone>::new("What is the tone?"));

        assert_eq!(
            serde_json::to_string(&questions).unwrap(),
            r#"{"tone":{"type":"choice","instructions":"What is the tone?","criteria":{"calm":"A neutral or polite message","angry":{"what":"An upset or hostile message"},"excited":null}}}"#
        );
    }

    #[test]
    fn parses_choice_and_probabilities_into_variants() {
        let answer = choice_answer("angry", &[("excited", 0.1), ("angry", 0.8), ("calm", 0.1)]);

        let typed = TypedChoice::<Tone>::read("tone", &answer).unwrap();

        assert_eq!(typed.choice, Tone::Angry);
        assert_eq!(
            typed.probabilities,
            [(Tone::Calm, 0.1), (Tone::Angry, 0.8), (Tone::Excited, 0.1)]
        );
        assert_eq!(typed.top(1), [(Tone::Angry, 0.8)]);
    }

    #[test]
    fn returns_unknown_option_when_answer_names_an_option_outside_the_enum() {
        let answer = choice_answer("sarcastic", &[("sarcastic", 1.0)]);

        assert_eq!(
            TypedChoice::<Tone>::read("tone", &answer),
            Err(AnswerError::UnknownOption {
                id: "tone".into(),
                option: "sarcastic".into()
            })
        );
    }

    #[test]
    fn returns_unknown_option_when_probabilities_include_an_unexpected_option() {
        let answer = choice_answer("calm", &[("calm", 0.9), ("sarcastic", 0.1)]);

        assert!(matches!(
            TypedChoice::<Tone>::read("tone", &answer),
            Err(AnswerError::UnknownOption { option, .. }) if option == "sarcastic"
        ));
    }

    #[test]
    fn treats_options_missing_from_probabilities_as_zero() {
        let answer = choice_answer("calm", &[("calm", 1.0)]);

        let typed = TypedChoice::<Tone>::read("tone", &answer).unwrap();

        assert!(typed.probability(&Tone::Excited).abs() < f64::EPSILON);
    }

    #[test]
    fn sends_runtime_values_as_bare_options_and_parses_them_back() {
        let mut questions = Questions::new();
        let line = questions.add("line", ValueChoice::new("Which line?", [3_usize, 14, 15]));
        assert_eq!(
            serde_json::to_value(&questions).unwrap()["line"]["criteria"],
            json!({"3": null, "14": null, "15": null})
        );

        let answer = choice_answer("14", &[("3", 0.1), ("14", 0.7), ("15", 0.2)]);
        let typed = ValueChoice::<usize>::read(line.id(), &answer).unwrap();

        assert_eq!(typed.choice, 14);
        assert_eq!(typed.top(2), [(14, 0.7), (15, 0.2)]);
        assert!((typed.probability(&3) - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn returns_unknown_option_when_a_value_does_not_parse() {
        let answer = choice_answer("L14", &[("L14", 1.0)]);

        assert_eq!(
            ValueChoice::<usize>::read("line", &answer),
            Err(AnswerError::UnknownOption {
                id: "line".into(),
                option: "L14".into()
            })
        );
    }

    #[test]
    fn round_trips_typed_choices_through_serde() {
        let typed = TypedChoice::new(14_usize, 0.7, [(14, 0.7), (15, 0.3)]);

        let json = serde_json::to_value(&typed).unwrap();
        let restored: TypedChoice<usize> = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(
            json,
            json!({"choice": 14, "confidence": 0.7, "probabilities": [[14, 0.7], [15, 0.3]]})
        );
        assert_eq!(restored, typed);
    }

    #[test]
    fn compares_strings_in_const_context() {
        const _: () = assert!(__str_eq("calm", "calm"));
        const _: () = assert!(!__str_eq("calm", "calms"));
        assert!(!__str_eq("calm", "clam"));
    }
}
