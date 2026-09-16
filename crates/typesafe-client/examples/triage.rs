//! Support-ticket triage from TypeSafe's speculative fan-out pattern: every question in
//! one request, then plain code decides which answers matter.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run -p typesafe-client --example triage
//! ```

use typesafe_client::{Client, EnumChoice, NoulQuestion, Questions, ScoreQuestion, choice_options};

choice_options! {
    enum Category {
        BugReport = "bug_report" => "The user is reporting something that is broken or producing errors",
        Billing = "billing" => "Charges, invoices, refunds, subscriptions",
        FeatureRequest = "feature_request" => "The user is requesting new functionality",
        Account = "account" => "Login, permissions, profile, security",
    }
}

const TICKET: &str = "Hi, I placed an order (#98423) last Thursday and was charged twice. \
    I also can't log in after the site update, and adding Apple Pay would be really helpful. \
    This is getting frustrating.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::from_env()?;

    let mut questions = Questions::new();
    let category = questions.add(
        "category",
        EnumChoice::<Category>::new("Determine the broad category of this support ticket"),
    );
    let bug_severity = questions.add(
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
    let has_repro = questions.add(
        "has_reproducible_steps",
        NoulQuestion::new("The user describes specific steps to reproduce the issue"),
    );
    let refund = questions.add(
        "refund_requested",
        NoulQuestion::new("The user is explicitly asking for a refund or credit"),
    );
    let frustration = questions.add(
        "frustration",
        ScoreQuestion::new(
            "How frustrated the user appears",
            ["Calm, matter-of-fact", "Frustrated but civil", "Very angry"],
        ),
    );

    let response = client.system_one(TICKET, questions).send().await?;

    let category = response.answer(&category)?;
    let bug_severity = response.answer(&bug_severity)?;
    let has_repro = response.answer(&has_repro)?;
    let refund = response.answer(&refund)?;
    let frustration = response.answer(&frustration)?;

    println!(
        "model {} · request {} · {} input tokens",
        response.model,
        response.request_id.as_deref().unwrap_or("-"),
        response.usage.input_tokens
    );
    println!("category (confidence {:.2}):", category.confidence);
    for (option, probability) in category.ranked() {
        println!("  {option:?}: {probability:.2}");
    }
    println!(
        "bug severity {:.2}, reproducible steps {:.2}",
        bug_severity.score, has_repro.noul
    );
    println!(
        "refund requested {:.2}, frustration {:.2}",
        refund.noul, frustration.score
    );

    let route = match category.choice {
        _ if category.confidence < 0.5 => "human review (uncertain category)",
        Category::BugReport if bug_severity.score > 1.5 && has_repro.noul > 0.6 => {
            "engineering, high severity"
        }
        Category::BugReport => "bug backlog",
        Category::Billing if refund.noul > 0.7 => "billing, refund likely",
        Category::Billing => "billing",
        Category::FeatureRequest => "feature request log",
        Category::Account => "account support",
    };
    println!("route: {route}");
    if frustration.score > 1.5 {
        println!("flag: priority response");
    }
    Ok(())
}
