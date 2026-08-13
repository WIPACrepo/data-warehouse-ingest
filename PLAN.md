# Test Plan for Barbican Project

This document outlines the plan for creating comprehensive tests for the `barbican` project, including MongoDB mocking and unit tests for `create_inbox` and `get_inbox` handlers.

## Overview

The barbican project is an Axum-based HTTP service that provides inbox management endpoints backed by MongoDB. The current implementation has:
- `POST /api/v1/inbox` - Creates a new inbox
- `GET /api/v1/inbox/{id}` - Retrieves an inbox by ID

## Phase 1: MongoDB Mocking Infrastructure with `mockall`

### 1.1 Define Database Trait

**File**: `barbican/src/db_trait.rs`

Create a trait that abstracts MongoDB operations. `mockall` will auto-generate the mock:

```rust
use mongodb::{bson::{doc, oid::ObjectId, Document}, results::{InsertOneResult, FindOneResult}};
use mockall::automock;
use crate::InboxDB;

#[automock]
#[async_trait::async_trait]
pub trait InboxStore: Send + Sync {
    async fn insert_one(&self, inbox: InboxDB) -> Result<InsertOneResult<ObjectId>, mongodb::error::Error>;
    async fn find_one(&self, filter: Document) -> Result<Option<InboxDB>, mongodb::error::Error>;
}
```

The `#[automock]` attribute generates `MockInboxStore` automatically.

### 1.2 Implement Production Adapter

**File**: `barbican/src/db_adapter.rs`

Implement the trait for the actual MongoDB `Collection<InboxDB>`:

```rust
use mongodb::{Collection, bson::{doc, oid::ObjectId, Document}, results::{InsertOneResult, FindOneResult}};
use async_trait::async_trait;
use crate::{InboxDB, db_trait::InboxStore};

pub struct MongoInboxStore {
    collection: Collection<InboxDB>,
}

impl MongoInboxStore {
    pub fn new(collection: Collection<InboxDB>) -> Self {
        Self { collection }
    }
}

#[async_trait]
impl InboxStore for MongoInboxStore {
    async fn insert_one(&self, inbox: InboxDB) -> Result<InsertOneResult<ObjectId>, mongodb::error::Error> {
        self.collection.insert_one(inbox).await
    }

    async fn find_one(&self, filter: Document) -> Result<Option<InboxDB>, mongodb::error::Error> {
        self.collection.find_one(filter).await
    }
}
```

### 1.3 Update Cargo.toml for Testing

**File**: `barbican/Cargo.toml`

Add test dependencies:
```toml
[dev-dependencies]
async-trait = "0.1"
mockall = "0.13"
```

## Phase 2: Refactor Handlers for Testability

### 2.1 Update Handler Signatures

**File**: `barbican/src/main.rs`

Refactor handlers to use the trait instead of concrete `Collection`:

```rust
async fn create_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Json(input): Json<CreateInbox>,
) -> Result<Json<InsertOneResult<ObjectId>>, (StatusCode, String)>;

async fn get_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Path(id): Path<ObjectId>,
) -> Result<Json<InboxDB>, (StatusCode, String)>;
```

### 2.2 Update App Function

Modify `app()` to accept the store trait:

```rust
fn app(store: Arc<dyn InboxStore>) -> Router {
    // routes using the store
}
```

## Phase 3: Unit Tests for create_inbox

**File**: `barbican/src/handlers/create_inbox_tests.rs`

### 3.1 Success Cases

- **test_create_inbox_success**: 
  - Given a valid `CreateInbox` request
  - When the mock store successfully inserts
  - Then return `200 OK` with the insert result

- **test_create_inbox_with_minimal_data**:
  - Given a `CreateInbox` with minimum required fields
  - When inserted successfully
  - Then return success response

### 3.2 Error Cases

- **test_create_inbox_database_error**:
  - Given a mock store configured to return an insert error
  - When create_inbox is called
  - Then return `500 Internal Server Error` with error message

- **test_create_inbox_invalid_json**:
  - Given malformed JSON in request body
  - When the handler processes it
  - Then return `400 Bad Request`

## Phase 4: Unit Tests for get_inbox

**File**: `barbican/src/handlers/get_inbox_tests.rs`

### 4.1 Success Cases

- **test_get_inbox_success**:
  - Given an inbox exists in the mock store
  - When get_inbox is called with valid ID
  - Then return `200 OK` with the inbox data

- **test_get_inbox_with_all_fields**:
  - Given an inbox with all fields populated
  - When retrieved
  - Then all fields are correctly returned

### 4.2 Error Cases

- **test_get_inbox_not_found**:
  - Given no inbox exists with the provided ID
  - When get_inbox is called
  - Then return `404 Not Found`

- **test_get_inbox_invalid_id**:
  - Given an invalid ObjectId format
  - When get_inbox is called
  - Then return `400 Bad Request`

- **test_get_inbox_database_error**:
  - Given a mock store configured to return a find error
  - When get_inbox is called
  - Then return `500 Internal Server Error`

## Phase 5: Integration Tests

**File**: `barbican/tests/integration.rs`

### 5.1 End-to-End Tests

- **test_full_inbox_lifecycle**:
  - Create an inbox via POST
  - Retrieve it via GET
  - Verify data consistency

- **test_concurrent_operations**:
  - Multiple creates and reads in parallel
  - Verify thread safety

## Phase 6: Test Utilities and Fixtures

**File**: `barbican/src/test_utils/fixtures.rs`

### 6.1 Helper Functions

- `create_test_inbox(name: &str, size: u64) -> InboxDB`
- `create_test_store() -> MockInboxStore`
- `create_test_request(method, path, body) -> Request`

## File Structure

```
barbican/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── db_trait.rs          # New: Database trait with #[automock]
│   ├── db_adapter.rs         # New: MongoDB implementation
│   ├── handlers/
│   │   ├── mod.rs
│   │   ├── create_inbox.rs   # Extracted handler
│   │   ├── get_inbox.rs      # Extracted handler
│   │   ├── create_inbox_tests.rs
│   │   └── get_inbox_tests.rs
│   └── test_utils/
│       ├── mod.rs
│       └── fixtures.rs
└── tests/
    └── integration.rs
```

## Implementation Order

1. **Phase 1** - Create mocking infrastructure (db_trait, mock_store)
2. **Phase 2** - Refactor handlers to use traits
3. **Phase 3** - Write create_inbox unit tests
4. **Phase 4** - Write get_inbox unit tests
5. **Phase 5** - Add integration tests
6. **Phase 6** - Add test utilities and fixtures

## Success Criteria

- All tests pass with `cargo test`
- MongoDB mocking allows tests to run without a database connection
- Both success and error paths are covered for `create_inbox` and `get_inbox`
- Code coverage is >80% for handler logic

## Notes

- The existing test in `main.rs` (`test_with_axum_test`) should be updated to use the `MockInboxStore`
- `mockall` automatically generates `MockInboxStore` from the `#[automock]` trait - no manual mock implementation needed
- Use `mockall::predicate` for matching function arguments in expectations
- All tests should be async and use tokio test runtime