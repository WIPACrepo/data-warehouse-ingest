# Shared Data Model

This document describes the data types in `common/src/models.rs` that both Barbican and Citadel share. Because these types serialize to MongoDB documents (and over the wire), understanding them is essential for anyone working on either service.

## Table of Contents

- [Overview](#overview)
- [InboxDB](#inboxdb)
- [InboxStatus](#inboxstatus)
- [InboxProgress](#inboxprogress)
- [InboxError](#inboxerror)
- [CreateInbox](#createinbox)
- [Serialization Notes](#serialization-notes)
- [See also](#see-also)

---

## Overview

`models.rs` defines the canonical shapes exchanged between the services:

| Type | Purpose | Persisted? |
|------|---------|------------|
| `InboxDB` | The full inbox document stored in MongoDB | Yes |
| `InboxStatus` | The 8-state lifecycle used in state transitions | Yes (embedded) |
| `InboxProgress` | Progress of a long-running operation (create/publish/delete) | Yes (embedded) |
| `InboxError` | Failure information for a failed operation | Yes (embedded) |
| `CreateInbox` | Request payload for creating a new inbox | No (request body) |

`InboxDB` is the single source of truth for what an inbox is. Barbican writes these documents; Citadel reads them from the shared collection and coordinates physical work around them.

---

## InboxDB

The persisted inbox document. Defined in `common/src/models.rs`:

```rust
pub struct InboxDB {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<mongodb::bson::oid::ObjectId>,
    pub name: String,
    pub size: u64,
    pub status: InboxStatus,
    pub warehouse_path: String,
    pub inbox_path: Option<String>,
    pub lifetime: Option<u64>,
    pub archive_enabled: bool,
    pub archive_sites: Vec<String>,
    pub version: u32,
    pub current_job_id: Option<String>,
    pub progress: Option<InboxProgress>,
    pub error: Option<InboxError>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

### Field Semantics

| Field | Type | Serde default | Notes |
|-------|------|---------------|-------|
| `id` | `Option<ObjectId>` | — | Serialized as `_id`. `None` when not yet inserted. |
| `name` | `String` | — | Inbox display name. |
| `size` | `u64` | `0` via reader defaults / builder | Estimated or total size in bytes. |
| `status` | `InboxStatus` | — | Current lifecycle state (see below). |
| `warehouse_path` | `String` | — | Final destination path in the data warehouse. |
| `inbox_path` | `Option<String>` | `None` | Physical staging directory once created by Citadel. |
| `lifetime` | `Option<u64>` | `None` | Expected lifetime / retention hint for the inbox. |
| `archive_enabled` | `bool` | `false` | Whether data should be archived to the LTA. |
| `archive_sites` | `Vec<String>` | `[]` | Sites where archive copies should be placed. |
| `version` | `u32` | `0` | Optimistic-locking counter, incremented on state transitions. |
| `current_job_id` | `Option<String>` | `None` | ID of the in-flight Citadel job for this inbox. |
| `progress` | `Option<InboxProgress>` | `None` | Progress of the current operation. |
| `error` | `Option<InboxError>` | `None` | Error details if the last operation failed. |
| `created_at` / `updated_at` | `chrono::DateTime<Utc>` | — | Timestamps. `updated_at` changes on every write. |

### Ownership & Writing

- **Barbican** owns all writes to `InboxDB` (creates, state transitions, progress/error updates) via the `InboxStore` abstraction.
- **Citadel** reads `InboxDB` metadata from the shared collection and requests transitions **through the Barbican API** — it does not mutate `InboxDB` directly.

### Optimistic Locking

`version` is the concurrency guard. State transitions bump `version`; a transition attempted against a stale version should be rejected (Barbican surfaces this as an `AppError::Conflict` → HTTP `409`). This prevents Barbican and Citadel from racing each other on the same document.

---

## InboxStatus

The shared lifecycle vocabulary:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Active,     // -> "active"
    Creating,   // -> "creating"
    Ready,      // -> "ready"
    Publishing, // -> "publishing"
    Published,  // -> "published"
    Failed,     // -> "failed"
    Deleting,   // -> "deleting"
    Deleted,    // -> "deleted"
}
```

### Lifecycle at a Glance

- `Active` — inbox metadata created; physical staging not yet started.
- `Creating` — Citadel is creating the physical staging directory.
- `Ready` — inbox directory exists and is ready to receive file uploads.
- `Publishing` — Citadel is copying/verifying/deleting files into the warehouse.
- `Published` — publish completed successfully.
- `Failed` — an operation failed; error details in `error`.
- `Deleting` — Citadel is cleaning up the inbox directory.
- `Deleted` — cleanup complete.

> The authoritative transition rules live in [Barbican's state machine](../../barbican/src/docs/state-machine.md). `common` defines the *vocabulary*; Barbican enforces the *rules*.

### Serialization

Because of `#[serde(rename_all = "snake_case")]`, each variant maps to its lowercase, snake_case JSON/BSON string (shown above). The `PartialEq` derive lets services compare statuses directly (e.g., `inbox.status == InboxStatus::Ready`) without string matching.

---

## InboxProgress

Progress reporting for a long-running operation:

```rust
pub struct InboxProgress {
    pub phase: String,
    pub total_files: u64,
    pub processed_files: u64,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub deleted_bytes: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

| Field | Meaning |
|-------|---------|
| `phase` | Current phase name (service-specific, e.g. `"copy"`, `"verify"`, `"delete"`). |
| `total_files` | Total number of files in the operation. |
| `processed_files` | Files processed so far. |
| `total_bytes` | Total bytes to transfer. |
| `transferred_bytes` | Bytes transferred so far. |
| `deleted_bytes` | Bytes deleted so far (publish/delete flows). |
| `updated_at` | When the snapshot was last updated. |

Citadel reports progress; Barbican persists it into `InboxDB.progress`. It is a point-in-time snapshot, designed to be **polled/overwritten** by clients (Barbican's `GET /api/v1/inbox/{id}`) rather than appended.

---

## InboxError

Failure information recorded when an operation fails:

```rust
pub struct InboxError {
    pub code: String,
    pub message: String,
    pub job_id: String,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}
```

| Field | Meaning |
|-------|---------|
| `code` | Machine-readable error code. |
| `message` | Human-readable description. |
| `job_id` | The Citadel job that failed (links to the job record). |
| `occurred_at` | When the failure occurred. |

Set via the Barbican action endpoints (e.g., `/create/failure`) when a Citadel operation fails.

---

## CreateInbox

The request payload for creating a new inbox:

```rust
pub struct CreateInbox {
    pub name: String,
    pub size: u64,
    pub warehouse_path: String,
    pub lifetime: Option<u64>,
    pub archive_enabled: bool,
    pub archive_sites: Vec<String>,
}
```

This is what clients send to `POST /api/v1/inbox`. It mirrors the user-facing fields of `InboxDB`; Barbican's `create_inbox` handler deserializes it and constructs the initial `InboxDB` document (adding `id`, `status: Active`, `version`, timestamps, etc.).

---

## Serialization Notes

- `InboxDB.id` is renamed to `_id` for MongoDB wire compatibility and skipped when `None`.
- Optional fields (`inbox_path`, `lifetime`, `progress`, `error`, `current_job_id`) use `#[serde(default)]`/`Option` so MongoDB documents without them deserialize cleanly.
- `InboxStatus` uses `snake_case` on the wire.
- Timestamps are `chrono::DateTime<chrono::Utc>` and serialize in the standard ISO-8601/RFC-3339 form.

Refer to `common/src/tests/models_tests.rs` for round-trip serialization tests.

---

## See also

- [README.md](README.md) - Main index
- [infrastructure.md](infrastructure.md) - The `InboxStore` trait and `MongoInboxStore` that persist these models
- [barbican/src/docs/state-machine.md](../../barbican/src/docs/state-machine.md) - Authoritative state transitions
- [citadel/src/docs/state-machine.md](../../citadel/src/docs/state-machine.md) - Citadel's view of shared statuses
- [barbican/src/docs/api-reference.md](../../barbican/src/docs/api-reference.md) - Request/response bodies
