//! Checks the crate's wire types against the OpenAPI spec published at
//! <https://api.typesafe.ai/openapi.json>, vendored in `tests/fixtures/openapi.json`.
//! The ignored `vendored_openapi_spec_matches_live_spec` test in `live.rs` detects drift.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde::Serialize;
use serde_json::{Value, json};
use typesafe_client::{
    ChoiceQuestion, Content, EnumChoice, ModelList, NoulQuestion, Questions, ScoreQuestion,
    SystemOneRequest, SystemOneResponse, choice_options,
};

static SPEC: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("fixtures/openapi.json")).expect("vendored spec is JSON")
});

choice_options! {
    enum Tone {
        Calm = "calm" => "A neutral or polite message",
        Angry = "angry" => json!({"what": "An upset or hostile message", "not_for": ["sarcasm"]}),
        Excited = "excited",
    }
}

fn validator(schema_name: &str) -> jsonschema::Validator {
    let schema = json!({
        "$ref": format!("#/components/schemas/{schema_name}"),
        "components": SPEC["components"].clone(),
    });
    jsonschema::validator_for(&schema).expect("spec schema compiles")
}

#[track_caller]
fn assert_conforms(schema_name: &str, instance: &Value) {
    let validator = validator(schema_name);
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{schema_name} violations:\n{}\ninstance: {instance:#}",
        errors.join("\n")
    );
}

fn every_question_shape() -> Questions {
    let mut questions = Questions::new();
    questions.add("plain_noul", NoulQuestion::new("Is this spam?"));
    questions.add(
        "noul_with_criteria",
        NoulQuestion::new(json!({"task": "Identify unsolicited advertising."})).with_criteria(
            "Unsolicited advertising",
            json!(["A legitimate conversation"]),
        ),
    );
    questions.add(
        "choice",
        ChoiceQuestion::new("Which team?")
            .with_option("billing", "Payments")
            .with_option("technical", json!({"what": "Bugs", "examples": ["Outage"]}))
            .with_bare_option("sales"),
    );
    questions.add("enum_choice", EnumChoice::<Tone>::new("What is the tone?"));
    questions.add(
        "score",
        ScoreQuestion::new(
            json!(["Rate urgency", "Consider deadlines"]),
            [
                Content::from("Can wait"),
                Content::from(json!({"level": "This week"})),
                Content::from(json!(["Today", "Immediately"])),
            ],
        ),
    );
    questions
}

#[test]
fn requests_with_every_question_shape_conform_to_spec() {
    #[derive(Serialize)]
    struct Ticket {
        subject: &'static str,
        messages: Vec<&'static str>,
    }

    let states = [
        Content::from("I was charged twice."),
        Content::from(json!(["Hi", "My card was charged twice."])),
        Content::json(&Ticket {
            subject: "Duplicate charge",
            messages: vec!["Please refund"],
        })
        .unwrap(),
    ];

    for state in states {
        let request = SystemOneRequest::new(state, every_question_shape()).with_model("jev-latest");
        request.validate().unwrap();
        assert_conforms("SystemOneRequest", &serde_json::to_value(&request).unwrap());
    }
}

#[test]
fn schema_rejects_request_without_model() {
    let request = json!({"state": "x", "questions": {"q": {"type": "noul", "instructions": "?"}}});

    assert!(
        !validator("SystemOneRequest").is_valid(&request),
        "validator should not accept everything"
    );
}

#[test]
fn documented_responses_conform_to_spec_and_deserialize() {
    let response = json!({
        "model": "jev-latest",
        "answers": {
            "is_urgent": {"type": "noul", "noul": 0.92},
            "department": {
                "type": "choice",
                "choice": "technical",
                "probabilities": {"billing": 0.08, "technical": 0.85, "sales": 0.07},
                "confidence": 0.82
            },
            "frustration": {
                "type": "score",
                "score": 1.6,
                "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
                "probabilities": {"0": 0.05, "1": 0.3, "2": 0.65},
                "confidence": 0.78
            }
        },
        "usage": {"input_tokens": 312, "output_tokens": 48}
    });

    assert_conforms("SystemOneResponse", &response);
    let parsed: SystemOneResponse = serde_json::from_value(response.clone()).unwrap();
    assert_eq!(serde_json::to_value(&parsed).unwrap(), response);
}

#[test]
fn schema_examples_deserialize_into_wire_types() {
    let schemas = &SPEC["components"]["schemas"];
    let first_example =
        |name: &str, field: &str| schemas[name]["properties"][field]["examples"][0].clone();

    let request = json!({
        "state": first_example("SystemOneRequest", "state"),
        "model": first_example("SystemOneRequest", "model"),
        "questions": first_example("SystemOneRequest", "questions"),
    });
    let response = json!({
        "model": first_example("SystemOneResponse", "model"),
        "answers": first_example("SystemOneResponse", "answers"),
        "usage": first_example("SystemOneResponse", "usage"),
    });
    let models = json!({"models": first_example("ModelMetadataList", "models")});

    let request: SystemOneRequest = serde_json::from_value(request).unwrap();
    request.validate().unwrap();
    serde_json::from_value::<SystemOneResponse>(response).unwrap();
    let models: ModelList = serde_json::from_value(models).unwrap();
    assert!(!models.models.is_empty());
}

