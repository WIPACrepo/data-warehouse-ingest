# Test Consolidation Plan

Consolidate tests in `barbican` (and eventually other crates) into a `src/tests/` directory structure.

## Goals

1. Move all unit tests from `#[cfg(test)]` modules in source files to `src/tests/`
2. Move integration tests from top-level `tests/` to `src/tests/`
3. Consolidate test utilities into `src/tests/test_utils/`
4. Create a common pattern applicable to all crates in the workspace
5. Maintain `cargo test` as the standard test runner

## Target Directory Structure

```
barbican/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── main.rs
│   ├── db_adapter.rs
│   ├── db_trait.rs
│   ├── handlers/
│   │   ├── mod.rs
│   │   ├── create_inbox.rs
│   │   └── get_inbox.rs
│   └── tests/
│       ├── mod.rs
│       ├── db_adapter_tests.rs
│       ├── db_trait_tests.rs
│       ├── integration_tests.rs
│       ├── handlers/
│       │   ├── mod.rs
│       │   ├── create_inbox_tests.rs
│       │   └── get_inbox_tests.rs
│       └── test_utils/
│           ├── mod.rs
│           └── fixtures.rs
└── tests/
    └── (removed or kept for true e2e tests)
```

## Migration Steps

### Step 1: Create `src/tests/` directory structure

- Create `src/tests/mod.rs` - main test module entry point
- Create `src/tests/handlers/mod.rs` - handler tests submodule
- Create `src/tests/test_utils/mod.rs` - test utilities submodule

### Step 2: Move test utilities

- Move `src/test_utils/fixtures.rs` → `src/tests/test_utils/fixtures.rs`
- Move `src/test_utils/mod.rs` → `src/tests/test_utils/mod.rs`
- Update imports in test files to reference `crate::tests::test_utils`
- Delete empty `src/test_utils/` directory

### Step 3: Move handler tests

- Move `src/handlers/create_inbox_tests.rs` → `src/tests/handlers/create_inbox_tests.rs`
- Move `src/handlers/get_inbox_tests.rs` → `src/tests/handlers/get_inbox_tests.rs`
- Remove `#[cfg(test)] mod create_inbox_tests;` from `src/handlers/mod.rs`
- Remove `#[cfg(test)] mod get_inbox_tests;` from `src/handlers/mod.rs`
- Update imports in test files to use:
  - `crate::tests::test_utils::*`
  - `crate::handlers::{create_inbox, get_inbox}`
  - `crate::db_trait::{InboxStore, MockInboxStore}`

### Step 4: Move integration tests

- Move `tests/integration.rs` → `src/tests/integration_tests.rs`
- Update imports to use public crate exports:
  - `crate::{create_inbox, get_inbox, MockInboxStore, CreateInbox, InboxDB}`

### Step 5: Create test files for source modules

- Create `src/tests/db_adapter_tests.rs` - tests for db_adapter.rs
- Create `src/tests/db_trait_tests.rs` - tests for db_trait.rs

### Step 6: Update `src/lib.rs`

Add test module declaration:

```rust
#[cfg(test)]
pub mod tests;
```

Ensure all types needed for tests are properly exported:
- `pub use db_trait::{InboxStore, MockInboxStore};`
- `pub use handlers::{create_inbox, get_inbox};`
- `pub use {InboxDB, CreateInbox};`

### Step 7: Clean up

- Remove empty `src/test_utils/` directory
- Remove empty `tests/` directory (or keep for future true end-to-end tests)
- Run `cargo test` to verify all tests pass

## Common Pattern for Other Crates

Apply the same structure to `citadel/` and `common/`:

- Each crate gets its own `src/tests/` directory
- Test utilities live in `src/tests/test_utils/`
- Tests mirror the source structure
- All tests run with standard `cargo test`

## Benefits

1. **Centralized test location** - All tests in one place, easier to find and manage
2. **Clear separation** - Source code and test code are visually separated
3. **Shared utilities** - Test utilities accessible to all test files
4. **Scalable pattern** - Works consistently across all crates in workspace
5. **Standard tooling** - No changes to `cargo test` workflow