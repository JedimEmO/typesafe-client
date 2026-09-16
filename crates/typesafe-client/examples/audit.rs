//! Reviews every source file of a given extension under a directory and asks a
//! TypeSafe System One model to judge each file: how clean it is, how many
//! responsibilities it carries, and whether it should be refactored or split.
//!
//! All four questions for a file are asked in one speculative fan-out request.
//!
//! ```sh
//! # Human-readable, `.rs` files by default
//! TYPESAFE_API_KEY=... cargo run -p typesafe-client --example audit -- crates/typesafe-client/src
//!
//! # Only `.py` files, machine-readable (one JSON object per line)
//! TYPESAFE_API_KEY=... cargo run -p typesafe-client --example audit -- --ext py --json .
//!
//! # No API key needed: list the files and the requests that would be sent
//! cargo run -p typesafe-client --example audit -- --dry-run --ext rs src
//! ```

use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use typesafe_client::{
    ChoiceOptions, Client, Content, EnumChoice, NoulAnswer, NoulQuestion, QuestionKey, Questions,
    ScoreAnswer, ScoreQuestion, SystemOneRequest, TypedChoice, choice_options,
};

/// Extensions scanned when `--ext` is not given.
const DEFAULT_EXT: &str = "rs";
/// Characters sent per file unless `--max-chars` says otherwise.
const DEFAULT_MAX_CHARS: usize = 20_000;
/// Highest value a Score answer can take, for display only.
const MAX_SCORE: usize = 4;
/// Directories the walk never descends into.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules", "dist", "build", "vendor"];

const CLEANLINESS_LEVELS: [&str; 5] = [
    "Hard to follow; tangled, inconsistent or confusing",
    "Messy in places; several unclear sections",
    "Acceptable but could be clearer",
    "Clean and consistent",
    "Exemplary; clear, consistent and well organized",
];

const RESPONSIBILITY_LEVELS: [&str; 5] = [
    "A single clear responsibility",
    "Mostly one responsibility, with minor incidental logic",
    "Two or three distinguishable responsibilities",
    "Several responsibilities mixed together",
    "Many unrelated responsibilities",
];

choice_options! {
    enum Action {
        Keep = "keep" => "No change needed; the file is cohesive and clear",
        MinorCleanup = "minor_cleanup" => "Small in-file readability or consistency fixes",
        RefactorInPlace = "refactor_in_place" => "Restructure within the same file; it should stay one file",
        Split = "split" => "Split into two or more smaller modules or files",
    }
}

const HELP: &str = "\
Usage: audit [OPTIONS] [DIR]

Reviews every source file under DIR (recursive) with TypeSafe System One.

Arguments:
  DIR                 Directory to scan (default: .)

Options:
  --ext <EXTS>        Comma-separated extensions, case-insensitive (default: rs)
  --model <MODEL>     Model to use (default: the client's default model)
  --max-chars <N>     Truncate each file to N characters (default: 20000)
  --json              Emit one JSON object per file on stdout
  --dry-run           List matching files and the requests, without calling the API
  -h, --help          Show this help

Environment:
  TYPESAFE_API_KEY        Required unless --dry-run
  TYPESAFE_BASE_URL       Optional API base URL override
  TYPESAFE_DEFAULT_MODEL  Optional default model override
";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    dir: PathBuf,
    extensions: Vec<String>,
    model: Option<String>,
    max_chars: usize,
    json: bool,
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{HELP}");
            return Ok(());
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{HELP}");
            std::process::exit(2);
        }
    };
    run(args).await
}

async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let files = collect_files(&args.dir, &args.extensions)?;
    if files.is_empty() {
        eprintln!(
            "no {:?} files under {}",
            args.extensions,
            args.dir.display()
        );
        return Ok(());
    }

    if args.dry_run {
        run_dry_run(&args, &files);
        return Ok(());
    }

    let client = Client::from_env()?;
    let mut totals = Totals::default();
    for path in &files {
        let audit = match audit_file(&client, path, &args).await {
            Ok(audit) => audit,
            Err(message) => FileAudit::failed(path.display().to_string(), message),
        };
        totals.record(&audit);
        if args.json {
            println!("{}", serde_json::to_string(&audit)?);
        } else {
            print_human(&audit);
        }
    }
    print_summary(&totals, args.json);
    Ok(())
}

