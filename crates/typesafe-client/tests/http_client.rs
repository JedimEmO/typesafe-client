//! HTTP behavior against an in-process mock server: headers, bodies, error mapping,
//! retries, backoff budget and timeouts.

#![allow(
    clippy::float_cmp,
    reason = "answers are parsed from exact JSON literals and compared to the same literals"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use typesafe_client::{
    AnswerError, ApiErrorKind, ChoiceQuestion, Client, Error, NoulQuestion, Questions, RetryPolicy,
    SystemOne, ValidationError,
};
use wiremock::matchers::{bearer_token, body_json, header, header_regex, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SYSTEM_ONE: &str = "/v1/systemone";

fn fast_retries() -> RetryPolicy {
    RetryPolicy::default()
        .with_backoff(Duration::from_millis(1), Duration::from_millis(5))
        .with_backoff_jitter(0.0)
}

fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .api_key("test-key")
        .base_url(server.uri())
        .default_model("jev-test")
        .retry(fast_retries())
        .build()
        .expect("client builds")
}

fn urgency_question() -> (
    Questions,
    typesafe_client::QuestionKey<typesafe_client::NoulAnswer>,
) {
    let mut questions = Questions::new();
    let key = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));
    (questions, key)
}

fn noul_response(noul: f64) -> Value {
    json!({
        "model": "jev-test",
        "answers": {"is_urgent": {"type": "noul", "noul": noul}},
        "usage": {"input_tokens": 312, "output_tokens": 48}
    })
}

fn ok_response() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(noul_response(0.92))
        .insert_header("x-typesafe-request-id", "req_ok")
}

async fn request_count(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("recording enabled")
        .len()
}

#[tokio::test]
async fn sends_authenticated_json_and_reads_typed_answer_when_request_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .and(bearer_token("test-key"))
        .and(header("content-type", "application/json"))
        .and(header("accept", "application/json"))
        .and(header_regex("user-agent", "^typesafe-client-rust/"))
        .and(body_json(json!({
            "state": "Help! My payouts have been failing for 3 days.",
            "model": "jev-test",
            "questions": {"is_urgent": {"type": "noul", "instructions": "Does this convey urgency?"}}
        })))
        .respond_with(ok_response())
        .expect(1)
        .mount(&server)
        .await;
    let (questions, urgent) = urgency_question();

    let response = client_for(&server)
        .system_one("Help! My payouts have been failing for 3 days.", questions)
        .send()
        .await
        .unwrap();

    assert_eq!(response.answer(&urgent).unwrap().noul, 0.92);
    assert_eq!(response.request_id.as_deref(), Some("req_ok"));
    assert_eq!(response.usage.output_tokens, 48);
}

#[tokio::test]
async fn sends_per_call_model_when_overridden() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response())
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    client_for(&server)
        .system_one("state", questions)
        .model("jev-1.12")
        .send()
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "jev-1.12");
}

#[tokio::test]
async fn returns_authentication_error_without_retrying_when_key_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"detail": {
                    "error_type": "authentication_error",
                    "message": "Cannot authenticate with the server. Please check your API key and try again."
                }}))
                .insert_header("x-typesafe-request-id", "req_denied"),
        )
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    let api = error.api().expect("an API error");
    assert_eq!(api.kind, ApiErrorKind::Authentication);
    assert_eq!(api.error_type.as_deref(), Some("authentication_error"));
    assert!(api.message.starts_with("Cannot authenticate"));
    assert_eq!(error.request_id(), Some("req_denied"));
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn reports_offending_fields_when_body_fails_server_validation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"detail": [
            {"loc": ["body", "questions", "is_urgent", "noul", "instructions"], "msg": "Input should be a valid string", "type": "string_type"}
        ]})))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    let api = error.api().expect("an API error");
    assert_eq!(api.kind, ApiErrorKind::UnprocessableEntity);
    assert_eq!(
        api.message,
        "body.questions.is_urgent.noul.instructions: Input should be a valid string"
    );
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn retries_then_succeeds_when_rate_limited_with_retry_after_ms() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after-ms", "20"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response())
        .mount(&server)
        .await;
    let (questions, urgent) = urgency_question();
    let started = Instant::now();

    let response = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap();

    assert_eq!(response.answer(&urgent).unwrap().noul, 0.92);
    assert_eq!(request_count(&server).await, 2);
    assert!(
        started.elapsed() >= Duration::from_millis(20),
        "waited for retry-after-ms"
    );
}

