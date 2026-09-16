//! Requests built with the typed builders must serialize to the JSON shown in the
//! TypeSafe docs, and the documented responses must deserialize into typed answers.
//! Examples are from <https://docs.typesafe.ai/api> and
//! <https://docs.typesafe.ai/patterns/fan-out>.

#![allow(
    clippy::float_cmp,
    reason = "answers are parsed from exact JSON literals and compared to the same literals"
)]

use serde_json::{Value, json};
use typesafe_client::{
    ChoiceQuestion, Content, EnumChoice, NoulQuestion, Questions, ScoreQuestion, SystemOneRequest,
    SystemOneResponse, choice_options,
};

const TICKET: &str = "Help! My payouts have been failing for 3 days.";

choice_options! {
    enum Department {
        Billing = "billing" => "Payments, invoicing, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades, new accounts",
    }
}

fn to_json(request: &SystemOneRequest) -> Value {
    serde_json::to_value(request).expect("request serializes")
}

fn response(json: Value) -> SystemOneResponse {
    serde_json::from_value(json).expect("documented response deserializes")
}

#[test]
fn noul_request_matches_api_reference() {
    let mut questions = Questions::new();
    questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));

    assert_eq!(
        to_json(&SystemOneRequest::new(TICKET, questions).with_model("jev-latest")),
        json!({
            "state": "Help! My payouts have been failing for 3 days.",
            "model": "jev-latest",
            "questions": {
                "is_urgent": {"type": "noul", "instructions": "Does this convey urgency?"}
            }
        })
    );
}

#[test]
fn noul_request_with_criteria_matches_api_reference() {
    let mut questions = Questions::new();
    questions.add(
        "is_urgent",
        NoulQuestion::new("Does this convey urgency?")
            .with_criteria("Explicitly time-sensitive", "No urgency expressed"),
    );

    assert_eq!(
        to_json(&SystemOneRequest::new(TICKET, questions).with_model("jev-latest")),
        json!({
            "state": "Help! My payouts have been failing for 3 days.",
            "model": "jev-latest",
            "questions": {
                "is_urgent": {
                    "type": "noul",
                    "instructions": "Does this convey urgency?",
                    "criteria": {"true": "Explicitly time-sensitive", "false": "No urgency expressed"}
                }
            }
        })
    );
}

#[test]
fn choice_request_matches_api_reference_with_builder_and_enum() {
    let expected = json!({
        "state": "Help! My payouts have been failing for 3 days.",
        "model": "jev-latest",
        "questions": {
            "department": {
                "type": "choice",
                "instructions": "Which team should handle this?",
                "criteria": {
                    "billing": "Payments, invoicing, refunds",
                    "technical": "Bugs, outages, integrations",
                    "sales": "Pricing, upgrades, new accounts"
                }
            }
        }
    });

    let mut built = Questions::new();
    built.add(
        "department",
        ChoiceQuestion::new("Which team should handle this?")
            .with_option("billing", "Payments, invoicing, refunds")
            .with_option("technical", "Bugs, outages, integrations")
            .with_option("sales", "Pricing, upgrades, new accounts"),
    );
    let mut from_enum = Questions::new();
    from_enum.add(
        "department",
        EnumChoice::<Department>::new("Which team should handle this?"),
    );

    assert_eq!(
        to_json(&SystemOneRequest::new(TICKET, built).with_model("jev-latest")),
        expected
    );
    assert_eq!(
        to_json(&SystemOneRequest::new(TICKET, from_enum).with_model("jev-latest")),
        expected
    );
}

#[test]
fn score_request_matches_api_reference() {
    let mut questions = Questions::new();
    questions.add(
        "frustration",
        ScoreQuestion::new(
            "How frustrated is the customer?",
            ["Calm", "Frustrated", "Very angry"],
        ),
    );

    assert_eq!(
        to_json(&SystemOneRequest::new(TICKET, questions).with_model("jev-latest")),
        json!({
            "state": "Help! My payouts have been failing for 3 days.",
            "model": "jev-latest",
            "questions": {
                "frustration": {
                    "type": "score",
                    "instructions": "How frustrated is the customer?",
                    "criteria": ["Calm", "Frustrated", "Very angry"]
                }
            }
        })
    );
}

