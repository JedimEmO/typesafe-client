//! Images over HTTP against an in-process mock server: the images header, the multipart
//! body and retries that send the images again.

use std::time::Duration;

use serde_json::{Value, json};
use typesafe_client::constants::IMAGES_HEADER;
use typesafe_client::{
    Assets, BoxCoordinates, BoxQuestion, Client, Image, NoulQuestion, Questions, RetryPolicy,
};
use wiremock::matchers::{header, header_regex, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SYSTEM_ONE: &str = "/v1/systemone";
const PNG: &[u8] = include_bytes!("fixtures/red-rectangle.png");

fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .api_key("test-key")
        .base_url(server.uri())
        .default_model("jev-test")
        .retry(
            RetryPolicy::default()
                .with_backoff(Duration::from_millis(1), Duration::from_millis(5))
                .with_backoff_jitter(0.0),
        )
        .build()
        .expect("client builds")
}

fn box_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "jev-1.14.0",
        "answers": {
            "red": {"type": "noul", "noul": 0.97},
            "where": {
                "type": "box",
                "box": {"top": 0.25, "bottom": 0.62, "left": 0.31, "right": 0.69},
                "confidence": 0.8
            }
        },
        "usage": {"input_tokens": 594, "output_tokens": 36}
    }))
}

/// One part of a multipart body: its headers (lowercased names) and content.
struct Part {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Part {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn name(&self) -> Option<&str> {
        let disposition = self.header("content-disposition")?;
        let start = disposition.find("name=\"")? + "name=\"".len();
        let end = start + disposition[start..].find('"')?;
        Some(&disposition[start..end])
    }
}

/// Splits a `multipart/form-data` body into its parts, in order.
fn multipart_parts(request: &Request) -> Vec<Part> {
    let content_type = request.headers["content-type"]
        .to_str()
        .expect("content type is ASCII");
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .expect("multipart content type has a boundary")
        .trim_matches('"');
    let delimiter = format!("--{boundary}").into_bytes();

    let mut parts = Vec::new();
    let body = &request.body;
    let mut cursor =
        find(body, &delimiter, 0).expect("body starts with a boundary") + delimiter.len();
    while !body[cursor..].starts_with(b"--") {
        cursor += 2; // CRLF after the boundary
        let headers_end = find(body, b"\r\n\r\n", cursor).expect("part has headers");
        let headers = String::from_utf8(body[cursor..headers_end].to_vec())
            .expect("part headers are UTF-8")
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let content_start = headers_end + 4;
        let next = find(body, &delimiter, content_start).expect("part is closed");
        parts.push(Part {
            headers,
            body: body[content_start..next - 2].to_vec(), // drop the CRLF before the boundary
        });
        cursor = next + delimiter.len();
    }
    parts
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| position + from)
}

#[tokio::test]
async fn uploads_images_as_multipart_parts_with_the_images_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .and(header(IMAGES_HEADER, "true"))
        .and(header_regex(
            "content-type",
            "^multipart/form-data; boundary=",
        ))
        .respond_with(box_response())
        .expect(1)
        .mount(&server)
        .await;
    let mut assets = Assets::new();
    let photo = assets.insert("img-photo", Image::png(PNG.to_vec()));
    let mut questions = Questions::new();
    let red = questions.add("red", NoulQuestion::new("Is there a red shape in `photo`?"));
    let found = questions.add("where", BoxQuestion::new("the red rectangle in `photo`"));

    let response = client_for(&server)
        .system_one(json!({"photo": photo}), questions)
        .assets(assets)
        .send()
        .await
        .unwrap();

    assert!(response.answer(&red).unwrap().noul > 0.9);
    let found = response.answer(&found).unwrap();
    assert_eq!(found.bounds, BoxCoordinates::new(0.25, 0.62, 0.31, 0.69));
    assert_eq!(found.confidence, Some(0.8));

    let requests = server.received_requests().await.unwrap();
    let parts = multipart_parts(&requests[0]);
    assert_eq!(parts.len(), 2);

    assert_eq!(parts[0].name(), Some("request"));
    assert_eq!(parts[0].header("content-type"), Some("application/json"));
    let request: Value = serde_json::from_slice(&parts[0].body).unwrap();
    assert_eq!(
        request,
        json!({
            "state": {"photo": "img-photo"},
            "model": "jev-test",
            "questions": {
                "red": {"type": "noul", "instructions": "Is there a red shape in `photo`?"},
                "where": {"type": "box", "instructions": "the red rectangle in `photo`"}
            },
            "assets": {"img-photo": {"type": "image", "format": "png", "part": "image-0"}}
        })
    );

    assert_eq!(parts[1].name(), Some("image-0"));
    assert_eq!(parts[1].header("content-type"), Some("image/png"));
    assert_eq!(parts[1].body, PNG);
}