#[tokio::test]
async fn gives_up_after_max_retries_when_server_stays_overloaded() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(529))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    assert_eq!(
        error.api().map(|api| api.kind),
        Some(ApiErrorKind::Overloaded)
    );
    assert_eq!(
        request_count(&server).await,
        3,
        "first attempt plus two retries"
    );
}

#[tokio::test]
async fn uses_per_call_retry_policy_when_given() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .retry(RetryPolicy::disabled())
        .send()
        .await
        .unwrap_err();

    assert_eq!(
        error.api().map(|api| api.kind),
        Some(ApiErrorKind::InternalServer)
    );
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn stops_retrying_when_retry_after_exceeds_total_budget() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();
    let budget = fast_retries().with_total_timeout(Some(Duration::from_secs(2)));
    let started = Instant::now();

    let error = client_for(&server)
        .system_one("state", questions)
        .retry(budget)
        .send()
        .await
        .unwrap_err();

    let api = error.api().expect("an API error");
    assert_eq!(api.kind, ApiErrorKind::RateLimited);
    assert_eq!(api.retry_after, Some(Duration::from_secs(30)));
    assert_eq!(request_count(&server).await, 1);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "returned without sleeping"
    );
}

#[tokio::test]
async fn retries_when_an_attempt_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response().set_delay(Duration::from_secs(2)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response())
        .mount(&server)
        .await;
    let (questions, urgent) = urgency_question();

    let response = client_for(&server)
        .system_one("state", questions)
        .timeout(Duration::from_millis(200))
        .send()
        .await
        .unwrap();

    assert_eq!(response.answer(&urgent).unwrap().noul, 0.92);
    assert_eq!(request_count(&server).await, 2);
}

#[tokio::test]
async fn returns_timeout_error_when_retries_are_disabled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response().set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .timeout(Duration::from_millis(100))
        .retry(RetryPolicy::disabled())
        .send()
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Timeout(_)), "got {error:?}");
}

#[tokio::test]
async fn returns_connection_error_when_server_is_unreachable() {
    let unused_port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("free port")
        .port();
    let client = Client::builder()
        .api_key("test-key")
        .base_url(format!("http://127.0.0.1:{unused_port}"))
        .retry(RetryPolicy::disabled())
        .build()
        .unwrap();
    let (questions, _) = urgency_question();

    let error = client
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Connection(_)), "got {error:?}");
}

#[tokio::test]
async fn returns_decode_error_with_body_excerpt_when_success_body_is_malformed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html>proxy page</html>")
                .insert_header("x-typesafe-request-id", "req_html"),
        )
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    assert!(
        matches!(&error, Error::Decode { body_excerpt, .. } if body_excerpt == "<html>proxy page</html>"),
        "got {error:?}"
    );
    assert_eq!(error.request_id(), Some("req_html"));
    assert_eq!(
        request_count(&server).await,
        1,
        "decode errors are not retried"
    );
}

#[tokio::test]
async fn rejects_invalid_request_without_sending_it() {
    let server = MockServer::start().await;
    let mut questions = Questions::new();
    questions.add(
        "too_many",
        ChoiceQuestion::new("Which line?").with_bare_options((0..300).map(|i| format!("L{i}"))),
    );

    let error = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::InvalidRequest(ValidationError::TooManyChoiceOptions { count: 300, .. })
    ));
    assert_eq!(request_count(&server).await, 0);
}

