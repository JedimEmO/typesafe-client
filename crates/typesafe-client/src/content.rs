use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A string, JSON object or JSON array.
///
/// The API accepts exactly these three shapes for request `state`, question
/// `instructions`, Choice option descriptions and Score levels. A string suits simple
/// text; an object with named fields suits content that has several parts.
///
/// Object fields are kept in `serde_json::Map` order: sorted by key, unless your
/// application enables `serde_json`'s `preserve_order` feature.
///
/// ```
/// use serde_json::json;
/// use typesafe_client::Content;
///
/// let text = Content::from("My card was charged twice.");
/// let record = Content::from(json!({ "message": "My card was charged twice.", "order_id": "A-104" }));
///
/// assert_eq!(text.as_text(), Some("My card was charged twice."));
/// assert!(matches!(record, Content::Object(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// Plain text.
    Text(String),
    /// A JSON object with named fields.
    Object(Map<String, Value>),
    /// A JSON array, such as a sequence of messages.
    Array(Vec<Value>),
}

impl Content {
    /// Serializes `value` into content, for example an application struct used as state.
    ///
    /// Numbers, booleans and `null` become their JSON text, because the API only accepts
    /// strings, objects and arrays. The error converts into [`Error`](crate::Error) with `?`.
    pub fn json<T: Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        serde_json::to_value(value).map(Self::from)
    }

    /// The text, if this is [`Content::Text`].
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Object(_) | Self::Array(_) => None,
        }
    }
}

impl From<&str> for Content {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<&&str> for Content {
    fn from(text: &&str) -> Self {
        Self::Text((*text).to_owned())
    }
}

impl From<String> for Content {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&String> for Content {
    fn from(text: &String) -> Self {
        Self::Text(text.clone())
    }
}

impl From<Cow<'_, str>> for Content {
    fn from(text: Cow<'_, str>) -> Self {
        Self::Text(text.into_owned())
    }
}

impl From<Box<str>> for Content {
    fn from(text: Box<str>) -> Self {
        Self::Text(text.into())
    }
}

impl From<Map<String, Value>> for Content {
    fn from(fields: Map<String, Value>) -> Self {
        Self::Object(fields)
    }
}

impl From<Vec<Value>> for Content {
    fn from(items: Vec<Value>) -> Self {
        Self::Array(items)
    }
}

impl From<Value> for Content {
    fn from(value: Value) -> Self {
        match value {
            Value::String(text) => Self::Text(text),
            Value::Object(fields) => Self::Object(fields),
            Value::Array(items) => Self::Array(items),
            scalar @ (Value::Null | Value::Bool(_) | Value::Number(_)) => {
                Self::Text(scalar.to_string())
            }
        }
    }
}

impl From<&Value> for Content {
    fn from(value: &Value) -> Self {
        Self::from(value.clone())
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::json;

    use super::*;

    #[test]
    fn maps_json_shapes_to_matching_variants() {
        assert_eq!(Content::from(json!("hi")), Content::Text("hi".into()));
        assert!(matches!(Content::from(json!({"a": 1})), Content::Object(_)));
        assert!(matches!(Content::from(json!([1, 2])), Content::Array(_)));
    }

    #[test]
    fn renders_scalars_as_text_when_converted_from_json() {
        assert_eq!(Content::from(json!(42)), Content::Text("42".into()));
        assert_eq!(Content::from(json!(true)), Content::Text("true".into()));
        assert_eq!(Content::from(json!(null)), Content::Text("null".into()));
    }

    #[test]
    fn converts_borrowed_and_owned_strings() {
        let owned = String::from("ticket");
        let levels = ["low", "high"];

        assert_eq!(Content::from(&owned), Content::from("ticket"));
        assert_eq!(Content::from(&levels[0]), Content::from("low"));
        assert_eq!(
            Content::from(Cow::Borrowed("ticket")),
            Content::from("ticket")
        );
        assert_eq!(
            Content::from(Box::<str>::from("ticket")),
            Content::from("ticket")
        );
        assert_eq!(Content::from(&json!(["a"])), Content::from(json!(["a"])));
    }

    #[test]
    fn serializes_structs_as_objects() {
        #[derive(Serialize)]
        struct Ticket {
            subject: &'static str,
            body: &'static str,
        }

        let content = Content::json(&Ticket {
            subject: "Duplicate charge",
            body: "Charged twice",
        })
        .unwrap();

        assert_eq!(
            serde_json::to_value(&content).unwrap(),
            json!({"subject": "Duplicate charge", "body": "Charged twice"})
        );
    }

    #[test]
    fn rejects_scalars_when_deserializing() {
        assert!(serde_json::from_value::<Content>(json!(3.5)).is_err());
        assert!(serde_json::from_value::<Content>(json!(null)).is_err());
    }
}