#[test]
fn spec_question_and_answer_types_are_all_modelled() {
    let schemas = &SPEC["components"]["schemas"];
    let mapping_keys = |name: &str| -> BTreeSet<String> {
        schemas[name]["discriminator"]["mapping"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} has a discriminator mapping"))
            .keys()
            .cloned()
            .collect()
    };
    // Box is modelled behind the `images` feature; without it, leave it out of the spec's
    // set too.
    let without_box = |mut types: BTreeSet<String>| {
        if !cfg!(feature = "images") {
            types.remove("box");
        }
        types
    };
    let modelled: BTreeSet<String> =
        without_box(["box", "choice", "noul", "score"].map(String::from).into());

    assert_eq!(
        without_box(mapping_keys("Question")),
        modelled,
        "question types changed in the spec"
    );
    assert_eq!(
        without_box(mapping_keys("Answer")),
        modelled,
        "answer types changed in the spec"
    );
}

#[test]
fn spec_endpoints_are_the_ones_the_client_calls() {
    let paths: BTreeSet<String> = SPEC["paths"]
        .as_object()
        .expect("spec has paths")
        .keys()
        .cloned()
        .collect();

    assert_eq!(
        paths,
        ["/v1/models", "/v1/systemone"].map(String::from).into()
    );
}

/// The request and response examples the spec gives for `POST /v1/systemone`, by name.
fn endpoint_examples(content_type: &str, location: &str) -> Vec<(String, Value)> {
    let operation = &SPEC["paths"]["/v1/systemone"]["post"];
    let content = match location {
        "request" => &operation["requestBody"]["content"][content_type],
        _ => &operation["responses"]["200"]["content"][content_type],
    };
    content["examples"]
        .as_object()
        .unwrap_or_else(|| panic!("{location} has examples"))
        .iter()
        .map(|(name, example)| (name.clone(), example["value"].clone()))
        .collect()
}

#[test]
fn endpoint_examples_deserialize_validate_and_answer_their_questions() {
    let requests = endpoint_examples("application/json", "request");
    let responses = endpoint_examples("application/json", "response");
    assert!(!requests.is_empty());

    for (name, request) in requests {
        if name == "box" && !cfg!(feature = "images") {
            continue;
        }
        assert_conforms("SystemOneRequest", &request);
        let request: SystemOneRequest = serde_json::from_value(request)
            .unwrap_or_else(|error| panic!("request example `{name}`: {error}"));
        request.validate().unwrap();

        let (_, response) = responses
            .iter()
            .find(|(response_name, _)| *response_name == name)
            .unwrap_or_else(|| panic!("response example `{name}` exists"));
        let response: SystemOneResponse = serde_json::from_value(response.clone())
            .unwrap_or_else(|error| panic!("response example `{name}`: {error}"));
        response.verify(&request.questions).unwrap();
    }
}

#[cfg(feature = "images")]
mod images {
    use serde_json::json;
    use typesafe_client::{
        Assets, BoxAnswer, BoxCoordinates, BoxQuestion, Image, NoulQuestion, Questions,
        SystemOneRequest, SystemOneResponse, Usage,
    };

    use super::assert_conforms;

    const PNG: &[u8] = include_bytes!("fixtures/red-rectangle.png");

    #[test]
    fn requests_with_images_and_box_questions_conform_to_spec() {
        let mut assets = Assets::new();
        let photo = assets.add(Image::png(PNG.to_vec()));
        let badge = assets.add(Image::png(PNG.to_vec()));
        let mut questions = Questions::new();
        questions.add(
            "same_person",
            NoulQuestion::new(format!("Is the person in `photo` the one in {badge}?")),
        );
        questions.add("where", BoxQuestion::new("the person in `photo`"));
        questions.add(
            "where_structured",
            BoxQuestion::new(json!({"target": "the badge", "image": "`photo`"})),
        );
        let request = SystemOneRequest::new(json!({"photo": photo}), questions)
            .with_model("jev-latest")
            .with_assets(assets);

        request.validate().unwrap();
        assert_conforms("SystemOneRequest", &serde_json::to_value(&request).unwrap());
    }

    #[test]
    fn box_answers_conform_to_spec() {
        for confidence in [Some(0.92), None] {
            let response = SystemOneResponse::new("jev-1.14.0", Usage::new(290, 21)).with_answer(
                "mug",
                BoxAnswer::new(BoxCoordinates::new(0.25, 0.82, 0.44, 0.84), confidence),
            );

            assert_conforms(
                "SystemOneResponse",
                &serde_json::to_value(&response).unwrap(),
            );
        }
    }

    #[test]
    fn multipart_asset_shape_conforms_to_spec() {
        // The shape the HTTP client sends for each image in a multipart request.
        assert_conforms(
            "Asset",
            &json!({"type": "image", "format": "png", "part": "image-0"}),
        );
        assert_conforms(
            "Asset",
            &serde_json::to_value(Image::png(PNG.to_vec())).unwrap(),
        );
    }
}
