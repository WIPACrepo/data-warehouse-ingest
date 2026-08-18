# Inbox State Machine

Documentation of the inbox lifecycle, valid states, and state transitions in Barbican.

## Table of Contents

- [Overview](#overview)
- [Inbox States](#inbox-states)
- [State Transition Diagram](#state-transition-diagram)
- [Transition Rules](#transition-rules)
- [Action-Based Transitions](#action-based-transitions)
- [Optimistic Locking](#optimistic-locking)
- [Error Handling](#error-handling)

---

## Overview

Barbican manages inbox lifecycle through a well-defined state machine. Each inbox progresses through states based on actions performed by external clients or the Citadel backend service.

**Key Principles:**
- All state transitions are **action-based**, not state-based
- Transitions are **validated** before being applied
- Concurrent modifications are prevented with **optimistic locking**
- Invalid transitions return **422 Unprocessable Entity**

---

## Inbox States

```rust
pub enum InboxStatus {
    Active,      // Metadata created, waiting for physical creation
    Creating,    // Physical directory being created by Citadel
    Ready,       // Physical directory ready, accepting file uploads
    Publishing,  // Files being transferred to protected warehouse
    Published,   // Transfer complete, files in protected area
    Failed,      // Operation failed, requires intervention
    Deleting,    // Cleanup operation in progress
    Deleted,     // Cleanup complete,, metadata 100% complete
}
```

### State Descriptions

| State | Description | Can Upload? | Can Publish? | Can Delete? |
|-------|-------------|-------------|--------------|-------------|
| `Active` | Metadata created, Citadel trigger pending | No | No | Yes |
| `Creating` | Citadel creating physical directory | No | No | No |
| `Ready` | Directory created, ready for use | Yes | Yes | Yes |
| `Publishing` | Files being transferred to warehouse | No | No | No |
| `Published` | Transfer complete, data in warehouse | No | No | Yes |
| `Failed` | Operation failed, manual intervention needed | No | No | Yes |
| `Deleting` | Cleanup operation in progress | No | No | No |
| `Deleted` | Cleanup complete, 100% | No | No | No |

---

## State Transition Diagram

```
                    ┌──────────┐
                    │  Active  │
                    └────┬─────┘
                         │ create/start (auto)
                         ▼
                    ┌──────────┐
             ┌──────│ Creating │──────┐
             │      └────┬─────┘      │
  create/    │           │            │   create/
  complete   │           │            │   failure
  (Ready)    │           │            │   (Failed)
             │           ▼            │
             │      ┌──────────┐     │
             │      │  Ready   │     │
             │      └────┬─────┘     │
             │           │            │
             │           │ publish    │
             │           ▼            │
             │      ┌───────────┐    │
             └──────│ Publishing │────┤
                    └─────┬─────┘    │
                          │          │
         publish/         │          │  publish/
         complete         │          │  failure
         (Published)      │          │  (Failed)
                          ▼          │
                    ┌───────────┐    │
                    │ Published │    │
                    └─────┬─────┘    │
                          │          │
                          │ delete   │
                          ▼          │
                    ┌───────────┐    │
             ┌──────│ Deleting  │────┤
             │      └─────┬─────┘    │
  delete/    │            │          │  delete/
  complete   │            │          │  failure
  (Deleted)  │            ▼          │  (Failed)
             │      ┌───────────┐   │
             └─────▶│  Deleted  │   │
                    └───────────┘   │
                          ▲         │
                          │         │
                          └─────────┘
                        (Failed → Deleting allowed)
```

---

## Transition Rules

### External Client Transitions

| Action | From States | To State | Endpoint |
|--------|-------------|----------|----------|
| Create Inbox | (none) | Active | `POST /api/v1/inbox` |
| Delete Inbox | Ready, Failed, Published | Deleting | `DELETE /api/v1/inbox/{id}` |
| Publish Inbox | Ready | Publishing | `POST /api/v1/inbox/{id}/publish` |

### Automatic Barbican Triggers

These transitions happen automatically when external actions are performed:

| Trigger | From State | To State | Description |
|---------|------------|----------|-------------|
| `create/start` | Active | Creating | Barbican triggers Citadel after inbox creation |
| `publish/start` | Publishing | Publishing | Barbican triggers Citadel after publish request |
| `delete/start` | Deleting | Deleting | Barbican triggers Citadel after delete request |

### Citadel Callback Transitions

Citadel calls these endpoints to report progress and completion:

| Action | From State | To State | Endpoint |
|--------|------------|----------|----------|
| `create/progress` | Creating | Creating | Progress update during creation |
| `create/complete` | Creating | Ready | Physical directory created successfully |
| `create/failure` | Creating | Failed | Physical directory creation failed |
| `publish/progress` | Publishing | Publishing | Progress update during transfer |
| `publish/complete` | Publishing | Published | Files transferred successfully |
| `publish/failure` | Publishing | Failed | File transfer failed |
| `delete/progress` | Deleting | Deleting | Progress update during cleanup |
| `delete/complete` | Deleting | Deleted | Cleanup completed successfully |
| `delete/failure` | Deleting | Failed | Cleanup failed |

---

## Action-Based Transitions

### Why Action-Based?

Barbican uses **action-based** transitions rather than direct state updates for several reasons:

1. **Business Logic Validation**: Actions encode business rules (e.g., "can't publish while creating")
2. **Audit Trail**: Each action is logged with context (job ID, progress, errors)
3. **Idempotency**: Actions can be safely retried without side effects
4. **Clear Semantics**: `/create/complete` is clearer than `PUT /status=ready`

### Action Pattern

All actions follow the pattern: `{operation}/{action}`

**Operations:**
- `create` - Physical inbox directory creation
- `publish` - File transfer to protected warehouse
- `delete` - Inbox and file cleanup

**Actions:**
- `start` - Begin operation (automatic trigger)
- `progress` - Report operation progress (periodic update)
- `complete` - Operation finished successfully
- `failure` - Operation finished with error

### Example: Create Flow

```
1. POST /api/v1/inbox
   → State: Active
   
2. (Automatic) POST /api/v1/inbox/{id}/create/start
   → State: Creating
   
3. POST /api/v1/inbox/{id}/create/progress (multiple times)
   → State: Creating (progress updated)
   
4. POST /api/v1/inbox/{id}/create/complete
   → State: Ready
```

---

## Optimistic Locking

### Version Field

All inbox documents include a `version` field for optimistic locking:

```rust
pub struct InboxDB {
    pub _id: ObjectId,
    pub version: u32,  // Optimistic locking
    pub status: InboxStatus,
    // ... other fields
}
```

### Update Pattern

State transitions use MongoDB's conditional update:

```rust
let update = doc! {
    "$set": {
        "status": new_status,
        "version": current_version + 1,
        "updated_at": Utc::now(),
    }
};

let result = collection
    .update_one(
        doc! { "_id": id, "version": current_version },
    update
    )
    .await?;

if result.matched_count == 0 {
    return Err(AppError::Conflict(
        "Inbox was modified concurrently".to_string()
    ));
}
```

### Conflict Resolution

When a conflict occurs (version mismatch):

1. **Return 409 Conflict** to the caller
2. **Caller should retry** the operation
3. **Exponential backoff** recommended for retries
4. **Maximum 3 retries** before failing the operation

---

## Error Handling

### Invalid State Transitions

Attempting an invalid transition returns `422 Unprocessable Entity`:

**Example:** Publishing an inbox that's still being created

```http
POST /api/v1/inbox/{id}/publish
Authorization: Bearer <token>
```

**Response (422):**
```json
{
  "error": "INVALID_STATE_TRANSITION",
  "message": "Cannot publish inbox in state 'creating', expected 'ready'",
  "details": {
    "current_state": "creating",
    "required_state": "ready"
  }
}
```

### Concurrent Modification

When optimistic locking detects a conflict:

**Response (409):**
```json
{
  "error": "OPTIMISTIC_LOCK_FAILURE",
  "message": "Inbox was modified concurrently, please retry",
  "details": {
    "expected_version": 5,
    "current_version": 6
  }
}
```

### Job ID Mismatch

When Citadel reports progress for the wrong job:

**Response (400):**
```json
{
  "error": "JOB_ID_MISMATCH",
  "message": "Job ID does not match current operation",
  "details": {
    "expected_job_id": "abc-123",
    "received_job_id": "xyz-789"
  }
}
```

---

## State Validation Examples

### Valid Transitions

✅ Active → Creating (via `create/start`)  
✅ Creating → Ready (via `create/complete`)  
✅ Creating → Failed (via `create/failure`)  
✅ Ready → Publishing (via `publish`)  
✅ Publishing → Published (via `publish/complete`)  
✅ Publishing → Failed (via `publish/failure`)  
✅ Ready → Deleting (via `delete`)  
✅ Failed → Deleting (via `delete`)  
✅ Deleting → Deleted (via `delete/complete`)  
✅ Deleting → Failed (via `delete/failure`)  

### Invalid Transitions

❌ Active → Ready (must go through Creating)  
❌ Creating → Publishing (must complete creation first)  
❌ Published → Creating (cannot recreate published inbox)  
❌ Deleted → any state (terminal state)  

---

## Monitoring State

### Progress Tracking

During long-running operations, Citadel reports progress:

```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "publishing",
  "progress": {
    "phase": "copying",
    "total_files": 100,
    "processed_files": 45,
    "total_bytes": 1073741824,
    "transferred_bytes": 483183820,
    "updated_at": "2024-01-01T00:05:00Z"
  }
}
```

### Error Tracking

When operations fail, error details are recorded:

```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "failed",
  "error": {
    "code": "TRANSFER_FAILED",
    "message": "Source file not found during verification",
    "job_id": "uuid-string",
    "occurred_at": "2024-01-01T00:05:00Z"
  }
}
```

### Querying by State

Clients can filter inboxes by state:

```http
GET /api/v1/inbox?status=ready
GET /api/v1/inbox?status=publishing
```

---

*See also:*
- [API Reference](api-reference.md) - Endpoint documentation
- [Architecture](architecture.md) - System design overview
- [Citadel Integration](citadel-integration.md) - Backend coordination