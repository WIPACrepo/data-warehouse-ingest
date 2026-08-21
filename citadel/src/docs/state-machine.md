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
| `TimedOut` | Callback with `status=timed_out` (or poller detection, planned) | Yes |

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
3. Barbican inbox transition to `publishing` / `deleting`
4. `app_state.executor.submit(&job)` returns an `executor_id`
5. `job.executor.executor_id = Some(executor_id)`; `job.status = Running`
6. `jobs_collection.replace_one(...)`

> **Note**: `JobProcessor` (`jobs/processor.rs`) also implements permit acquisition + `executor.submit`; the handlers currently perform submission directly. The `JobQueue` semaphores (`queue_publish_rate_limit` / `queue_delete_rate_limit`) bound concurrency per type.

### Running → Completed

Where: [`handlers/job_callback.rs`](../src/handlers/job_callback.rs).

- Callback received with `status = "completed"` and `result.success == true`
- `job.status = Completed`
- `job.result = Some(JobResult { success, files_transferred, total_bytes, completed_at })`
- **Planned**: notify catalog (must succeed) → notify LTA. Currently `NoOp`/commented in the callback handler.

### Running → Failed

- Callback with `status = "failed"`, or `completed` with `result.success == false`
- `job.status = Failed`; a `JobError` is appended for failure callbacks

### Running → TimedOut

- Callback with `status = "timed_out"` → `job.status = TimedOut`
- **Planned**: background poller detects K8s jobs exceeding `CITADEL_JOB_MAX_DURATION_SECS` (loop currently logs only)

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

The `queue.rs` `JobQueue` gives publish and delete jobs **independent** concurrency limits:

| Queue | Limiter | Env Override |
|-------|---------|--------------|
| Publish | publish semaphore | `CITADEL_QUEUE_PUBLISH_RATE_LIMIT` (default 10) |
| Delete | delete semaphore | `CITADEL_QUEUE_DELETE_RATE_LIMIT` (default 20) |

---

## Inbox State Coordination

Citadel does **not** own inbox state. It coordinates through the Barbican API.

### Where Citadel Transitions Inbox State

| Handler | Barbican call | State string sent |
|---------|---------------|-------------------|
| `create_inbox` | `transition_inbox_state(&id, "active")` | `active` |
| `publish_inbox` | `transition_inbox_state(&id, "publishing")` | `publishing` |
| `delete_inbox` | `transition_inbox_state(&id, "deleting")` | `deleting` |

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

### Job Submission Failures

- If `executor.submit()` fails, handlers return the error; the job record may remain at `queued`/`running` (caller/operator intervention or a future poller reconciles).

### Barbican Coordination Failures

- `barbican_client.transition_inbox_state` retries with exponential backoff up to `CITADEL_BARBICAN_API_RETRY_MAX` (default 5), maximum backoff 30s
- A **409 conflict** fails immediately (no retry)
- The `jobs` record is updated before Ken Barbican transition in publish/delete; if coordination fails the job is left in a partial state (see [PLAN.md](../PLAN.md) Section 6.5 for the target "fail job immediately" behavior)

### Callback Handling

- Timestamp must be within the 5-minute window (1-minute clock skew allowance)
- Nonces must be unique; duplicates are rejected as `CallbackNonceReused`
- Callbacks for already-terminal jobs are acknowledged but ignored
- Unknown callback `status` values are rejected

### Retry & Timeout Policy

| Item | Policy | Env |
|------|--------|-----|
| Barbican API | Exponential backoff, max 5 retries, conflict fails immediately | `CITADEL_BARBICAN_API_RETRY_MAX`, `_RETRY_BACKOFF_MS`, `_TIMEOUT_SECS` |
| Catalog notify | **Planned**: max 3 retries, catalog-before-LTA ordering | `CITADEL_CATALOG_NOTIFY_RETRY_MAX` |
| LTA notify | **Planned**: max 3 retries, only after catalog succeeds | `CITADEL_LTA_NOTIFY_RETRY_MAX` |
| Job max duration | `CITADEL_JOB_MAX_DURATION_SECS` (default 24h); **planned** poller/timeout enforcement | `CITADEL_JOB_MAX_DURATION_SECS` |

---

## Terminal States

Once a job reaches `Completed`, `Failed`, `Cancelled`, or `TimedOut`, it is terminal:

- Callbacks for terminal jobs are still acknowledged (idempotent) but ignored for state changes
- `POST /job/{id}/cancel` rejects terminal jobs with `409`
- No automatic re-queue of failed jobs (see [PLAN.md](../PLAN.md) Section 9.3: "Failed jobs require operator intervention to re-submit")

> **Planned**: `Failed` and `TimedOut` jobs will be flagged in metrics for alerting (`citadel_jobs_total{status="failed"}` etc.).

---

*See also:*
- [API Reference](api-reference.md) - Endpoint documentation
- [Architecture](architecture.md) - System design
- [Barbican State Machine](../../barbican/src/docs/state-machine.md) - Inbox lifecycle (authoritative)
- [Barbican Integration](barbican-integration.md) - Coordination details
- [Design Plan](../PLAN.md) - Complete specification
