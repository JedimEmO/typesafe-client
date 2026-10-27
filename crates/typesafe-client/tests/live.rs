//! Tests against the real TypeSafe API. They are ignored by default; run them with
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo test -p typesafe-client --test live -- --ignored
//! ```
//!
//! Set `TYPESAFE_DEFAULT_MODEL` to pin a model. Assertions check structure and only
//! unambiguous judgments, since model outputs are probabilities rather than fixed values.

use serde_json::Value;
use typesafe_client::{
    ApiErrorKind, Client, EnumChoice, NoulQuestion, Questions, ScoreQuestion, choice_options,
};

const LIVE_SPEC_URL: &str = "https://api.typesafe.ai/openapi.json";

choice_options! {
    enum Department {
        Billing = "billing" => "Payments, invoicing, refunds, duplicate charges",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades, new accounts",
    }
}

fn live_client() -> Client {
    Client::from_env().expect("set TYPESAFE_API_KEY to run live tests")
}

fn assert_probability(value: f64, what: &str) {
    assert!(
        (0.0..=1.0).contains(&value),
        "{what} = {value} is not a probability"
    );
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn lists_available_models() {
    let models = live_client().list_models().await.unwrap();

    assert!(!models.models.is_empty());
    for model in &models.models {
        assert!(!model.name.is_empty());
        // The spec documents `YYYY-MM-DD`, the server sends a full timestamp; both start
        // with the calendar date.
        assert!(starts_with_calendar_date(&model.release_date), "{model:?}");
    }
}

fn starts_with_calendar_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes[..10].iter().enumerate().all(|(i, byte)| {
            if i == 4 || i == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn answers_noul_choice_and_score_questions_in_one_request() {
    let state =
        "Hi, I was charged twice for order A-104 yesterday. Please refund the duplicate charge.";
    let mut questions = Questions::new();
    let refund = questions.add(
        "refund_requested",
        NoulQuestion::new("Is the customer explicitly asking for a refund?"),
    );
    let poem = questions.add(
        "is_poem",
        NoulQuestion::new("Is the message written as a poem?"),
    );
    let department = questions.add(
        "department",
        EnumChoice::<Department>::new("Which team should handle this message?"),
    );
    let frustration = questions.add(
        "frustration",
        ScoreQuestion::new(
            "How frustrated is the customer?",
            ["Calm, matter-of-fact", "Frustrated but civil", "Very angry"],
        ),
    );

    let response = live_client()
        .system_one(state, questions)
        .send()
        .await
        .unwrap();

    assert!(
        response.request_id.is_some(),
        "x-typesafe-request-id header"
    );
    assert!(response.usage.input_tokens > 0);
    assert_eq!(response.answers.len(), 4);

    let refund = response.answer(&refund).unwrap();
    let poem = response.answer(&poem).unwrap();
    assert_probability(refund.noul, "refund_requested");
    assert!(
        refund.noul > 0.5,
        "an explicit refund request scored {}",
        refund.noul
    );
    assert!(
        poem.noul < 0.5,
        "a plain support message scored {} as a poem",
        poem.noul
    );

    let department = response.answer(&department).unwrap();
    assert_eq!(department.choice, Department::Billing, "{department:?}");
    assert_probability(department.confidence, "department confidence");
    let total: f64 = department.probabilities.iter().map(|(_, p)| p).sum();
    assert!(
        (total - 1.0).abs() < 0.01,
        "choice probabilities sum to {total}"
    );

    let frustration = response.answer(&frustration).unwrap();
    assert!((0.0..=2.0).contains(&frustration.score), "{frustration:?}");
    assert_eq!(frustration.level_probabilities().len(), 3);
    assert_probability(frustration.confidence, "frustration confidence");
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs network access"]
async fn rejects_an_invalid_api_key() {
    let client = Client::builder()
        .api_key("invalid-key-from-typesafe-client-tests")
        .build()
        .unwrap();
    let mut questions = Questions::new();
    questions.add("q", NoulQuestion::new("Is this a greeting?"));

    let error = client.system_one("hi", questions).send().await.unwrap_err();

    let api = error.api().expect("an API error");
    assert_eq!(api.kind, ApiErrorKind::Authentication, "{api}");
    assert_eq!(api.error_type.as_deref(), Some("authentication_error"));
    assert!(api.request_id.is_some());
}

#[tokio::test]
#[ignore = "fetches the live OpenAPI spec; needs network access"]
async fn vendored_openapi_spec_matches_live_spec() {
    let live: Value = reqwest::get(LIVE_SPEC_URL)
        .await
        .and_then(reqwest::Response::error_for_status)
        .expect("spec is reachable")
        .json()
        .await
        .expect("spec is JSON");
    let vendored: Value = serde_json::from_str(include_str!("fixtures/openapi.json")).unwrap();

    assert!(
        live == vendored,
        "the live spec changed (live version {}, vendored {}). Review the diff, update the \
         wire types if needed, then refresh tests/fixtures/openapi.json from {LIVE_SPEC_URL}",
        live["info"]["version"],
        vendored["info"]["version"]
    );
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn reports_a_request_over_the_context_length() {
    let state = "The quick brown fox jumps over the lazy dog. ".repeat(9_000);
    let mut questions = Questions::new();
    questions.add("fox", NoulQuestion::new("Is a fox mentioned?"));

    let error = live_client()
        .system_one(state, questions)
        .send()
        .await
        .unwrap_err();

    let api = error.api().expect("an API error");
    assert!(api.is_context_length_exceeded(), "{api}");
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn accepts_the_long_context_header() {
    let mut questions = Questions::new();
    let greeting = questions.add("greeting", NoulQuestion::new("Is this a greeting?"));

    let response = live_client()
        .system_one("Hello there!", questions)
        .long_context()
        .send()
        .await
        .unwrap();

    assert_probability(response.answer(&greeting).unwrap().noul, "greeting");
}

/// A 320×320 white PNG with a red rectangle from (100, 80) to (220, 200).
#[cfg(feature = "images")]
const RED_RECTANGLE: &[u8] = include_bytes!("fixtures/red-rectangle.png");

#[cfg(feature = "images")]
#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn locates_a_red_rectangle_in_an_uploaded_image() {
    use typesafe_client::{Assets, BoxQuestion, Image};

    let mut assets = Assets::new();
    let photo = assets.add(Image::png(RED_RECTANGLE.to_vec()));
    let mut questions = Questions::new();
    let red = questions.add(
        "red",
        NoulQuestion::new("Is there a red rectangle in `photo`?"),
    );
    let blue = questions.add(
        "blue",
        NoulQuestion::new("Is there a blue circle in `photo`?"),
    );
    let found = questions.add("where", BoxQuestion::new("the red rectangle in `photo`"));

    let response = live_client()
        .system_one(serde_json::json!({"photo": photo}), questions)
        .assets(assets)
        .send()
        .await
        .unwrap();

    let red = response.answer(&red).unwrap().noul;
    let blue = response.answer(&blue).unwrap().noul;
    assert!(red > 0.5, "a red rectangle scored {red}");
    assert!(blue < 0.5, "an absent blue circle scored {blue}");
    let found = response.answer(&found).unwrap();
    let (x, y) = found.bounds.center();
    // The rectangle's centre is at (0.5, 0.4375); allow for an imprecise box.
    assert!((0.3..=0.7).contains(&x), "{found:?}");
    assert!((0.2..=0.7).contains(&y), "{found:?}");
    if let Some(confidence) = found.confidence {
        assert_probability(confidence, "box confidence");
    }
}

#[cfg(feature = "images")]
#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn sends_an_image_inside_question_instructions() {
    use typesafe_client::{Assets, Image};

    let mut assets = Assets::new();
    let example = assets.add(Image::png(RED_RECTANGLE.to_vec()));
    let mut questions = Questions::new();
    let red = questions.add(
        "red",
        NoulQuestion::new(format!("Is the main shape in this image red: {example}?")),
    );

    let response = live_client()
        .system_one("Answer about the image in the question.", questions)
        .assets(assets)
        .send()
        .await
        .unwrap();

    let red = response.answer(&red).unwrap().noul;
    assert!(red > 0.5, "a red rectangle scored {red}");
}
