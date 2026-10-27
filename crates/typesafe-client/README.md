# typesafe-client

[![CI](https://github.com/JedimEmO/typesafe-client/actions/workflows/ci.yml/badge.svg)](https://github.com/JedimEmO/typesafe-client/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/typesafe-client.svg)](https://crates.io/crates/typesafe-client)
[![docs.rs](https://img.shields.io/docsrs/typesafe-client)](https://docs.rs/typesafe-client)
[![MSRV 1.87](https://img.shields.io/badge/MSRV-1.87-blue)](#minimum-supported-rust-version)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

A typed, async Rust client for the [TypeSafe](https://typesafe.ai) System One API.

> **Unofficial.** This is an independent community project. It is not affiliated with,
> endorsed by, or supported by TypeSafe AI. For the API itself, see the
> [official TypeSafe documentation](https://docs.typesafe.ai).

TypeSafe's System One models, such as Jev, don't generate text. They answer narrow questions
about content you give them, and return typed answers with calibrated probabilities: the
probability that something is true, the most likely option out of a set, or a position on a
scale. This crate lets you ask those questions from Rust and read each answer back as the type
its question promises.

- **Typed end to end.** Adding a question returns a key typed by its answer, so reading a
  Choice answer as a yes/no probability doesn't compile. Enums and runtime values map straight
  to Choice options.
- **Checked both ways.** Requests are checked against the documented limits before they are
  sent. Responses are verified against the questions before you see them.
- **Production defaults.** Retries 408, 429 and 5xx responses, connection errors and timeouts
  with exponential backoff. Honors `Retry-After`, and puts a hard deadline on every call.
- **Testable.** Application code depends on the `SystemOne` trait. The `fake` feature provides
  an in-memory implementation that answers every question and records requests.
- **Light when you want it.** Without default features the crate is just the types, builders
  and validation, with no HTTP stack.

## Contents

- [Installation](#installation)
- [Quick start](#quick-start)
- [Core concepts](#core-concepts)
- [Guide](#guide): [yes/no questions](#yesno-questions),
  [choices from an enum](#choices-from-an-enum), [choices from runtime values](#choices-from-runtime-values),
  [scores](#scores), [structured state](#structured-state),
  [many questions at once](#many-questions-in-one-request), [images](#images-and-box-questions),
  [configuration](#configuring-the-client),
  [retries and timeouts](#retries-and-timeouts), [errors](#handling-errors),
  [testing](#testing-code-that-uses-the-client), [without HTTP](#using-only-the-types)
- [Examples](#examples)
- [Minimum supported Rust version](#minimum-supported-rust-version)
- [License](#license)

## Installation

```toml
[dependencies]
typesafe-client = "0.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

| Feature | Default | What it adds |
|---|---|---|
| `http` | yes | `Client`, the async HTTP client (reqwest with rustls) |
| `fake` | no | `fake::FakeSystemOne`, an in-memory implementation for tests |
| `images` | no | Images in requests and `BoxQuestion`. Experimental; see [images and Box questions](#images-and-box-questions) |

Create an API key in the [TypeSafe console](https://console.typesafe.ai/keys) and make it
available to your program:

```sh
export TYPESAFE_API_KEY="your-key"
```

## Quick start

```rust,no_run
use typesafe_client::{Client, NoulQuestion, Questions};

#[tokio::main]
async fn main() -> Result<(), typesafe_client::Error> {
    // Reads TYPESAFE_API_KEY, and optionally TYPESAFE_BASE_URL and TYPESAFE_DEFAULT_MODEL.
    let client = Client::from_env()?;

    let mut questions = Questions::new();
    let urgent = questions.add(
        "is_urgent",
        NoulQuestion::new("Does this message convey urgency?"),
    );

    let response = client
        .system_one("Help! My payouts have been failing for 3 days.", questions)
        .send()
        .await?;

    let urgent = response.answer(&urgent)?;
    println!("probability of urgency: {:.2}", urgent.noul);
    Ok(())
}
```

## Core concepts

| Concept | In this crate |
|---|---|
| **State**: the content every question is about | `Content`: a string, a JSON object or a JSON array. Strings and `serde_json` values convert directly, and any `Serialize` type goes through `Content::json`. |
| **Question**: one narrow judgment | `NoulQuestion` (yes/no), `ChoiceQuestion`, `EnumChoice` and `ValueChoice` (one of up to 255 options), `ScoreQuestion` (2 to 10 ordered levels), and with the `images` feature `BoxQuestion` (where something is in an image) |
| **Key**: how you read an answer | `Questions::add(id, question)` returns a `QuestionKey`, typed by how its answer is read |
| **Answer**: probabilities, not prose | `NoulAnswer` (`noul`), `ChoiceAnswer` and `TypedChoice<T>` (`choice`, `probabilities`, `confidence`), `ScoreAnswer` (`score`, level probabilities, `confidence`), `BoxAnswer` (`bounds`, `confidence`) |

Every question in a request sees the same state and is answered independently and in parallel,
so extra questions in one request cost little. The question id only matches answers to
questions. It is never sent to the model, so each question's instructions must make sense on
their own.

TypeSafe's guides explain how to phrase questions and choose thresholds. Start with
[primitives](https://docs.typesafe.ai/primitives), [state](https://docs.typesafe.ai/concepts/state)
and [confidence](https://docs.typesafe.ai/confidence).

## Guide

### Yes/no questions

A Noul answer is the probability that the answer is yes. Near 1 means yes and near 0 means no.
A value near 0.5 means the model is unsure, not that something is "somewhat" true. Criteria
describe what counts as each answer.

```rust
use typesafe_client::{NoulQuestion, Questions};

let mut questions = Questions::new();
let refund = questions.add(
    "refund_requested",
    NoulQuestion::new("Is the customer explicitly asking for a refund?")
        .with_criteria(
            "Asks for money back or a credit",
            "Mentions a charge without asking for money back",
        ),
);
```

### Choices from an enum

`choice_options!` declares an enum whose variants are the options, each with an optional
description. The answer comes back as that enum, with a probability for every variant and a
confidence for the distribution as a whole.

```rust
use typesafe_client::{
    ChoiceAnswer, EnumChoice, Questions, SystemOneResponse, Usage, choice_options,
};

choice_options! {
    /// Which team handles a support ticket.
    pub enum Department {
        Billing = "billing" => "Payments, invoices, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades, new accounts",
    }
}

fn main() -> Result<(), typesafe_client::Error> {
    let mut questions = Questions::new();
    let department = questions.add(
        "department",
        EnumChoice::<Department>::new("Which team should handle this ticket?"),
    );

    // A response like the one the API returns (sending is shown in the quick start).
    let response = SystemOneResponse::new("jev-latest", Usage::default()).with_answer(
        &department,
        ChoiceAnswer::new(
            "technical",
            0.82,
            [("billing", 0.08), ("technical", 0.85), ("sales", 0.07)],
        ),
    );

    let department = response.answer(&department)?;
    let queue = match department.choice {
        _ if department.confidence < 0.5 => "human review",
        Department::Billing => "billing",
        Department::Technical => "engineering",
        Department::Sales => "sales",
    };
    assert_eq!(queue, "engineering");
    Ok(())
}
```

Descriptions can also be structured JSON, which helps separate similar options:
`Billing = "billing" => serde_json::json!({ "what": "...", "not_for": "..." })`.

### Choices from runtime values

`ValueChoice` offers values that are only known at runtime, such as line numbers or record ids.
Each value is sent as its `Display` text and parsed back with `FromStr`. Put what each value
refers to in the state.

```rust
use typesafe_client::{NoulQuestion, Questions, ValueChoice};

let lines = [
    "Refunds are issued within 5 business days.",
    "Contact support by email.",
    "You can cancel your plan at any time.",
];
let document: String = lines
    .iter()
    .enumerate()
    .map(|(number, line)| format!("{number}| {line}\n"))
    .collect();

let mut questions = Questions::new();
let line = questions.add(
    "line",
    ValueChoice::new("Which line says how long refunds take?", 0..lines.len()),
);
// Choice probabilities always sum to 1, so ask separately whether any line fits.
let answered = questions.add(
    "answered",
    NoulQuestion::new("Does any line say how long refunds take?"),
);
// Send `document` as the state. `response.answer(&line)?` is a `TypedChoice<usize>`,
// and `.top(3)` returns the three most likely line numbers.
```

### Scores

A Score rates the state on 2 to 10 ordered levels. The answer's `score` is probability-weighted,
so it can fall between levels. `nearest_level()` rounds it when your code needs a single
outcome. `normalized()` scales it to 0–1 so scores with different numbers of levels can be
combined.

```rust
use typesafe_client::{Questions, ScoreAnswer, ScoreQuestion};

let mut questions = Questions::new();
let severity = questions.add(
    "severity",
    ScoreQuestion::new(
        "How severe is the reported issue?",
        ["Cosmetic", "Degraded, with a workaround", "Blocking, no workaround"],
    ),
);

// An answer like the one the API returns:
let severity = ScoreAnswer::new(1.3, 0.62, ["Cosmetic", "Degraded", "Blocking"], [0.0, 0.7, 0.3]);
assert_eq!(severity.nearest_level(), 1);
assert!((severity.normalized() - 0.65).abs() < 1e-9);
```

### Structured state

Give the model named, related context. Refer to parts of it by backticked paths in your
instructions.

```rust
use serde::Serialize;
use typesafe_client::{Content, NoulQuestion, Questions, SystemOneRequest};

#[derive(Serialize)]
struct Ticket {
    subject: String,
    messages: Vec<String>,
}

#[derive(Serialize)]
struct State<'a> {
    ticket: &'a Ticket,
    refund_policy: &'a str,
}

fn main() -> Result<(), typesafe_client::Error> {
    let ticket = Ticket {
        subject: "Duplicate charge".into(),
        messages: vec!["I was charged twice for order A-104.".into()],
    };
    let state = Content::json(&State {
        ticket: &ticket,
        refund_policy: "Duplicate charges are refunded in full.",
    })?;

    let mut questions = Questions::new();
    questions.add(
        "policy_supports_refund",
        NoulQuestion::new("Does `refund_policy` support a refund for `ticket.messages`?"),
    );

    let request = SystemOneRequest::new(state, questions);
    request.validate()?;
    Ok(())
}
```

Object fields are sent in `serde_json` map order, which is sorted by key unless your
application enables `serde_json`'s `preserve_order` feature.

### Many questions in one request

Ask every question you might need in one request, including speculative ones, then use only
the answers that apply. Adding questions barely changes the response time.

```rust
use typesafe_client::{NoulQuestion, Questions};

let hazards = [
    "asks the reader for a password",
    "offers an unexpected prize or payment",
    "pressures the reader to act immediately",
];

let mut questions = Questions::new();
let keys: Vec<_> = hazards
    .iter()
    .map(|hazard| {
        questions.add(
            format!("hazard::{hazard}"),
            NoulQuestion::new(format!("Does `message.body` {hazard}?")),
        )
    })
    .collect();

assert_eq!(questions.len(), keys.len());
```

A request has two token limits: 64k for the state and all its questions together, and 32k for
the state plus any one question. `.long_context()` on a call raises both to 262k. It costs the
same per token but is slower and less accurate, so turn it on only for the requests that need
it. See [models](https://docs.typesafe.ai/models) for the limits of each model.

### Images and Box questions

Images and Box questions need the `images` feature. TypeSafe marks both as experimental: the
request format, the answers and the limits can change between API releases. The types behind
the feature follow those changes, so they may change in minor releases of this crate.

```toml
[dependencies]
typesafe-client = { version = "0.2", features = ["images"] }
```

An image can go anywhere text can go. Add it to the request's `Assets`, which returns a
reference string, and write the reference where the image belongs:

```rust,no_run
# #[cfg(not(feature = "images"))]
# fn main() {}
# #[cfg(feature = "images")]
use serde_json::json;
# #[cfg(feature = "images")]
use typesafe_client::{Assets, BoxQuestion, Client, Image, NoulQuestion, Questions};

# #[cfg(feature = "images")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::from_env()?;
    // Upright, and 280 to 1568 pixels on each side.
    let (width, height) = (1568, 1176);
    let mut assets = Assets::new();
    let kitchen = assets.add(Image::jpeg(std::fs::read("kitchen.jpg")?));

    let mut questions = Questions::new();
    let visible = questions.add(
        "mug_visible",
        NoulQuestion::new("Is there a coffee mug in `photo`?"),
    );
    let mug = questions.add("mug", BoxQuestion::new("the coffee mug in `photo`"));

    let response = client
        .system_one(json!({"photo": kitchen}), questions)
        .assets(assets)
        .send()
        .await?;

    if response.answer(&visible)?.noul > 0.5 {
        let mug = response.answer(&mug)?;
        let crop = mug.bounds.padded(0.05).to_pixels(width, height);
        println!("mug at {crop:?}, confidence {:?}", mug.confidence);
    }
    Ok(())
}
```

Notice the Noul next to the Box. A Box always answers with a box, even when the target isn't
in the image, so a yes/no question in the same request decides whether to use it. The box's
edges are fractions of the image size; `to_pixels` converts them for one size, and `padded`
moves each edge out before cropping.

A reference also works inside text, as in `format!("Is this the dog in {poster}?")`, and as a
Choice option's description. Each use sends another copy of the image to the model, and every
copy counts toward the size and token limits. So an image that several questions look at
belongs in the state once, referred to by path, like `photo` above.

The client uploads each image as a multipart file part, and sends the
`TypeSafe-Danger-Zone-Images: true` header for every request with images or a Box question.
Validation checks that each image is referenced somewhere and is a PNG or JPEG of at most
16 MiB. The API checks the rest, including the pixel size. It also ignores the EXIF
orientation, so rotate photos upright before sending them. TypeSafe's
[images guide](https://docs.typesafe.ai/images) lists every limit.

### Configuring the client

`Client::from_env()` reads these variables. `Client::builder()` sets the same values in code,
and explicit settings win.

| Variable | Builder method | Default |
|---|---|---|
| `TYPESAFE_API_KEY` | `api_key` | required |
| `TYPESAFE_BASE_URL` | `base_url` | `https://api.typesafe.ai` |
| `TYPESAFE_DEFAULT_MODEL` | `default_model` | `jev-latest` |

```rust,no_run
use std::time::Duration;
use typesafe_client::{Client, RetryPolicy};

fn main() -> Result<(), typesafe_client::Error> {
    let client = Client::builder()
        .api_key(std::env::var("MY_APP_TYPESAFE_KEY").unwrap_or_default())
        .ignore_env() // don't read TYPESAFE_* variables
        .default_model("jev-latest") // pin a concrete model for results you compare over time
        .timeout(Duration::from_secs(5)) // per attempt
        .retry(RetryPolicy::default().with_max_retries(4))
        .build()?;

    // Calls can override the model, timeout and retry policy:
    let call = client
        .system_one("state", typesafe_client::Questions::new())
        .model("jev-latest")
        .timeout(Duration::from_secs(2));
    drop(call);
    Ok(())
}
```

`Client` is cheap to clone, and clones share one connection pool. A call made with
`system_one` owns its own clone, so it can be spawned with `tokio::spawn(call.send())`, stored,
or cloned to send again. To use a preconfigured `reqwest` client, for example with a proxy,
pass it to `ClientBuilder::http_client`; the crate re-exports the `reqwest` version it uses.

### Retries and timeouts

| Setting | Default |
|---|---|
| Retries after the first attempt | 2 |
| Retried failures | HTTP 408, 429 and 5xx (including 529); connection errors; timeouts |
| Backoff | 0.5 s, doubling up to 5 s, minus up to 25% jitter |
| `retry-after-ms` / `Retry-After` | honored up to 60 s; longer values fall back to the backoff |
| Timeout per attempt | 10 s |
| Deadline for the whole call | 30 s: attempts are cut short to fit it |

Adjust these with `RetryPolicy`'s `with_*` methods, or turn retries off with
`RetryPolicy::disabled()`.

### Handling errors

`Error` separates problems you fix in code from ones worth retrying later.

```rust,no_run
use typesafe_client::{ApiErrorKind, Client, Error, NoulQuestion, Questions};

#[tokio::main]
async fn main() {
    let client = Client::from_env().expect("TYPESAFE_API_KEY is set");
    let mut questions = Questions::new();
    let spam = questions.add(
        "is_spam",
        NoulQuestion::new("Is this message unsolicited advertising?"),
    );

    match client.system_one("Cheap watches, today only!", questions).send().await {
        Ok(response) => {
            let spam = response.answer(&spam).expect("verified responses answer every question");
            println!("spam: {:.2}", spam.noul);
        }
        // Broke a documented limit; nothing was sent.
        Err(Error::InvalidRequest(problem)) => eprintln!("fix the request: {problem}"),
        Err(error) if error.api().is_some_and(|api| api.kind == ApiErrorKind::Authentication) => {
            eprintln!("check TYPESAFE_API_KEY");
        }
        // Still failing after the built-in retries.
        Err(error) if error.is_retryable() => {
            eprintln!("temporary failure: {error} (request id {:?})", error.request_id());
        }
        Err(error) => eprintln!("request failed: {error}"),
    }
}
```

For a rejected request body (HTTP 422), `error.api()` gives the `ApiError`, and its
`field_errors()` list what the server rejected. A request longer than the model's context fails
with an `ApiError` whose `is_context_length_exceeded()` is true.

### Testing code that uses the client

Let your code depend on `Arc<dyn SystemOne>`. In production that's the `Client`; in tests it's
`FakeSystemOne`.

```toml
[dev-dependencies]
typesafe-client = { version = "0.2", features = ["fake"] }
```

```rust
use std::sync::Arc;
use typesafe_client::fake::FakeSystemOne;
use typesafe_client::{Error, NoulQuestion, Questions, SystemOne};

struct Moderator {
    typesafe: Arc<dyn SystemOne>, // Arc::new(Client::from_env()?) in production
}

impl Moderator {
    async fn should_hide(&self, message: &str) -> Result<bool, Error> {
        let mut questions = Questions::new();
        let spam = questions.add(
            "is_spam",
            NoulQuestion::new("Is this message unsolicited advertising?"),
        );
        let response = self.typesafe.system_one(message, questions).send().await?;
        Ok(response.answer(&spam)?.noul > 0.9)
    }
}

// In your test suite this would be a #[tokio::test].
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let fake = Arc::new(FakeSystemOne::new());
    let moderator = Moderator { typesafe: fake.clone() };

    // Questions without a configured answer are maximally uncertain: a Noul answers 0.5.
    assert!(!moderator.should_hide("See you at lunch").await?);

    fake.set_noul("is_spam", 0.97);
    assert!(moderator.should_hide("Cheap watches, today only!").await?);

    assert_eq!(fake.request_count(), 2);
    assert_eq!(
        fake.last_request().unwrap().state.as_text(),
        Some("Cheap watches, today only!")
    );
    Ok(())
}
```

The fake behaves like a careful API:

- **Configure answers:** `set_noul`, `set_choice`, `set_choice_probabilities`, `set_score`,
  `set_box` (feature `images`) or `set_answer`, by question id or key.
- **Script outcomes:** `push_response`, or `push_error` with an `ApiError` or
  `Error::Timeout(TransportError::new("timed out"))`.
- **Inspect what was sent:** `requests`, `last_request`, `request_count`.
- **Checks like the real client:** it validates requests and verifies answers. A configured
  option the question doesn't have fails the call instead of producing an impossible response.

### Using only the types

```toml
[dependencies]
typesafe-client = { version = "0.2", default-features = false }
```

Without `http` you still get questions, keys, answers, validation, response verification and
the `SystemOne` trait. That's useful in a domain crate that shouldn't depend on an HTTP client,
or with your own transport:

```rust
use typesafe_client::{
    CallOptions, Error, ModelList, SystemOne, SystemOneRequest, SystemOneResponse, async_trait,
};

struct MyTransport;

#[async_trait]
impl SystemOne for MyTransport {
    async fn send(
        &self,
        request: &SystemOneRequest,
        _options: &CallOptions,
    ) -> Result<SystemOneResponse, Error> {
        request.validate()?;
        // POST the request (filling in `model` if it is `None`) to
        // `typesafe_client::constants::SYSTEM_ONE_PATH`, turn failures into
        // `ApiError::from_response`, then return `response` after
        // `response.verify(&request.questions)?`.
        unimplemented!()
    }

    async fn list_models(&self) -> Result<ModelList, Error> {
        unimplemented!()
    }
}
```

## Examples

The repository has runnable examples:

- [`triage`](https://github.com/JedimEmO/typesafe-client/blob/main/crates/typesafe-client/examples/triage.rs)
  classifies a support ticket with several questions in one request, then routes it in code.
- [`audit`](https://github.com/JedimEmO/typesafe-client/blob/main/crates/typesafe-client/examples/audit.rs)
  reviews every source file in a directory for cleanliness and responsibilities, and suggests
  whether to refactor or split each one. `--dry-run` shows the requests without an API key.

```sh
TYPESAFE_API_KEY=... cargo run -p typesafe-client --example triage
TYPESAFE_API_KEY=... cargo run -p typesafe-client --example audit -- crates/typesafe-client/src
```

## Minimum supported Rust version

Rust 1.87. CI checks this version, and raising it counts as a minor change.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/JedimEmO/typesafe-client/blob/main/LICENSE-APACHE)
or [MIT license](https://github.com/JedimEmO/typesafe-client/blob/main/LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions. See
[CONTRIBUTING.md](https://github.com/JedimEmO/typesafe-client/blob/main/CONTRIBUTING.md) for
how to build and test.
