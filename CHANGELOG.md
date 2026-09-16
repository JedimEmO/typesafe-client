# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0]

First release.

### Added

- Typed questions: `NoulQuestion`, `ChoiceQuestion`, `ScoreQuestion`, plus `EnumChoice` (options
  from an enum declared with `choice_options!`) and `ValueChoice` (options from runtime values).
- `Questions` with `QuestionKey`s typed by their answers, and `SystemOneResponse::answer`, which
  borrows Choice and Score answers.
- Request validation against the documented limits, and response verification against the
  questions that were sent.
- `SystemOne` trait, implemented by the HTTP `Client` and by `Arc<T>`.
- `Client` (feature `http`): environment or builder configuration, owned and spawnable
  `SystemOneCall`s, SDK-compatible retries with a hard deadline and a cap on `Retry-After`.
- `fake::FakeSystemOne` (feature `fake`) for testing code that uses the client.
- `Error` with `status`, `retry_after`, `request_id` and `is_retryable`; `ApiError::field_errors`
  for validation responses.
- Score helpers `nearest_level` and `normalized`; ranking helpers `ranked` and `top`.

[Unreleased]: https://github.com/JedimEmO/typesafe-client/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/JedimEmO/typesafe-client/releases/tag/v0.1.0
