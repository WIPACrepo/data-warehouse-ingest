# AGENTS.md

This is a Rust workspace with several related projects and a shared library.

## Project Overview

See `README.md` for a project overview.

## Adding or Updating Packages

* Always set the package version in the workspace `Cargo.toml`, then reference it from workspace packages and libraries that need it.

## Testing instructions

* Always run `cargo test` to check that all tests pass.
