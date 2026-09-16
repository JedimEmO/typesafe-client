# Contributing

Thanks for helping improve `typesafe-client`. Issues and pull requests are welcome.

This is an unofficial client. For questions about the TypeSafe API itself, see the
[TypeSafe documentation](https://docs.typesafe.ai).

## Development

The repository is a Cargo workspace. The crate lives in `crates/typesafe-client`.

```sh
cargo fmt --all
cargo clippy -p typesafe-client --all-targets --all-features -- -D warnings
cargo test -p typesafe-client --all-features
cargo test -p typesafe-client --no-default-features --lib --tests
```

The test suite needs no network access or API key:

| Tests | What they check |
|---|---|
| `src/**` unit tests | Builders, validation, answer reading, retry and error logic |
| `tests/docs_examples.rs` | Requests serialize to the JSON in TypeSafe's API reference, and documented responses parse |
| `tests/openapi_contract.rs` | Requests and responses validate against the vendored OpenAPI spec |
| `tests/http_client.rs` | HTTP behavior against an in-process mock server: headers, errors, retries, deadlines |
| `tests/fake.rs` | `FakeSystemOne` used the way application tests would use it |
| Doctests | Every example in the README and the API docs |

`README.md` exists twice: at the repository root for GitHub, and in `crates/typesafe-client`,
where it becomes the crate's documentation. Edit the crate copy, then copy it to the root. CI
fails if the two differ.

### Live API tests

`tests/live.rs` talks to the real API and is ignored by default.

```sh
# No API key needed: checks error handling and that the vendored OpenAPI spec is current.
cargo test -p typesafe-client --test live -- --ignored rejects_an_invalid_api_key vendored_openapi_spec_matches_live_spec

# Everything, with a key:
TYPESAFE_API_KEY=... cargo test -p typesafe-client --test live -- --ignored
```

The scheduled `API drift` workflow runs the keyless tests weekly. If the repository has a
`TYPESAFE_API_KEY` secret, it runs all of them.

If `vendored_openapi_spec_matches_live_spec` fails, TypeSafe changed their API. Review the
difference, update the wire types if needed, and replace
`crates/typesafe-client/tests/fixtures/openapi.json` with the new
[spec](https://api.typesafe.ai/openapi.json).

## Pull requests

- Add or update tests for behavior changes.
- Update `CHANGELOG.md` under `Unreleased`.
- Keep public items documented; the crate warns on missing docs and CI treats warnings as errors.

## Releasing

Releases are published by the `Release` workflow when a `v*` tag is pushed.

1. Move the `Unreleased` changes in `CHANGELOG.md` under a new version heading, and set
   `version` in `crates/typesafe-client/Cargo.toml` to match.
2. Commit, then tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.
3. The workflow checks that the tag matches the crate version, runs the tests, publishes to
   crates.io and creates a GitHub release with the changelog section as notes.

Publishing uses [crates.io trusted publishing](https://crates.io/docs/trusted-publishing), so
the repository stores no crates.io token. One-time setup:

1. Publish the first version by hand with an API token:
   `cargo publish -p typesafe-client`.
2. On crates.io, open the crate's **Settings → Trusted Publishing** and add a GitHub publisher:
   owner `JedimEmO`, repository `typesafe-client`, workflow `release.yml`, environment `release`.
3. Optionally, add protection rules to the `release` environment in the repository settings,
   such as required reviewers.
