# AGENTS.md

This is a Rust workspace with several related projects and a shared library.

## Project Overview

See `README.md` for a project overview.

## Adding or Updating Packages

* Always set the package version in the workspace `Cargo.toml`, then reference it from workspace packages and libraries that need it.

## Tests

* Each crate gets its own `src/tests/` directory
* Test utilities live in `src/tests/test_utils/`
* Tests mirror the source structure
* For integration tests that require services like MongoDB, they should live in `tests/` and be gated behind the feature flag `integration-tests`. They will be run in CI, but may not run in a dev environment.
* All tests run with standard `cargo test`

## Documentation

Detailed documentation for each crate is written in `src/docs`. Documentation is a first-class output, and should be considered just as valuable as code. Document all design decisions and other important features.