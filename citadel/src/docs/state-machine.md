# State Machine

Documentation of the job lifecycle and the inbox state coordination Citadel participates in.

## Table of Contents

- [Overview](#overview)
- [Job Statuses](#job-statuses)
- [Job Lifecycle Diagram](#job-lifecycle-diagram)
- [Transitions in the Code](#transitions-in-the-code)
- [Job Type Checklist](#job-type-checklist)
- [Inbox State Coordination](#inbox-state-coordination)
- [Failure Handling](#failure-handling)
- [Terminal States](#terminal-states)

---

## Overview

Citadel manages two stateful concerns:

1. **Job lifecycle** (Citadel-owned): publish/delete operations tracked in the `jobs` collection through `queue → running → terminal`.
2. **Inbox state coordination**: Citadel does **not** own inbox state — it reads inbox metadata and requests transitions via the Barbican API. The authoritative state machine lives in [Barbican's docs](../../barbican/src/docs/state-machine.md).

This document focuses on the job lifecycle (fully owned by Citadel) and describes the inbox coordination from Citadel's perspective.

---

## Job Statuses

Defined in [`jobs/types.rs`](../src/jobs/types.rs):

```rust
pub enum JobStatus {
    Queued,     // Job created in MongoDB, not yet submitted
    Running,    // Submitted to executor (K8s job/pod created, or mock)
    Completed,  // Callback received with success=true
    Failed,     // Callback received with failure, or submission failed
    Cancelling, // Cancel requested; in progress
    Cancelled,  // Cancellation finished
    TimedOut,   // Job exceeded max duration
}
```

### Status Descriptions

| Status | Description | Terminal? |
|--------|-------------|-----------|
| `Queued` | Record inserted into `jobs`; not yet handed to an executor | No |
| `Running` | Executor accepted the job (`executor_id` set) | No |
| `Completed` | Callback with `status=completed` and `result.success=true` | Yes |
| `Failed` | Callback failure, failed submission, or rejected completion | Yes |
| `Cancelling` | `POST /job/{id}/cancel`; not yet finished | No |
| `Cancelled` | Cancellation completed | Yes |
| `TimedOut` | Callback with `status=timed_out`, or the job-status poller (`detect_timed_out_jobs`) finding a `queued`/`running` job older than `CITADEL_JOB_MAX_DURATION_SECS` | Yes |

---

## Job Lifecycle Diagram

```
                  POST /inbox/{id}/publish | POST /inbox/{id}/delete
                                |
                                v
                          ┌───────────┐
                          │  Queued   │◄──── insert JobDB (jobs collection)
                          └─────┬─────┘
                                │ executor.submit(&job)
                                v
                          ┌───────────┐
                 ┌────────│  Running  │────────┐
                 │        └─────┬─────┘        │
                 │              │ callback     │
                 │              v              │
                 │        ┌───────────┐        │
                 │        │ Completed │        │
                 │        └───────────┘        │
                 │                             │
                 │        ┌───────────┐        │
                 │        │  Failed   │◄───────┤
                 │        └───────────┘        │
                 │                             │
                 │        ┌───────────┐        │
                 │        │ TimedOut  │◄───────┤
                 │        └───────────┘        │
                 │                             │
                 │        ┌────────────┐       │
                 └───────▶│ Cancelled  │◄──────┘
                          └────────────┘
                               ▲
                               │ cancel POST /job/{id}/cancel
                               │ (admin) — via Cancelling
```

For a visual of the *inbox* state machine (owned by Barbican), see [Barbican State Machine](../../barbican/src/docs/state-machine.md).

---

## Transitions in the Code

### Queued → Running

Where: [`handlers/publish_inbox.rs`](../src/handlers/publish_inbox.rs) and [`handlers/delete_inbox.rs`](../src/handlers/delete_inbox.rs).

1. `JobDB::new` creates the record with `status = Queued`
2. `jobs_collection.insert_one(&job)`
3. Quota check: `count_documents` over `[queued, running, cancelling]` jobs of the same type; at/above `CITADEL_QUEUE_PUBLISH_RATE_LIMIT` (10) / `CITADEL_QUEUE_DELETE_RATE_LIMIT` (20) → roll back the JobDB and return `429 Too Many Requests` (D8)
4. Barbican inbox transition to `publishing` / `deleting`
5. `app_state.executor.submit(&job)` returns an `executor_id`
6. `job.executor.executor_id = Some(executor_id)`; `job.status = Running`
7. `jobs_collection.replace_one(...)`

> **Note**: Queue admission is enforced by a **MongoDB-backed in-flight count** (no semaphore): the `JobQueue`/`JobProcessor` modules were removed (they had no callers). The counter is the source of truth for both submit paths.

### Running → Completed

Where: [`handlers/job_callback.rs`](../src/handlers/job_callback.rs).

- Callback received with `status = "completed"` and `result.success == true`
- `job.status = Completed`
- `job.result = Some(JobResult { success, files_transferred, total_bytes, completed_at })`
- Terminal JobDB write is **optimistic-locked** (`{job_id, version}` filter + `$inc version`) so a delayed/stale callback can't overwrite a newer version (P2.3)
- Then: notify catalog → (if catalog succeeded) notify LTA (`notify_errors_total{target}` + `errors` push on failure)
- Finally: forward the terminal state to Barbican (`publish_complete` / `delete_complete`) — non-fatal, metered (P1.2, D1)

### Running → Failed

- Callback with `status = "failed"`, or `completed` with `result.success == false`
- `job.status = Failed`; a `JobError` is appended for failure callbacks
- Common causes on publish jobs (see [Recursive Transfer](recursive-transfer.md#error-collection--cancellation)):
  - **Threshold failure**: `max_errors_before_fail` errors collected (e.g. a broken symlink with the default threshold of 1) — the cancellation token fires, workers stop, and the run fails
  - **Structural failure**: symlink loop, max recursion/symlink depth, max file/symlink/path-length limits — always fail immediately
  - **Security failure**: external symlink rejected, integrity scan failure, insufficient disk space, warehouse path outside the protected base — fail before or during copy
  - In all failure cases the warehouse is left clean (atomic rollback) and the **inbox is untouched**; sub-threshold errors do *not* fail the job but are reported in the transfer result

### Running → TimedOut

- Callback with `status = "timed_out"` → `job.status = TimedOut`
- The `JobStatusPoller` also detects K8s jobs exceeding `CITADEL_JOB_MAX_DURATION_SECS`, marks them `timed_out` (with the same optimistic-locked terminal write), and re-issues the matching Barbican failure transition so the inbox is never left mid-publish/delete (P1.2 self-healing).

### Running → Cancelling → Cancelled

Where: [`handlers/cancel_job.rs`](../src/handlers/cancel_job.rs).

1. Validate the job isn't in a terminal state (`completed`, `cancelled`, `failed`, `timed_out`) and isn't already `cancelling`
2. `job.status = Cancelling`; persist
3. `app_state.executor.cancel(&job_id)` (K8s delete / mock no-op)
4. `job.status = Cancelled`; persist

---

## Job Type Checklist

Jobs are tagged with a type in `JobType` (from [`jobs/types.rs`](../src/jobs/types.rs)):

| Job Type | Flags | Purpose |
|----------|-------|---------|
| `PublishInbox { inbox_id, notify_catalog, notify_lta }` | publish | Transfer inbox files to protected warehouse; then notify catalog + LTA |
| `DeleteInbox { inbox_id }` | delete | Remove the inbox directory and clean up |

Queue admission is enforced by a **MongoDB-backed in-flight count** (no semaphore) per job type:

| Queue | Limiter | Env Override |
|-------|---------|--------------|
| Publish | in-flight count of `[queued, running, cancelling]` publish jobs | `CITADEL_QUEUE_PUBLISH_RATE_LIMIT` (default 10) |
| Delete | in-flight count of `[queued, running, cancelling]` delete jobs | `CITADEL_QUEUE_DELETE_RATE_LIMIT` (default 20) |

When the count is at/above the limit the handler returns `429 Too Many Requests` and rolls back the just-inserted JobDB (D8).

---

## Inbox State Coordination

Citadel does **not** own inbox state. It coordinates through the Barbican API.

### Where Citadel Transitions Inbox State

Citadel calls `InboxTransitioner` methods ([`integrations/barbican.rs`](../integrations/barbican.rs)), each mapping to `POST /api/v1/inbox/{id}/{create|publish|delete}/{start|progress|complete|failure}` on Barbican. See [Barbican Integration](barbican-integration.md#barbican-api-client) for the full table.

| Caller | `InboxTransitioner` call(s) | Inbox state (Barbican) |
|--------|-----------------------------|------------------------|
| `create_inbox` | `create_start`, then `create_complete` after mkdir | `Active` → `Creating` → `Ready` |
| `publish_inbox` | `publish_start` | `Ready` → `Publishing` |
| `delete_inbox` | `delete_start` | `Ready`/`Failed` → `Deleting` |
| `job_progress` | `publish_progress` / `delete_progress` | unchanged |
| `job_callback` | `publish_complete`/`publish_failure`, `delete_complete`/`delete_failure` | `Published` / `Deleted` / `Failed` |

### Where Citadel Reads Inbox State

- `create_inbox` / `publish_inbox` / `delete_inbox` read inbox metadata directly from the shared MongoDB `inboxes` collection and validate preconditions (e.g., `inbox_path` presence) before acting.

### Shared Inbox Statuses (from `common`)

The `common` crate defines the inbox status set used by both services:

```rust
pub enum InboxStatus {
    Active, Creating, Ready, Publishing, Published, Failed, Deleting, Deleted,
}
```

The authoritative transition rules are documented in [Barbican's State Machine](../../barbican/src/docs/state-machine.md). Citadel's responsibility is to request transitions through the API and honor the responses (including `409` conflicts, which fail the operation immediately).

---

## Failure Handling

### Transfer Failures (within a Running publish job)

The transfer engine distinguishes *failure modes* that map to job outcomes:

| Mode | Trigger | Job outcome | Warehouse state | Inbox state |
|------|---------|-------------|-----------------|-------------|
| Sub-threshold errors | fewer than `max_errors_before_fail` errors (e.g. one broken symlink, threshold 5) | `Completed` (errors reported in result) | fully committed | removed |
| Threshold failure | Nth error collected (cancellation token fires) | `Failed` | rolled back (no partial data) | untouched |
| Structural failure | loop / depth / count / path-length limit | `Failed` | rolled back | untouched |
| Pre-transfer failure | integrity scan, disk space, warehouse validation, lock | `Failed` (nothing copied) | empty | untouched |

**Cancellation mechanism**: a single `CancellationToken` shared by the
producer and all copy workers; any threshold-reaching error cancels it and
all in-flight work exits gracefully. `Cancelled` is never reported as a
failure cause — only the underlying errors are.

### Job Submission Failures

- If `executor.submit()` fails, handlers return the error; the job record may remain at `queued`/`running` (caller/operator intervention or a future poller reconciles).

### Barbican Coordination Failures

- Every `InboxTransitioner` call retries with exponential backoff up to `CITADEL_BARBICAN_API_RETRY_MAX` retries (default 5), backoff doubling from `CITADEL_BARBICAN_API_RETRY_BACKOFF_MS`, capped at 30 s
- A **409 conflict** fails immediately (no retry)
- After `RETRY_MAX` consecutive failed calls the client's circuit breaker opens and calls fail fast with `503 service_unavailable` for 30 s (see [Barbican Integration](barbican-integration.md#circuit-breaker))
- The `jobs` record is inserted before the Barbican `publish_start`/`delete_start` call; if that call fails the handler returns the error and the job row is left at `queued` (the target behavior is to "fail job immediately" — not yet implemented)

### Callback Handling

- Timestamp must be less than 300 s old and no more than 60 s in the future; `citadel-job` mints a fresh timestamp + nonce for every send attempt, so job duration does not matter
- Nonces must be unique; duplicates are rejected as `CallbackNonceReused`
- Callbacks for already-terminal jobs are acknowledged but ignored
- Unknown callback `status` values are rejected

### Retry & Timeout Policy

| Item | Policy | Env |
|------|--------|-----|
| Barbican API | Exponential backoff, max 5 retries, conflict fails immediately; circuit breaker (30 s cooldown) | `CITADEL_BARBICAN_API_RETRY_MAX`, `_RETRY_BACKOFF_MS`, `_TIMEOUT_SECS` |
| Catalog notify | Up to 3 attempts with exponential backoff; catalog-before-LTA ordering | `CITADEL_CATALOG_NOTIFY_RETRY_MAX`, `_RETRY_BACKOFF_MS` |
| LTA notify | Up to 3 attempts with exponential backoff; only after catalog succeeds | `CITADEL_LTA_NOTIFY_RETRY_MAX`, `_RETRY_BACKOFF_MS` |
| Job max duration | Enforced by the job-status poller every `CITADEL_JOB_STATUS_POLL_INTERVAL_SECS` (default 30 s): executor cancel + `TimedOut` (default 24 h) | `CITADEL_JOB_MAX_DURATION_SECS` |

---

## Terminal States

Once a job reaches `Completed`, `Failed`, `Cancelled`, or `TimedOut`, it is terminal:

- Callbacks for terminal jobs are still acknowledged (idempotent) but ignored for state changes
- `POST /job/{id}/cancel` rejects terminal jobs with `409`
- No automatic re-queue of failed jobs: failed jobs require operator intervention to re-submit

> **Planned**: `Failed` and `TimedOut` jobs will be flagged in metrics for alerting (`citadel_jobs_total{status="failed"}` etc.).

---

*See also:*
- [API Reference](api-reference.md) - Endpoint documentation
- [Architecture](architecture.md) - System design
- [Barbican State Machine](../../barbican/src/docs/state-machine.md) - Inbox lifecycle (authoritative)
- [Barbican Integration](barbican-integration.md) - Coordination details
