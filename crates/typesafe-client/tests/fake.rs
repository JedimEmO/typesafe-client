//! `FakeSystemOne` behaves like a well-behaved API: complete, verified answers, scripted
//! failures, and a record of what was sent.

#![allow(
    clippy::float_cmp,
    reason = "configured answers are compared to the exact values that were configured"
)]

use std::sync::Arc;
use std::time::Duration;

use typesafe_client::fake::{FakeSystemOne, UNCERTAIN_NOUL};
use typesafe_client::{
    AnswerError, ApiError, ApiErrorKind, CallOptions, ChoiceOptions, ChoiceQuestion, EnumChoice,
    Error, ModelList, ModelMetadata, NoulAnswer, NoulQuestion, QuestionKey, Questions,
    ScoreQuestion, SystemOne, SystemOneResponse, TransportError, TypedChoice, Usage,
    ValidationError, ValueChoice, choice_options,
};

choice_options! {
    enum Department {
        Billing = "billing" => "Payments, invoicing, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades, new accounts",
    }
}

/// Application code under test: depends on the trait, not on the HTTP client.
struct Triage {
    typesafe: Arc<dyn SystemOne>,
}

#[derive(Debug)]
struct Decision {
    urgent: f64,
    department: TypedChoice<Department>,
    frustration_level: usize,
}

impl Triage {
    async fn triage(&self, ticket: &str) -> Result<Decision, Error> {
        let mut questions = Questions::new();
        let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
        let department = questions.add(
            "department",
            EnumChoice::<Department>::new("Which team should handle this?"),
        );
        let frustration = questions.add(
            "frustration",
            ScoreQuestion::new(
                "How frustrated is the customer?",
                ["Calm", "Frustrated", "Very angry"],
            ),
        );

        let response = self.typesafe.system_one(ticket, questions).send().await?;
        Ok(Decision {
            urgent: response.answer(&urgent)?.noul,
            department: response.answer(&department)?,
            frustration_level: response.answer(&frustration)?.nearest_level(),
        })
    }
}

fn triage_with_fake() -> (Triage, Arc<FakeSystemOne>) {
    let fake = Arc::new(FakeSystemOne::new());
    (
        Triage {
            typesafe: fake.clone(),
        },
        fake,
    )
}

#[tokio::test]
async fn answers_unconfigured_questions_with_maximal_uncertainty() {
    let (triage, _) = triage_with_fake();

    let decision = triage.triage("Where is my invoice?").await.unwrap();

    assert_eq!(decision.urgent, UNCERTAIN_NOUL);
    assert_eq!(decision.department.confidence, 0.0);
    for (_, probability) in &decision.department.probabilities {
        assert!((probability - 1.0 / 3.0).abs() < 1e-12);
    }
    assert_eq!(decision.frustration_level, 1);
}

#[tokio::test]
async fn answers_configured_questions_by_id() {
    let (triage, fake) = triage_with_fake();
    fake.set_noul("is_urgent", 0.92);
    fake.set_choice("department", Department::Technical.name());
    fake.set_score("frustration", 1.8);

    let decision = triage
        .triage("Payouts have failed for 3 days!")
        .await
        .unwrap();

    assert_eq!(decision.urgent, 0.92);
    assert_eq!(decision.department.choice, Department::Technical);
    assert_eq!(decision.department.confidence, 1.0);
    assert_eq!(decision.department.probability(&Department::Billing), 0.0);
    assert_eq!(decision.frustration_level, 2);
}

#[tokio::test]
async fn reports_every_option_of_a_choice_like_the_api() {
    let fake = Arc::new(FakeSystemOne::new());
    let mut questions = Questions::new();
    let team = questions.add(
        "team",
        ChoiceQuestion::new("Which team?").with_bare_options(["billing", "technical", "sales"]),
    );
    fake.set_choice(&team, "technical");

    let response = fake.system_one("ticket", questions).send().await.unwrap();
    let answer = response.answer(&team).unwrap();

    assert_eq!(answer.choice, "technical");
    assert_eq!(
        answer.probabilities.keys().collect::<Vec<_>>(),
        ["billing", "technical", "sales"]
    );
    assert_eq!(answer.probability("sales"), Some(0.0));
}