#[tokio::test]
async fn sends_request_unchecked_when_validation_is_disabled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(
            ResponseTemplate::new(422).set_body_json(json!({"detail": "too many options"})),
        )
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("test-key")
        .base_url(server.uri())
        .validate_requests(false)
        .build()
        .unwrap();
    let mut questions = Questions::new();
    questions.add(
        "too_many",
        ChoiceQuestion::new("Which line?").with_bare_options((0..300).map(|i| format!("L{i}"))),
    );

    let error = client
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    assert_eq!(
        error.api().map(|api| api.message.as_str()),
        Some("too many options")
    );
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn lists_models_with_get_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(bearer_token("test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"name": "jev-latest", "description": "General-purpose system one model.", "release_date": "2026-09-15"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;

    let models = client_for(&server).list_models().await.unwrap();

    assert_eq!(models.models.len(), 1);
    assert_eq!(models.models[0].name, "jev-latest");
}

#[tokio::test]
async fn spawns_owned_calls_through_a_shared_trait_object() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response())
        .expect(3)
        .mount(&server)
        .await;
    let typesafe: Arc<dyn SystemOne> = Arc::new(client_for(&server));
    let (questions, urgent) = urgency_question();

    let calls: Vec<_> = (0..3)
        .map(|ticket| {
            let call = typesafe.system_one(format!("ticket {ticket}"), questions.clone());
            tokio::spawn(call.send())
        })
        .collect();

    for call in calls {
        let response = call.await.expect("task ran").unwrap();
        assert_eq!(response.answer(&urgent).unwrap().noul, 0.92);
    }
}

#[tokio::test]
async fn sends_a_cloned_call_again_after_a_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response())
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();
    let call = client_for(&server)
        .system_one("state", questions)
        .retry(RetryPolicy::disabled());

    let first = call.clone().send().await;
    let second = call.send().await;

    assert!(first.unwrap_err().is_retryable());
    assert!(second.is_ok());
}

#[tokio::test]
async fn cuts_the_last_attempt_short_to_meet_the_total_deadline() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response().set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();
    let deadline = fast_retries().with_total_timeout(Some(Duration::from_millis(800)));
    let started = Instant::now();

    let error = client_for(&server)
        .system_one("state", questions)
        .retry(deadline)
        .send()
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Timeout(_)), "got {error:?}");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn falls_back_to_backoff_when_retry_after_exceeds_the_cap() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3600"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ok_response())
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();
    let unlimited = fast_retries().with_total_timeout(None);
    let started = Instant::now();

    client_for(&server)
        .system_one("state", questions)
        .retry(unlimited)
        .send()
        .await
        .unwrap();

    assert_eq!(request_count(&server).await, 2);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "did not wait an hour"
    );
}

#[tokio::test]
async fn rejects_a_response_that_does_not_answer_the_questions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-test",
            "answers": {"something_else": {"type": "noul", "noul": 0.1}},
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    assert!(
        matches!(&error, Error::Answer(AnswerError::Missing { id }) if id == "is_urgent"),
        "got {error:?}"
    );
}

#[tokio::test]
async fn exposes_rejected_fields_of_a_validation_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"detail": [
            {"loc": ["body", "questions", "is_urgent", "noul", "instructions"], "msg": "Input should be a valid string", "type": "string_type"}
        ]})))
        .mount(&server)
        .await;
    let (questions, _) = urgency_question();

    let error = client_for(&server)
        .system_one("state", questions)
        .send()
        .await
        .unwrap_err();

    let fields = error.api().expect("an API error").field_errors();
    assert_eq!(fields.len(), 1);
    assert_eq!(
        fields[0].location.last().map(String::as_str),
        Some("instructions")
    );
    assert_eq!(fields[0].kind.as_deref(), Some("string_type"));
    assert_eq!(error.status(), Some(422));
    assert!(!error.is_retryable());
}