fn parse_args<I>(args: I) -> Result<Option<Args>, String>
where
    I: IntoIterator<Item = String>,
{
    let mut dir: Option<PathBuf> = None;
    let mut extensions: Option<Vec<String>> = None;
    let mut model: Option<String> = None;
    let mut max_chars = DEFAULT_MAX_CHARS;
    let mut json = false;
    let mut dry_run = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--json" => json = true,
            "--dry-run" => dry_run = true,
            "--ext" => {
                let value = args.next().ok_or("--ext needs a value")?;
                let parsed = parse_extensions(&value);
                if parsed.is_empty() {
                    return Err("--ext needs at least one extension".to_owned());
                }
                extensions = Some(parsed);
            }
            "--model" => model = Some(args.next().ok_or("--model needs a value")?),
            "--max-chars" => {
                let value = args.next().ok_or("--max-chars needs a value")?;
                max_chars = value
                    .parse()
                    .map_err(|_| format!("invalid --max-chars value `{value}`"))?;
            }
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            other => {
                if dir.is_some() {
                    return Err(format!("unexpected argument `{other}`"));
                }
                dir = Some(PathBuf::from(other));
            }
        }
    }

    Ok(Some(Args {
        dir: dir.unwrap_or_else(|| PathBuf::from(".")),
        extensions: extensions.unwrap_or_else(|| vec![DEFAULT_EXT.to_owned()]),
        model,
        max_chars,
        json,
        dry_run,
    }))
}

fn parse_extensions(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|part| part.trim().trim_start_matches('.'))
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn collect_files(dir: &Path, extensions: &[String]) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    walk(dir, extensions, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk(dir: &Path, extensions: &[String], files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let skipped = SKIPPED_DIRS
                .iter()
                .any(|candidate| name.eq_ignore_ascii_case(candidate));
            if !skipped {
                walk(&entry.path(), extensions, files)?;
            }
        } else if file_type.is_file() && has_extension(&entry.path(), extensions) {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn has_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        extensions
            .iter()
            .any(|allowed| ext.eq_ignore_ascii_case(allowed))
    })
}

fn language_of(path: &Path) -> String {
    path.extension()
        .and_then(OsStr::to_str)
        .map_or_else(String::new, str::to_ascii_lowercase)
}

fn truncated_source(source: &str, max_chars: usize) -> (String, bool) {
    let total = source.chars().count();
    if total <= max_chars {
        return (source.to_owned(), false);
    }
    let end = source
        .char_indices()
        .nth(max_chars)
        .map_or(source.len(), |(index, _)| index);
    let body = &source[..end];
    (
        format!("{body}\n\n[truncated: showing first {max_chars} of {total} characters]"),
        true,
    )
}

fn build_request(
    path: &Path,
    args: &Args,
    model: &str,
) -> Result<(SystemOneRequest, FileKeys), String> {
    let bytes = fs::read(path).map_err(|error| format!("read failed: {error}"))?;
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let language = language_of(path);
    let (source, truncated) = truncated_source(&raw, args.max_chars);
    let state = Content::json(&FileState {
        path: path.display().to_string(),
        language: language.clone(),
        truncated,
        source,
    })
    .map_err(|error| format!("encode state failed: {error}"))?;
    let (questions, keys) = build_questions(&language);
    Ok((
        SystemOneRequest::new(state, questions).with_model(model),
        keys,
    ))
}

fn build_questions(language: &str) -> (Questions, FileKeys) {
    let context = format!(
        "You are reviewing a single {language} source file from a software project. \
         Judge the file as a whole: how clear and consistent it is, how many distinct \
         responsibilities it carries, and whether it should be refactored or split. \
         Use only the file's own source as evidence."
    );

    let mut questions = Questions::new();
    let cleanliness = questions.add(
        "cleanliness",
        ScoreQuestion::new(
            format!("{context} Overall, how clean is this file?"),
            CLEANLINESS_LEVELS,
        ),
    );
    let responsibilities = questions.add(
        "responsibilities",
        ScoreQuestion::new(
            format!("{context} How many distinct responsibilities does this file have?"),
            RESPONSIBILITY_LEVELS,
        ),
    );
    let action = questions.add(
        "refactor_action",
        EnumChoice::<Action>::new(format!(
            "{context} What should be done with this file? Choose `split` only when it \
             genuinely mixes concerns that deserve separate modules."
        )),
    );
    let should_split = questions.add(
        "should_split",
        NoulQuestion::new(format!(
            "{context} Should this file be split into two or more smaller modules?"
        ))
        .with_criteria(
            "The file mixes concerns that would be clearer as separate modules",
            "The file is cohesive enough to stay one module",
        ),
    );

    (
        questions,
        FileKeys {
            cleanliness,
            responsibilities,
            action,
            should_split,
        },
    )
}