#[tokio::test]
async fn picks_the_most_probable_configured_option() {
    let (triage, fake) = triage_with_fake();
    fake.set_choice_probabilities("department", [("billing", 0.3), ("technical", 0.7)]);

    let decision = triage
        .triage("Charged twice after the outage")
        .await
        .unwrap();

    assert_eq!(decision.department.choice, Department::Technical);
    assert_eq!(decision.department.probability(&Department::Sales), 0.0);
    assert!(decision.department.confidence > 0.0 && decision.department.confidence < 1.0);
}

#[tokio::test]
async fn records_requests_with_their_state_and_questions() {
    let (triage, fake) = triage_with_fake();

    triage.triage("first").await.unwrap();
    triage.triage("second").await.unwrap();

    let requests = fake.requests();
    assert_eq!(fake.request_count(), 2);
    assert_eq!(requests[0].state.as_text(), Some("first"));
    assert_eq!(fake.last_request().unwrap().state.as_text(), Some("second"));
    assert!(requests[1].questions.get("department").is_some());
}

#[tokio::test]
async fn returns_scripted_errors_in_order_before_answering_again() {
    let (triage, fake) = triage_with_fake();
    fake.push_error(
        ApiError::new(429, "Too many requests").with_retry_after(Duration::from_secs(1)),
    );
    fake.push_error(Error::Timeout(TransportError::new("timed out")));

    let rate_limited = triage.triage("a").await.unwrap_err();
    let timed_out = triage.triage("b").await.unwrap_err();
    let answered = triage.triage("c").await;

    assert_eq!(
        rate_limited.api().map(|api| api.kind),
        Some(ApiErrorKind::RateLimited)
    );
    assert_eq!(rate_limited.retry_after(), Some(Duration::from_secs(1)));
    assert!(matches!(timed_out, Error::Timeout(_)));
    assert!(timed_out.is_retryable());
    assert!(answered.is_ok());
}

#[tokio::test]
async fn fails_like_the_api_when_configuration_does_not_fit_the_question() {
    let (triage, fake) = triage_with_fake();

    fake.set_choice("department", "legal");
    let unknown_option = triage.triage("ticket").await.unwrap_err();
    assert!(matches!(
        unknown_option,
        Error::Answer(AnswerError::UnknownOption { ref option, .. }) if option == "legal"
    ));

    fake.clear_answers();
    fake.set_noul("department", 0.9);
    let wrong_kind = triage.triage("ticket").await.unwrap_err();
    assert!(matches!(
        wrong_kind,
        Error::Answer(AnswerError::WrongKind { .. })
    ));
}

#[tokio::test]
async fn validates_requests_like_the_client() {
    let fake = FakeSystemOne::new();
    let mut questions = Questions::new();
    questions.add(
        "too_many",
        ChoiceQuestion::new("Which line?").with_bare_options((0..300).map(|i| format!("L{i}"))),
    );
    let request = typesafe_client::SystemOneRequest::new("state", questions);

    let error = fake
        .send(&request, &CallOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::InvalidRequest(ValidationError::TooManyChoiceOptions { count: 300, .. })
    ));
    assert_eq!(fake.request_count(), 0, "invalid requests are not sent");
}

#[tokio::test]
async fn answers_runtime_value_choices_with_typed_values() {
    let fake = Arc::new(FakeSystemOne::new());
    let mut questions = Questions::new();
    let line = questions.add(
        "line",
        ValueChoice::new("Which line answers the question?", 0..200_u32),
    );
    fake.set_choice_probabilities(&line, [("42", 0.9), ("7", 0.1)]);

    let response = fake.system_one("document", questions).send().await.unwrap();
    let answer = response.answer(&line).unwrap();

    assert_eq!(answer.choice, 42);
    assert_eq!(answer.top(2), [(42, 0.9), (7, 0.1)]);
}