#[test]
fn mixed_fan_out_request_matches_patterns_page() {
    let state = "Hi, I placed an order (#98423) last Thursday and was charged twice. I also can't log in after the site update, and adding Apple Pay would be really helpful. This is getting frustrating.";
    let mut questions = Questions::new();
    questions.add(
        "category",
        ChoiceQuestion::new("Determine the broad category of this support ticket")
            .with_option(
                "bug_report",
                "The user is reporting something that is broken or producing errors",
            )
            .with_option("billing", "Charges, invoices, refunds, subscriptions")
            .with_option(
                "feature_request",
                "The user is requesting new functionality",
            )
            .with_option("account", "Login, permissions, profile, security"),
    );
    questions.add(
        "bug_severity",
        ScoreQuestion::new(
            "How severe is the reported issue",
            [
                "Cosmetic; no impact to functionality",
                "Broken or degraded feature; workaround exists",
                "Blocking issue; no workaround exists",
            ],
        ),
    );
    questions.add(
        "has_reproducible_steps",
        NoulQuestion::new("The user describes specific steps to reproduce the issue"),
    );
    questions.add(
        "refund_requested",
        NoulQuestion::new("The user is explicitly asking for a refund or credit"),
    );
    questions.add(
        "frustration",
        ScoreQuestion::new(
            "How frustrated the user appears",
            ["Calm, matter-of-fact", "Frustrated but civil", "Very angry"],
        ),
    );

    let serialized = to_json(&SystemOneRequest::new(state, questions).with_model("jev-latest"));

    assert_eq!(
        serialized["questions"],
        json!({
            "category": {
                "type": "choice",
                "instructions": "Determine the broad category of this support ticket",
                "criteria": {
                    "bug_report": "The user is reporting something that is broken or producing errors",
                    "billing": "Charges, invoices, refunds, subscriptions",
                    "feature_request": "The user is requesting new functionality",
                    "account": "Login, permissions, profile, security"
                }
            },
            "bug_severity": {
                "type": "score",
                "instructions": "How severe is the reported issue",
                "criteria": [
                    "Cosmetic; no impact to functionality",
                    "Broken or degraded feature; workaround exists",
                    "Blocking issue; no workaround exists"
                ]
            },
            "has_reproducible_steps": {"type": "noul", "instructions": "The user describes specific steps to reproduce the issue"},
            "refund_requested": {"type": "noul", "instructions": "The user is explicitly asking for a refund or credit"},
            "frustration": {
                "type": "score",
                "instructions": "How frustrated the user appears",
                "criteria": ["Calm, matter-of-fact", "Frustrated but civil", "Very angry"]
            }
        })
    );
}

#[test]
fn structured_state_keeps_nested_fields() {
    let state = json!({
        "ticket": {
            "subject": "Duplicate charge",
            "messages": [{"from": "customer", "text": "I was charged twice for order A-104."}]
        },
        "refund_policy": "Duplicate charges are eligible for a refund."
    });
    let mut questions = Questions::new();
    questions.add(
        "duplicate_charge",
        NoulQuestion::new("Does `ticket.messages[0].text` describe a duplicate charge?"),
    );

    let request = SystemOneRequest::new(state.clone(), questions).with_model("jev-latest");

    assert!(matches!(request.state, Content::Object(_)));
    assert_eq!(to_json(&request)["state"], state);
}

#[test]
fn documented_noul_response_reads_as_typed_answer() {
    let mut questions = Questions::new();
    let urgent = questions.add("is_urgent", NoulQuestion::new("Does this convey urgency?"));

    let response = response(json!({
        "model": "jev-latest",
        "answers": {"is_urgent": {"type": "noul", "noul": 0.92}},
        "usage": {"input_tokens": 312, "output_tokens": 48}
    }));

    assert_eq!(response.answer(&urgent).unwrap().noul, 0.92);
    assert_eq!(response.usage.input_tokens, 312);
}

#[test]
fn documented_choice_response_reads_as_raw_and_enum_answers() {
    // The same response read two ways: as raw strings and as the `Department` enum.
    let mut raw_questions = Questions::new();
    let raw_key = raw_questions.add(
        "department",
        ChoiceQuestion::new("Which team?").with_bare_options(["billing", "technical", "sales"]),
    );
    let mut enum_questions = Questions::new();
    let enum_key = enum_questions.add("department", EnumChoice::<Department>::new("Which team?"));

    let response = response(json!({
        "model": "jev-latest",
        "answers": {"department": {
            "type": "choice",
            "choice": "technical",
            "probabilities": {"billing": 0.08, "technical": 0.85, "sales": 0.07},
            "confidence": 0.82
        }},
        "usage": {"input_tokens": 312, "output_tokens": 48}
    }));

    response.verify(&raw_questions).unwrap();
    response.verify(&enum_questions).unwrap();

    let raw = response.answer(&raw_key).unwrap();
    assert_eq!(raw.choice, "technical");
    assert_eq!(raw.probability("billing"), Some(0.08));

    let typed = response.answer(&enum_key).unwrap();
    assert_eq!(typed.choice, Department::Technical);
    assert_eq!(typed.confidence, 0.82);
    assert_eq!(typed.probability(&Department::Sales), 0.07);
}

#[test]
fn documented_score_response_reads_as_typed_answer() {
    let mut questions = Questions::new();
    let frustration = questions.add(
        "frustration",
        ScoreQuestion::new(
            "How frustrated is the customer?",
            ["Calm", "Frustrated", "Very angry"],
        ),
    );

    let response = response(json!({
        "model": "jev-latest",
        "answers": {"frustration": {
            "type": "score",
            "score": 1.6,
            "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
            "probabilities": {"0": 0.05, "1": 0.3, "2": 0.65},
            "confidence": 0.78
        }},
        "usage": {"input_tokens": 312, "output_tokens": 48}
    }));

    let score = response.answer(&frustration).unwrap();
    assert_eq!(score.score, 1.6);
    assert_eq!(score.level_probabilities(), [0.05, 0.3, 0.65]);
    assert_eq!(score.most_likely_level(), Some(2));
    assert_eq!(
        score.level_description(2),
        Some(&Content::from("Very angry"))
    );
}
