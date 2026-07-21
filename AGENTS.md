# AGENTS.md

This is a Rust workspace with several related projects and a shared library.

## Project Overview



## Adding or Updating Packages

* Always set the package version in the workspace `Cargo.toml`, then reference it from packages and libraries that need it.

## Testing instructions

* Always run `cargo test` to check that all tests pass.
* Tests require a mongo server running on localhost:27017.