#[tokio::test]
async fn returns_scripted_responses_and_verifies_them() {
    let fake = FakeSystemOne::new();
    let mut questions = Questions::new();
    let urgent: QuestionKey<_> = questions.add("is_urgent", NoulQuestion::new("Urgent?"));
    let request = typesafe_client::SystemOneRequest::new("state", questions);
    fake.push_response(
        SystemOneResponse::new("jev-1.12", Usage::new(40, 4))
            .with_answer(&urgent, NoulAnswer::new(0.25)),
    );

    let response = fake.send(&request, &CallOptions::default()).await.unwrap();

    assert_eq!(response.answer(&urgent).unwrap().noul, 0.25);
    assert_eq!(response.model, "jev-1.12");
    assert_eq!(response.request_id.as_deref(), Some("fake-request-1"));
}

#[tokio::test]
async fn lists_configured_models() {
    let fake = FakeSystemOne::new();
    assert_eq!(fake.list_models().await.unwrap().models.len(), 1);

    fake.set_models(ModelList::new([ModelMetadata::new(
        "jev-1.12",
        "Pinned",
        "2026-09-10T18:38:01+00:00",
    )]));

    assert_eq!(fake.list_models().await.unwrap().models[0].name, "jev-1.12");
}

#[cfg(feature = "images")]
mod images {
    use std::sync::Arc;

    use serde_json::json;
    use typesafe_client::fake::FakeSystemOne;
    use typesafe_client::{
        Assets, BoxAnswer, BoxCoordinates, BoxQuestion, Error, Image, NoulQuestion, QuestionKey,
        Questions, SystemOne, ValidationError,
    };

    const PNG: &[u8] = include_bytes!("fixtures/red-rectangle.png");

    fn mug_request() -> (Questions, Assets, serde_json::Value) {
        let mut assets = Assets::new();
        let photo = assets.add(Image::png(PNG.to_vec()));
        let mut questions = Questions::new();
        questions.add("visible", NoulQuestion::new("Is there a mug in `photo`?"));
        questions.add("mug", BoxQuestion::new("the mug in `photo`"));
        (questions, assets, json!({"photo": photo}))
    }

    #[tokio::test]
    async fn covers_the_whole_image_with_zero_confidence_when_a_box_is_not_configured() {
        let fake = Arc::new(FakeSystemOne::new());
        let mut assets = Assets::new();
        let photo = assets.add(Image::png(PNG.to_vec()));
        let mut questions = Questions::new();
        let mug = questions.add("mug", BoxQuestion::new("the mug in `photo`"));

        let response = fake
            .system_one(json!({"photo": photo}), questions)
            .assets(assets)
            .send()
            .await
            .unwrap();

        let mug = response.answer(&mug).unwrap();
        assert_eq!(mug.bounds, BoxCoordinates::FULL_IMAGE);
        assert_eq!(mug.confidence, Some(0.0));
    }

    #[tokio::test]
    async fn answers_a_configured_box_and_records_the_images_sent() {
        let fake = Arc::new(FakeSystemOne::new());
        let bounds = BoxCoordinates::new(0.25, 0.82, 0.44, 0.84);
        fake.set_box("mug", bounds);
        let (questions, assets, state) = mug_request();
        let mug = QuestionKey::<BoxAnswer>::new("mug");

        let response = fake
            .system_one(state, questions)
            .assets(assets.clone())
            .send()
            .await
            .unwrap();

        assert_eq!(response.answer(&mug).unwrap().bounds, bounds);
        assert_eq!(response.answer(&mug).unwrap().confidence, Some(1.0));
        assert_eq!(fake.last_request().unwrap().assets, assets);
    }

    #[tokio::test]
    async fn rejects_image_requests_the_api_would_reject() {
        let fake = Arc::new(FakeSystemOne::new());
        let mut questions = Questions::new();
        questions.add("mug", BoxQuestion::new("the mug"));

        let error = fake
            .system_one("no image", questions)
            .send()
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                Error::InvalidRequest(ValidationError::BoxWithoutImage { .. })
            ),
            "{error:?}"
        );
        assert_eq!(fake.request_count(), 0);
    }
}
