# Security Considerations: Recursive File Transfer

Threat model, attack vectors, mitigations, hardening guidance, and incident
response for the recursive transfer engine
([`storage/transfer.rs`](../storage/transfer.rs)). Companion documents:
[Recursive Transfer](recursive-transfer.md) (mechanism) and
[Symlink Limitations](symlink-limitations.md) (edge cases).

## Table of Contents

- [Threat Model](#threat-model)
- [Symlink Attack Vectors](#symlink-attack-vectors)
- [Mitigations](#mitigations)
- [Job Callback Replay Protection](#job-callback-replay-protection)
- [Accepted Risks](#accepted-risks)
- [Configuration Hardening Guide](#configuration-hardening-guide)
- [Observability & Audit Logging](#observability--audit-logging)
- [Incident Response](#incident-response)
- [Compliance Requirements](#compliance-requirements)

---

## Threat Model

### Attackers

| Actor | Capability | Motivation |
|-------|-----------|------------|
| **Malicious file submitter** | Writes files/symlinks into *their* inbox before publish; nothing else | Exfiltrate data, DoS the pipeline, corrupt published data, hide malicious payloads |
| **Compromised internal service** | Code execution as a service principal; may read other inboxes or the warehouse | Lateral movement, data theft, sabotage |
| **Root-level adversary** | Full filesystem access (kernel, other tenants on a shared host) | Bypass everything — **out of scope**; controls below are *defense in depth*, not root-proofing |
| **Accidental misconfiguration** | Ops sets `allow_external_symlinks=true`, shrinks limits, points `CITADEL_PROTECTED_BASE_PATH` at a writable location | Data exposure, clobbering |

### Assets

1. **Data confidentiality** — inbox contents and, especially, files *outside* the inbox (the main reason external symlinks are denied by default).
2. **Data integrity** — warehouse contents must exactly match verified inbox contents (checksums, atomic commit).
3. **Availability** — the pipeline must not be exhausted by crafted trees (loops, farms, depth bombs).
4. **Warehouse isolation** — the protected base must never be partially populated or overwritten.
5. **Auditability** — security events must be logged with enough detail for forensics.

---

## Symlink Attack Vectors

### 1. Symlink loops (DoS)

`a → b → a`, or a directory link back to an ancestor. Naive followers loop
forever (stack exhaustion, infinite I/O).

**Mitigation:** active-chain `(dev, ino)` tracking for directory recursion
plus a visited-link set inside chain resolution; both surface
`TransferError::SymlinkLoop` (audit event `SymlinkLoopDetected`). See
[Loop Detection Algorithm](recursive-transfer.md#loop-detection-algorithm).

### 2. External symlinks (data exfiltration / disclosure)

A link in the inbox pointing at `/etc/passwd`, another tenant's inbox, or
credential files. The transfer would copy that content into the warehouse.

**Mitigation:** multi-layer external check (canonical prefix **and**
`st_dev` vs inbox root — the second catches bind-mounted subtrees);
default-deny via `allow_external_symlinks = false`
(`ExternalSymlinkNotAllowed`, audit event `ExternalSymlinkRejected`).
External sources are never deleted even when allowed. See
[Risks of `allow_external_symlinks = true`](symlink-limitations.md#risks-of-allow_external_symlinks--true).

### 3. Symlink farms (resource exhaustion)

Thousands of links to one large inode, or one inode linked from many
directories, multiplying copy work.

**Mitigation:** `max_symlink_references` (per resolved inode, default 10),
`max_total_symlinks` (default 1000), `max_total_files` (default 100 000),
plus the pre-transfer size sum + disk-space check. See
[Symlink Farm Attacks](symlink-limitations.md#symlink-farm-attacks).

### 4. TOCTOU (race conditions)

Swap a validated link for a different target between check and use.

**Mitigation:** non-following check syscalls (`lstat`, `read_link`),
chmod-555 inbox lock before the walk, optional pre-transfer integrity scan
immediately before copy, per-file checksum verification of the on-disk
target, atomic temp-file + rename writes. Residual window is documented as
an **accepted risk** (requires defeating the lock first). See
[TOCTOU](symlink-limitations.md#toctou-vulnerabilities).

### 5. Invalid symlink targets (injection / traversal)

Targets with null bytes, control characters, or `..` components — attempting
to escape the inbox at the string level.

**Mitigation:** `validate_symlink_target` rejects every hop of the chain
(`InvalidSymlinkTarget`); the warehouse path is derived from the
**inbox-relative link position**, never from the target string, so even an
allowed external link cannot redirect *where* data lands. Warehouse paths
are additionally validated to stay inside `CITADEL_PROTECTED_BASE_PATH`
(`InvalidWarehousePath`, audit event `WarehousePathValidationFailed`).

### 6. Depth bombs

Extremely deep directory nesting or long link chains.

**Mitigation:** `max_recursion_depth` (25) for directories,
`max_symlink_depth` (10) for chains; both fail the transfer with specific
errors (`MaxRecursionDepthExceeded`, `MaxSymlinkDepthExceeded`).

### 7. Path-length bombs

Long names engineered to exceed filesystem limits after joining.

**Mitigation:** `max_path_length` (4096 bytes, the Linux `PATH_MAX`
default) checked against the *warehouse target path* for every entry
(`PathTooLong`).

### 8. Partial warehouse exposure

Crash or error mid-transfer leaving half-written data in the protected base
(where downstream consumers read it).

**Mitigation:** atomic transfer pattern — everything is written to a
`{name}.transfer-{ts}-{rand}` sibling directory and only committed by a
single `rename()` after **all** files are verified; any failure deletes the
temp tree (audit event `AtomicRollbackTriggered`, metric
`citadel_transfer_atomic_rollbacks_total`). Non-atomic mode rolls back via
the transfer manifest. Sources are deleted **only after** successful commit.

### 9. Warehouse clobbering

A transfer targeting a path that already holds published data.

**Mitigation:** the atomic commit refuses to rename over a **non-empty**
existing warehouse directory (rejected with an error, existing data
untouched); an existing *empty* directory is removed and replaced.

---

## Mitigations (summary)

| Control | Where |
|---------|-------|
| Default-deny external symlinks | `InboxConfig.allow_external_symlinks = false` |
| Multi-layer external check (canonical prefix + `st_dev`) | collector + integrity scan |
| Loop detection (active chain + visited links) | collector, `resolve_symlink_chain` |
| Per-hop target validation (nulls, control chars, `..`) | `validate_symlink_target` |
| Depth/count/path-length limits | collector, `InboxConfig` |
| Symlink-farm inode reference counting | `count_symlink_reference` |
| Pre-transfer integrity scan (fail fast) | `pre_transfer_integrity_scan` |
| Disk space check with 10% buffer (`statvfs`) | `check_disk_space` |
| Inbox lock (chmod 555, RAII restore) | `InboxLock` |
| Warehouse containment validation | `validate_warehouse_path` |
| Chunked copy + SHA-512 verify of on-disk target + atomic rename | `copy_file_worker` |
| Temp-dir + rename atomic commit / rollback | `transfer_files` |
| Bounded error collection + coordinated cancellation | `TransferContext`, `CancellationToken` |
| Security audit events (every violation, incl. on failure) | `SecurityAuditEvent`, `TransferError::TransferFailed` |
| Metrics without paths (privacy) | `CitadelMetrics` |

## Job Callback Replay Protection

The transfer result reaches Citadel via `citadel-job` → `POST /api/v1/job/callback` (terminal result) and `POST /api/v1/job/progress` (periodic counts). Both routes accept two credentials (D11, [`middleware/job_auth.rs`](../middleware/job_auth.rs)): a Bearer JWT with Citadel `admin` (manual operator action; the JWT signature and `exp` are its replay protection, and the caller's `sub` is logged), or — with **no** `Authorization` header — the worker credentials below. A present `Authorization` header is authoritative: an invalid (`401`) or non-admin (`403`) token is rejected even if a valid `X-Callback-Secret` accompanies it, so a leaked low-privilege JWT cannot be combined with the secret. On the worker path, both routes are protected against replay by the same checks in [`handlers/job_callback.rs`](../handlers/job_callback.rs) (`timestamp` and `nonce` are mandatory there; missing → `401 auth_error`):

| Check | Rule |
|-------|------|
| `X-Callback-Secret` | Constant-time compare against the hot-reloaded secret file (when configured) |
| `timestamp` | `now - timestamp < 300 s` (`NONCE_TTL_SECS`) and `> -60 s` (`TIMESTAMP_SKEW_SECS`) |
| `nonce` | Rejected if a *live* row exists in `callback_nonces`; stored with a 300 s expiry (TTL index garbage-collects) |

**Fresh credentials per send.** `citadel-job` mints a new `timestamp` (`Utc::now()`) and UUID v4 `nonce` for **every** send attempt — each callback attempt (initial + 2 retries) and each progress attempt (up to 3) — via `build_callback_request` / `build_progress_request` in [`job_main.rs`](../job_main.rs). A retry therefore never looks like a replay, and the 300 s window bounds only in-flight delay, not job duration.

> **Fixed defect**: previously the worker captured one timestamp and nonce when it started (`JobConfig::from_env`) and reused them for the completion callback and its retries. Any job running longer than 5 minutes therefore had its callback rejected as `callback_timestamp_expired`, and a retry reused an already-stored nonce. The Kubernetes executor also injected `CITADEL_CALLBACK_TIMESTAMP` / `CITADEL_CALLBACK_NONCE`, which the worker never read. The executor no longer sets these variables ([`jobs/executor/kubernetes.rs`](../jobs/executor/kubernetes.rs)).

**Residual risk:** a captured request cannot be replayed verbatim: while its nonce row is live it is rejected as reused, and the row is stored at receipt (≥ the request timestamp) with a 300 s expiry, so by the time it expires the timestamp is outside the window. An attacker who holds the shared secret can of course mint new requests. Confidentiality of the request (TLS / in-cluster network policy) is still required to protect the shared secret. Any holder of a Citadel `admin` JWT can also report job results; protect that role accordingly.

## Accepted Risks

1. **chmod-based locking does not stop root.** `flock()` (advisory,
   bypassable) and `inotify` (monitoring, not prevention) were explicitly
   rejected; the compensating controls are the atomic pattern, the scan,
   and audit logging. A root-level actor is out of scope.
2. **TOCTOU window after lock defeat** (see vector 4). Worst case is
   disclosure of one swapped file's content into the warehouse; no
   deletion or path escape is possible.
3. **Same-filesystem bind mounts** are not distinguishable from ordinary
   directories by `(dev, ino)`/`st_dev` (see
   [Bind Mount Detection](symlink-limitations.md#bind-mount-detection)).
   Requires mount-table inspection to rule out.
4. **External content when `allow_external_symlinks = true`** is fully in
   scope of the transfer (see
   [risks](symlink-limitations.md#risks-of-allow_external_symlinks--true)).
   Enabling it must be a reviewed configuration change.

---

## Configuration Hardening Guide

Security-critical defaults (already the `InboxConfig::default()` values):

```bash
CITADEL_INBOX_ALLOW_EXTERNAL_SYMLINKS=false
CITADEL_INBOX_MAX_RECURSION_DEPTH=25
CITADEL_INBOX_MAX_SYMLINK_DEPTH=10
CITADEL_INBOX_MAX_TOTAL_FILES=100000
CITADEL_INBOX_MAX_TOTAL_SYMLINKS=1000
CITADEL_INBOX_MAX_SYMLINK_REFERENCES=10
CITADEL_INBOX_MAX_ERRORS_BEFORE_FAIL=1
CITADEL_INBOX_ATOMIC_TRANSFER=true
CITADEL_INBOX_PRE_TRANSFER_INTEGRITY_SCAN=true
CITADEL_INBOX_MAX_PATH_LENGTH=4096
```

**Production recommendations:**

- Keep `ALLOW_EXTERNAL_SYMLINKS=false`. If business needs require `true`,
  scope it per-deployment (different `CITADEL_PROTECTED_BASE_PATH` + job
  user with minimal read ACLs), alert on `ExternalSymlinkRejected`-adjacent
  metrics, and record it in the change log.
- Tighten `MAX_RECURSION_DEPTH` (e.g. 10) and `MAX_TOTAL_FILES` (e.g. 50000)
  to the real maximum published tree; smaller limits fail faster and
  shrink the disk-space pre-check's required headroom.
- Keep `MAX_ERRORS_BEFORE_FAIL=1` unless a batch workload needs leniency;
  raising it means *broken links will be skipped and reported*, which is
  only safe if the reporting channel is monitored.
- Never raise `MAX_SYMLINK_DEPTH`/`MAX_SYMLINK_REFERENCES` above 10 without
  review — they are farm defenses.
- Ensure the job user owns the inbox base and cannot write the protected
  base's *parent* (temp transfer dirs are created as siblings of the
  warehouse path).
- Mount inboxes and the warehouse on filesystems that **support** `st_dev`
  distinctions you rely on (tmpfs bind mounts, overlay layers, etc. change
  `st_dev` semantics — test before deploying).

**Environment-specific settings:**

| Environment | Suggested delta |
|-------------|-----------------|
| Development | `ALLOW_EXTERNAL_SYMLINKS=true`, deeper limits, `PRE_TRANSFER_INTEGRITY_SCAN=false` for speed |
| Staging | production defaults + alerting on all `security_events_total` |
| Production | production defaults; audit log shipped to SIEM; secrets from vault |

---

## Observability & Audit Logging

**Metrics** (privacy: **no paths or targets** in metric names/labels —
paths belong in logs and audit events, not in scrapeable label space):

| Metric | Type | Labels |
|--------|------|--------|
| `citadel_transfer_files_total` | counter | — |
| `citadel_transfer_bytes_total` | counter | — |
| `citadel_transfer_errors_total` | counter | `error_type` (snake_case variant name) |
| `citadel_transfer_duration_seconds` | histogram | — |
| `citadel_transfer_atomic_rollbacks_total` | counter | — |
| `citadel_security_events_total` | counter | `event_type` |

**Audit events** (`SecurityAuditEvent`, severity `Info|Warning|Error|Critical`):
every security-relevant incident records a typed event
(`SymlinkLoopDetected`, `ExternalSymlinkRejected`, `MaxRecursionDepthExceeded`,
`MaxSymlinkDepthExceeded`, `InvalidSymlinkTarget`, `InboxIntegrityCheckFailed`,
`AtomicRollbackTriggered`, `InsufficientDiskSpace`,
`WarehousePathValidationFailed`, `InboxLockFailed`,
`MaxSymlinkReferencesExceeded`, `MaxTotalFilesExceeded`,
`MaxTotalSymlinksExceeded`) with structured `details` (paths, dev/ino,
counts) and optional `inbox_id` / `job_id` context. Events are collected in
`TransferResult.audit_events` on success **and** wrapped into
`TransferError::TransferFailed { errors, audit_events }` on failure, so the
trail survives to `citadel-job` and the callback regardless of outcome.
Example shape:

```json
{
  "timestamp": "2024-01-01T00:00:00Z",
  "event_type": "SymlinkLoopDetected",
  "inbox_id": "665f1c2e9b0a4d3f8a1b2c3d",
  "job_id": "job-123",
  "severity": "Error",
  "details": { "path": "/data/inboxes/.../sub/loop", "dev": 2096, "ino": 36548415 }
}
```

**Logging levels:** `info` transfer start/end + counts; `debug` per-file
copies; `warn` symlink activity, rejections, farm skips; `error` failures,
security violations, rollbacks.

**Retention & integrity:** audit logs must be forwarded to the centralized
SIEM with immutable (write-once or cryptographically signed) storage and
retained ≥ 90 days (see [Compliance](#compliance-requirements)).

---

## Incident Response

### Transfer failure investigation

1. **Read the job's log lines** for `Publish failed:` / `Transfer failed:`
   — `citadel-job` prints every error inside `TransferFailed`, and the
   handler logs the same from the callback.
2. **Classify** by `error_type` (or the specific variant):
   - `symlink_loop` / `max_*` / `invalid_symlink_target` → likely hostile
     or corrupted submitter: quarantine the inbox (it is left untouched on
     failure — do not delete before analysis), notify the submitter's
     admin, check for other inboxes from the same principal.
   - `external_symlink_not_allowed` → potential exfiltration attempt:
     pull the `details.symlink_path` + `resolved_target` from the audit
     event; verify the target was not readable content; alert security.
   - `broken_symlink` → probably benign (submitter cleanup race); confirm
     with the submitter.
   - `insufficient_disk_space` / `file_transfer_failed` → infrastructure:
     check warehouse volume, disk health, temp-dir space (atomic mode needs
     ~2× the payload briefly).
3. **Check the warehouse**: on failure it must contain **no** new data.
   Look for stray `*.transfer-*` directories (rollback failure indicator)
   and `*.tmp.<pid>` files (crash indicator); alert if found.
4. **Verify inbox integrity**: source files must be intact. If they are
   missing on a *failed* transfer, that is a P1 — engage storage owners
   immediately (possible lock defeat or cleanup bug).

### Security violation handling

- Any `ExternalSymlinkRejected`, `SymlinkLoopDetected`, or
  `WarehousePathValidationFailed` at production scale → open a security
  incident, preserve the inbox + audit events, and rotate the callback
  secret if the warehouse path validation failure involved an unexpected
  `CITADEL_PROTECTED_BASE_PATH` value.
- Repeated violations from one submitter → block at the Barbican layer
  (inbox creation), not just transfer rejection.

### Audit log analysis

- Query SIEM by `event_type` and time window; join `inbox_id`/`job_id` to
  the `jobs` collection for the full callback trail.
- Reconstruct loop chains from consecutive `SymlinkLoopDetected` `details`
  (path + dev/ino) entries.
- Correlate `AtomicRollbackTriggered` with the preceding error events to
  confirm rollback completeness.

---

## Compliance Requirements

- **Tamper evidence:** audit logs are append-only at the source and
  forwarded to SIEM immutable storage; `SecurityAuditEvent` structures are
  stable and versioned through the `event_type` enum.
- **Retention:** ≥ 90 days (configurable upward by policy), covering both
  job logs and SIEM copies.
- **Alerting:** page on `citadel_transfer_atomic_rollbacks_total`
  increments, `citadel_security_events_total` for
  `external_symlink_rejected` / `warehouse_path_validation_failed`, and
  sustained `citadel_transfer_errors_total{error_type="symlink_loop"}`.
- **Reporting:** failed transfers with security violations are reported to
  the compliance dashboard via the job callback result
  (`success=false` + logged error list); the audit trail travels with the
  failure (`TransferFailed.audit_events`).