async fn audit_file(client: &Client, path: &Path, args: &Args) -> Result<FileAudit, String> {
    let model = args
        .model
        .as_deref()
        .unwrap_or_else(|| client.default_model());
    let (request, keys) = build_request(path, args, model)?;
    let response = client
        .evaluate(&request)
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let cleanliness = response
        .answer(&keys.cleanliness)
        .map_err(|error| format!("cleanliness answer failed: {error}"))?;
    let responsibilities = response
        .answer(&keys.responsibilities)
        .map_err(|error| format!("responsibilities answer failed: {error}"))?;
    let action = response
        .answer(&keys.action)
        .map_err(|error| format!("refactor_action answer failed: {error}"))?;
    let should_split = response
        .answer(&keys.should_split)
        .map_err(|error| format!("should_split answer failed: {error}"))?;

    Ok(FileAudit {
        path: path.display().to_string(),
        error: None,
        cleanliness: Some(score_report(cleanliness)),
        responsibilities: Some(score_report(responsibilities)),
        refactor_action: Some(action_report(&action)),
        should_split: Some(should_split.noul),
        usage: UsageReport {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
        },
    })
}

fn run_dry_run(args: &Args, files: &[PathBuf]) {
    let model = args
        .model
        .as_deref()
        .unwrap_or(typesafe_client::client::DEFAULT_MODEL);
    println!("{} files · model {model}", files.len());
    for path in files {
        match build_request(path, args, model) {
            Ok((request, _)) => match serde_json::to_string_pretty(&request) {
                Ok(json) => println!("{}\n{json}", path.display()),
                Err(error) => println!("{}: ! encode failed: {error}", path.display()),
            },
            Err(message) => println!("{}: ! {message}", path.display()),
        }
    }
}

fn score_report(answer: &ScoreAnswer) -> ScoreReport {
    let level = answer.most_likely_level();
    ScoreReport {
        score: answer.score,
        confidence: answer.confidence,
        level,
        label: level.and_then(|level| answer.level_description(level).map(content_text)),
    }
}

fn action_report(answer: &TypedChoice<Action>) -> ActionReport {
    ActionReport {
        action: answer.choice.name().to_owned(),
        confidence: answer.confidence,
        ranked: answer
            .ranked()
            .into_iter()
            .map(|(option, probability)| (option.name().to_owned(), probability))
            .collect(),
    }
}