#[tokio::test]
async fn names_one_part_per_image_in_the_order_they_were_added() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-test",
            "answers": {"same": {"type": "noul", "noul": 0.1}},
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })))
        .mount(&server)
        .await;
    let jpeg = b"\xff\xd8\xff\xe0 not a full jpeg".to_vec();
    let mut assets = Assets::new();
    let first = assets.add(Image::png(PNG.to_vec()));
    let second = assets.add(Image::jpeg(jpeg.clone()));
    let mut questions = Questions::new();
    questions.add(
        "same",
        NoulQuestion::new(format!("Is {first} the same place as {second}?")),
    );

    client_for(&server)
        .system_one("Two photos.", questions)
        .assets(assets)
        .send()
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let parts = multipart_parts(&requests[0]);
    let request: Value = serde_json::from_slice(&parts[0].body).unwrap();
    assert_eq!(request["assets"][first.as_str()]["part"], "image-0");
    assert_eq!(request["assets"][second.as_str()]["part"], "image-1");
    assert_eq!(request["assets"][second.as_str()]["format"], "jpg");
    assert_eq!(parts[2].name(), Some("image-1"));
    assert_eq!(parts[2].header("content-type"), Some("image/jpeg"));
    assert_eq!(parts[2].body, jpeg);
}

#[tokio::test]
async fn sends_the_images_again_when_retrying() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(ResponseTemplate::new(529))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .respond_with(box_response())
        .mount(&server)
        .await;
    let mut assets = Assets::new();
    let photo = assets.add(Image::png(PNG.to_vec()));
    let mut questions = Questions::new();
    questions.add("red", NoulQuestion::new("Is there a red shape in `photo`?"));
    questions.add("where", BoxQuestion::new("the red rectangle in `photo`"));

    client_for(&server)
        .system_one(json!({"photo": photo}), questions)
        .assets(assets)
        .send()
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.headers[IMAGES_HEADER], "true");
        assert_eq!(multipart_parts(request)[1].body, PNG);
    }
}

#[tokio::test]
async fn sends_the_images_header_for_a_box_question_even_without_validation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SYSTEM_ONE))
        .and(header(IMAGES_HEADER, "true"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"detail": {
            "error_type": "api_usage_error",
            "message": "Invalid image request."
        }})))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("test-key")
        .base_url(server.uri())
        .validate_requests(false)
        .build()
        .unwrap();
    let mut questions = Questions::new();
    questions.add("where", BoxQuestion::new("the cat"));

    let error = client
        .system_one("no image here", questions)
        .send()
        .await
        .unwrap_err();

    assert_eq!(error.api().unwrap().message, "Invalid image request.");
}

#[tokio::test]
async fn rejects_a_box_question_without_an_image_before_sending() {
    let server = MockServer::start().await;
    let mut questions = Questions::new();
    questions.add("where", BoxQuestion::new("the cat"));

    let error = client_for(&server)
        .system_one("no image here", questions)
        .send()
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            typesafe_client::Error::InvalidRequest(
                typesafe_client::ValidationError::BoxWithoutImage { .. }
            )
        ),
        "{error:?}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}
