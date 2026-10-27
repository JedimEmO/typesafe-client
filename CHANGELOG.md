# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0]

Support for the TypeSafe API as of 2026-10-14: images, Box questions and long context.

### Added

- Feature `images`, for TypeSafe's experimental image support. Its types may change in minor
  releases, following the API.
  - `Assets`, `Image`, `ImageFormat` and `ImageRef`: images referenced from the state,
    instructions or criteria. `SystemOneRequest::with_assets` and `SystemOneCall::assets` send
    them.
  - `BoxQuestion`, `BoxAnswer`, `BoxCoordinates` and `PixelBox`: locate something in an image,
    with `padded` and `to_pixels` for cropping.
  - The HTTP `Client` uploads images as multipart file parts, and sends
    `TypeSafe-Danger-Zone-Images: true` for requests with images or Box questions.
  - Validation of image references, file signatures and file sizes, and of Box questions in
    requests without images.
  - `FakeSystemOne::set_box`, and a whole-image default answer for unconfigured Box questions.
- Long context: `SystemOneCall::long_context` and `CallOptions::long_context` send
  `TypeSafe-Danger-Zone-Long-Context: true`.
- `SystemOneRequest::uses_images`, for custom transports that need to send the images header.
- `ApiError::is_context_length_exceeded`, for 400 responses with error type
  `max_tokens_exceeded`.
- `ApiErrorKind::PayloadTooLarge` (413) and `ApiErrorKind::UnsupportedMediaType` (415).
- `constants::IMAGES_HEADER` and `constants::LONG_CONTEXT_HEADER`.

### Changed

- An API error whose detail has an `error_type` but no `message` uses a description of the
  error type as its message, instead of the raw response body.

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

[Unreleased]: https://github.com/JedimEmO/typesafe-client/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/JedimEmO/typesafe-client/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/JedimEmO/typesafe-client/releases/tag/v0.1.0