fn content_text(content: &Content) -> String {
    match content {
        Content::Text(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn print_human(audit: &FileAudit) {
    println!("{}", audit.path);
    if let Some(error) = &audit.error {
        println!("  ! {error}");
        return;
    }
    if let Some(cleanliness) = &audit.cleanliness {
        print_score("cleanliness", cleanliness);
    }
    if let Some(responsibilities) = &audit.responsibilities {
        print_score("responsibilities", responsibilities);
    }
    if let Some(action) = &audit.refactor_action {
        println!(
            "  {:<16} {}  (confidence {:.2})",
            "action", action.action, action.confidence
        );
        let ranked = action
            .ranked
            .iter()
            .map(|(name, probability)| format!("{name} {probability:.2}"))
            .collect::<Vec<_>>()
            .join(" · ");
        println!("    {ranked}");
    }
    if let Some(probability) = audit.should_split {
        println!("  {:<16} {:.2}", "should_split", probability);
    }
    if let Some(recommendation) = recommendation(audit) {
        println!("  -> {recommendation}");
    }
}

fn print_score(label: &str, report: &ScoreReport) {
    let suffix = match (&report.level, &report.label) {
        (Some(level), Some(text)) => format!(" (level {level}: {text})"),
        (Some(level), None) => format!(" (level {level})"),
        _ => String::new(),
    };
    println!(
        "  {label:<16} {:.1}/{MAX_SCORE}  (confidence {:.2}){suffix}",
        report.score, report.confidence
    );
}

fn recommendation(audit: &FileAudit) -> Option<&'static str> {
    if audit
        .should_split
        .is_some_and(|probability| probability >= 0.5)
    {
        return Some("SPLIT");
    }
    match audit.refactor_action.as_ref()?.action.as_str() {
        "split" => Some("SPLIT"),
        "refactor_in_place" => Some("REFACTOR"),
        "minor_cleanup" => Some("CLEAN UP"),
        "keep" => Some("KEEP"),
        _ => None,
    }
}

fn print_summary(totals: &Totals, json: bool) {
    let line = format!(
        "{files} files · {failed} failed · {split} split · {refactor} refactor · \
         {cleanup} cleanup · {keep} keep · {input} input tokens, {output} output tokens",
        files = totals.files,
        failed = totals.failed,
        split = totals.split,
        refactor = totals.refactor_in_place,
        cleanup = totals.minor_cleanup,
        keep = totals.keep,
        input = totals.input_tokens,
        output = totals.output_tokens,
    );
    if json {
        eprintln!("{line}");
    } else {
        println!("\n{line}");
    }
}

struct FileKeys {
    cleanliness: QuestionKey<ScoreAnswer>,
    responsibilities: QuestionKey<ScoreAnswer>,
    action: QuestionKey<TypedChoice<Action>>,
    should_split: QuestionKey<NoulAnswer>,
}

#[derive(Serialize)]
struct FileState {
    path: String,
    language: String,
    truncated: bool,
    source: String,
}

#[derive(Debug, Serialize)]
struct ScoreReport {
    score: f64,
    confidence: f64,
    level: Option<usize>,
    label: Option<String>,
}

#[derive(Debug, Serialize)]
struct ActionReport {
    action: String,
    confidence: f64,
    ranked: Vec<(String, f64)>,
}

#[derive(Debug, Default, Serialize)]
struct UsageReport {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Serialize)]
struct FileAudit {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanliness: Option<ScoreReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    responsibilities: Option<ScoreReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refactor_action: Option<ActionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    should_split: Option<f64>,
    usage: UsageReport,
}

impl FileAudit {
    fn failed(path: String, error: String) -> Self {
        Self {
            path,
            error: Some(error),
            cleanliness: None,
            responsibilities: None,
            refactor_action: None,
            should_split: None,
            usage: UsageReport::default(),
        }
    }
}

#[derive(Default)]
struct Totals {
    files: usize,
    failed: usize,
    keep: usize,
    minor_cleanup: usize,
    refactor_in_place: usize,
    split: usize,
    input_tokens: u64,
    output_tokens: u64,
}

impl Totals {
    fn record(&mut self, audit: &FileAudit) {
        self.files += 1;
        self.input_tokens += audit.usage.input_tokens;
        self.output_tokens += audit.usage.output_tokens;
        if audit.error.is_some() {
            self.failed += 1;
            return;
        }
        match audit
            .refactor_action
            .as_ref()
            .map(|report| report.action.as_str())
        {
            Some("keep") => self.keep += 1,
            Some("minor_cleanup") => self.minor_cleanup += 1,
            Some("refactor_in_place") => self.refactor_in_place += 1,
            Some("split") => self.split += 1,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_current_dir_and_rs() {
        let args = parse_args(Vec::<String>::new()).unwrap().unwrap();
        assert_eq!(args.dir, PathBuf::from("."));
        assert_eq!(args.extensions, ["rs"]);
        assert_eq!(args.max_chars, DEFAULT_MAX_CHARS);
        assert!(!args.json);
        assert!(!args.dry_run);
    }

    #[test]
    fn parses_flags_and_positional_dir() {
        let args = parse_args(
            [
                "--ext",
                "AG,.py",
                "--json",
                "--dry-run",
                "--model",
                "jev-test",
                "src",
            ]
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(args.extensions, ["ag", "py"]);
        assert_eq!(args.dir, PathBuf::from("src"));
        assert_eq!(args.model.as_deref(), Some("jev-test"));
        assert!(args.json);
        assert!(args.dry_run);
    }

    #[test]
    fn returns_none_for_help() {
        assert!(parse_args(["--help".to_owned()]).unwrap().is_none());
    }

    #[test]
    fn rejects_unknown_options_and_empty_ext() {
        assert!(parse_args(["--nope".to_owned()]).is_err());
        assert!(parse_args(["--ext".to_owned(), String::new()]).is_err());
    }

    #[test]
    fn truncates_on_character_boundaries() {
        let (short, truncated) = truncated_source("hello", 10);
        assert_eq!(short, "hello");
        assert!(!truncated);

        let (long, truncated) = truncated_source("aé漢b", 2);
        assert!(truncated);
        assert!(long.starts_with("aé"));
        assert!(long.contains("showing first 2 of 4 characters"));
    }

    #[test]
    fn matches_extensions_case_insensitively() {
        let allowed = vec!["rs".to_owned()];
        assert!(has_extension(Path::new("src/main.rs"), &allowed));
        assert!(has_extension(Path::new("src/main.RS"), &allowed));
        assert!(!has_extension(Path::new("src/main.py"), &allowed));
    }

    #[test]
    fn parses_extensions_without_leading_dots() {
        assert_eq!(parse_extensions(" .rs , PY ,"), ["rs", "py"]);
    }
}
