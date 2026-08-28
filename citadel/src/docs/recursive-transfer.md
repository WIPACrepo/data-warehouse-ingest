# Recursive Transfer with Symlink Handling

This document describes how `citadel-job` transfers an inbox's directory tree
into the protected warehouse: recursive traversal, symlink safety, the
producer/consumer pipeline, the atomic transfer pattern, error collection,
and cleanup semantics.

The implementation lives in [`storage/transfer.rs`](../storage/transfer.rs).
See [Symlink Limitations](symlink-limitations.md) for edge cases and
[Security Considerations](security-considerations.md) for the threat model.

## Table of Contents

- [Overview](#overview)
- [Symlink Handling](#symlink-handling)
- [Loop Detection Algorithm](#loop-detection-algorithm)
- [Symlink Chain Resolution (TOCTOU Mitigation)](#symlink-chain-resolution-toctou-mitigation)
- [External Symlink Validation](#external-symlink-validation)
- [Transfer Pipeline](#transfer-pipeline)
- [Producer-Consumer Diagram](#producer-consumer-diagram)
- [Atomic Transfer Pattern](#atomic-transfer-pattern)
- [Error Collection & Cancellation](#error-collection--cancellation)
- [Checksum Verification](#checksum-verification)
- [Cleanup Behavior](#cleanup-behavior)
- [Resource Limits](#resource-limits)
- [Pre-Transfer Integrity Scan](#pre-transfer-integrity-scan)
- [Configuration Examples](#configuration-examples)
- [Test Examples](#test-examples)

---

## Overview

The transfer engine walks the inbox tree **recursively**, preserving the
directory structure under the warehouse path. For each file at
`inbox/subdir1/subdir2/file.txt`, the target is
`warehouse/subdir1/subdir2/file.txt`.

Public entry points:

```rust
pub async fn transfer_files(
    inbox_path: &Path,
    warehouse_path: &Path,
    config: &CitadelConfig,      // inbox settings via `config.inbox`
    parallelism: usize,
    chunk_size_mb: usize,
    audit_context: AuditContext, // optional inbox_id / job_id for audit events
) -> Result<TransferResult, TransferError>;

pub async fn transfer_files_with_metrics(
    /* same as above */ ...,
    metrics: Option<Arc<CitadelMetrics>>,
) -> Result<TransferResult, TransferError>;
```

High-level flow:

```
START
  │
  ▼
Validate warehouse path            // must stay inside CITADEL_PROTECTED_BASE_PATH
  │
  ▼
Lock inbox (chmod 555)             // RAII InboxLock; original mode restored on drop
  │
  ▼
Resolve inbox root                 // canonical path + (dev, ino) identity
  │
  ▼
Pre-transfer integrity scan?       // optional read-only fail-fast security walk
  │  (sums sizes for disk-space check)
  ▼
Disk space check (10% buffer)      // statvfs on the warehouse filesystem
  │
  ▼
Prepare target base                // atomic: {name}.transfer-{ms}-{rand} temp dir
  │                                 // non-atomic: the warehouse path itself
  ▼
Produce tree walk ──bounded channel──▶ Copy workers (semaphore = parallelism)
 (inline producer, backpressure)        (chunked copy + SHA-512, verify, rename)
  │
  ▼
All copies done, no fatal errors?
  ├─ no  ──► ROLLBACK (delete temp dir / remove manifest files), inbox untouched
  │          return TransferError::TransferFailed { errors, audit_events }
  └─ yes ──► COMMIT: rename temp dir → warehouse (or keep in place)
             delete sources + remove inbox, return TransferResult
```

**Implementation note**: the producer (tree walk) is driven **inline**
in the transfer task via a `tokio::select!` loop rather than a spawned
task, because the `Collector` borrows stack-local state (tracking sets,
audit buffer). The bounded channel still provides backpressure: the
producer blocks when `parallelism * 2` items are buffered.

---

## Symlink Handling

Three cases:

| Entry | Behavior |
|-------|----------|
| Symlink → **file** | Transferred as a **regular file** at the link's path. Content is read *through* the link; the warehouse file is a plain copy (not a link). |
| Symlink → **directory** | **Followed and recursed** into, preserving the link's path in the warehouse (contents appear under both the real directory and the link path, as in the example below). |
| **Loop / cycle** | Detected and reported as `TransferError::SymlinkLoop`; the transfer fails (see [Loop Detection Algorithm](#loop-detection-algorithm)). |

Additional cases:

- **Broken symlink** (target missing): `TransferError::BrokenSymlink` — counts
  toward the error threshold.
- **Invalid target** (null byte, control char, `..` component):
  `TransferError::InvalidSymlinkTarget`.
- **Chain too long**: `TransferError::MaxSymlinkDepthExceeded` when the link
  chain exceeds `max_symlink_depth`.
- **External target**: rejected by default (`ExternalSymlinkNotAllowed`),
  followed when `allow_external_symlinks = true` (the **symlink itself** is
  transferred at its inbox-relative position; the external source file is
  **never deleted** — cleanup is scoped to the inbox).

```
inbox/                 →  warehouse/
├── a/
│   └── data.txt
├── link_a → a         ──►  link_a/data.txt  (copied content, same as a/data.txt)
└── ext  → /elsewhere/f  ──►  ext            (regular file with f's content;
                                            /elsewhere/f survives)
```

---

## Loop Detection Algorithm

Loop detection tracks the **active recursion chain** — the set of directory
identities on the current path from the inbox root to the directory being
entered — using `(st_dev, st_ino)` pairs (Linux-specific, via
`std::os::unix::fs::MetadataExt`).

```
active_chain: HashSet<(dev, ino)>   // directories on the current path

collect(dir):
    id = (dev, ino) of canonical(dir)
    if id in active_chain ──► SymlinkLoop(dir)        # cycle on this path
    active_chain.insert(id)
    for entry in dir: ... collect(subdirs and dir symlinks)
    active_chain.remove(id)
```

Key properties:

- **Insert on entry, remove on exit** — only ancestors are considered, so
  *sharing* a directory via two sibling symlinks is **not** a false loop
  (each sibling recursion re-enters the shared dir with a clean chain).
- **The inbox root is not pre-seeded.** The top-level `collect(inbox_root)`
  inserts it on entry; a symlink that points back at the root (e.g.
  `inbox/sub/root → inbox`) therefore *is* caught — and normal transfers do
  not self-trigger.
- Because the identity is the *resolved* `(dev, ino)`, hard-linked
  directories are impossible on POSIX, and a directory reached through any
  number of symlinks is recognized the moment its inode reappears on the
  chain.

**Two loop classes are caught by two mechanisms:**

1. **Directory loops** (a dir, or symlink-to-dir, reachable from itself
   through the tree): active-chain detection above.
2. **Chain cycles** (e.g. `link_a → link_b → link_a`, where every step is a
   *symlink* with distinct link inodes): caught during
   [`resolve_symlink_chain`](#symlink-chain-resolution-toctou-mitigation)
   with a per-resolution visited-link set, so the error is
   `SymlinkLoop`, not a depth-limit false positive.

---

## Symlink Chain Resolution (TOCTOU Mitigation)

`resolve_symlink_chain(link_path, max_depth)` resolves a symlink manually:

```
current = link_path;  visited = {};  depth = 0
loop:
    depth >= max_depth ──► MaxSymlinkDepthExceeded
    target = read_link(current)         # does NOT follow the link
    validate_symlink_target(target)     # no nulls / control chars / ".."
    resolved = target (absolute) or current.parent() / target
    meta = symlink_metadata(resolved)   # lstat: does NOT follow
    if meta is a symlink:
        id = (dev, ino) of the LINK itself
        id in visited ──► SymlinkLoop           # pure chain cycle
        visited.insert(id); current = resolved; continue
    else:
        canonical = canonicalize(resolved)   # final resolution
        return (canonical, meta, depth)
```

Why manual walking instead of `canonicalize()` alone:

- `read_link()` inspects each hop **without following it**, so the walker
  can apply validation (forbidden characters, traversal) and depth bounds
  at every hop.
- The visited set distinguishes *cycles* (`SymlinkLoop`) from *merely long*
  chains (`MaxSymlinkDepthExceeded`), and bounds the work per link.
- Rust's `symlink_metadata`/`read_link` are the idiomatic non-following
  (`lstat`-based) operations; no raw `O_NOFOLLOW` syscalls are needed.

**TOCTOU note:** the check-then-use window (link replaced between
`symlink_metadata` and the copy) is mitigated by the *ordering* of the
transfer: the inbox is locked (chmod 555) before any walk begins, the
integrity scan re-validates immediately before copying, and the copied
content is checksum-verified against the on-disk target. A swap *during*
copy would at worst produce content that is verified end-to-end, never
partial/unverified data in the warehouse. A hostile actor who can replace
links under a 555-owned directory must already have root — an accepted risk
documented in [Security Considerations](security-considerations.md).

---

## External Symlink Validation

A resolved symlink target is "external" if **either** layer trips
(defense in depth):

1. **Canonical path prefix** — `canonical(target)` does not start with
   `canonical(inbox_root)`.
2. **Device comparison** — the target's `st_dev` differs from the inbox
   root's `st_dev`. This catches **bind-mounted subtrees inside the inbox**
   whose paths canonicalize under the inbox root but whose contents come
   from another filesystem.

Behavior by configuration:

- `allow_external_symlinks = false` (default): external symlinks are
  rejected with `ExternalSymlinkNotAllowed` (audit event
  `ExternalSymlinkRejected`), counted toward the error threshold, and the
  remaining tree is still walked.
- `allow_external_symlinks = true`: external symlinks are followed for
  reading. The warehouse path is computed from the **symlink's position in
  the inbox** (structure preserved), and cleanup **never deletes** the
  external source — only the inbox (and the link entries inside it) is
  removed.

---

## Transfer Pipeline

### Producer

The `Collector` walks the tree asynchronously:

- Checks the cancellation token on entry and between directory entries.
- Enforces `max_recursion_depth` (directory nesting), `max_path_length`
  (warehouse target path, in bytes), `max_total_files`, and
  `max_total_symlinks`.
- For each regular file or file-symlink: increments counters, validates
  path length, and sends `(source, target)` over the bounded channel.
- For each directory (real or symlink-to-dir): runs loop detection and
  **pre-creates the warehouse directory** (`create_dir_all`, idempotent),
  then recurses. Pre-creation means *empty subdirectories exist in the
  warehouse* and workers never race to create parents.
- Errors are funneled through `emit_error` (see below); structural errors
  (loops, depth/count limits) are returned directly, aborting the walk.

### Consumers

Copy workers are spawned per file pair and bounded by a
`Semaphore(parallelism)` held for the duration of the copy:

1. Open the source (following symlinks, as intended).
2. Stream in `chunk_size_mb` chunks to a **temporary file** next to the
   target (`{target}.tmp.{pid}`), computing SHA-512 over the stream.
3. Re-read the temp file from disk and compare checksums
   (verifies the on-disk state, not just the in-memory digest).
4. `rename()` temp → target (atomic on the same filesystem).
5. Report `FileInfo` (source, target, size, checksum).

On failure a worker records the error in the shared `TransferContext`
so the producer observes the cancellation promptly.

### Producer-Consumer Diagram

```
                    bounded mpsc channel (capacity = parallelism × 2)
                    ───────────── backpressure: sender blocks ─────────────
Collector (inline) ──(source, target) pairs──────────────► select! driver
  │  walks tree, creates warehouse dirs,                          │
  │  validates symlinks, enforces limits                          │ dispatches
  │  errors → emit_error ───────────────────────────────────────► │ copy tasks
  │                                                               ▼
  └──────────── Err(error) items (sub-threshold) ─────────► JoinSet
                                                             │ Semaphore(parallelism)
                                                             ▼
                                                 copy → verify → rename
                                                             │
                                                             ▼
                                                   collected results / errors
```

The driver loop (`tokio::select!`) awaits the producer future and channel
items together; when the producer finishes, any buffered items are drained
before in-flight copies are joined.

---

## Atomic Transfer Pattern

With `atomic_transfer = true` (default):

```
1. TEMP      {warehouse}.transfer-{epoch_ms}-{random}   created next to the
            warehouse (same filesystem ⇒ rename is atomic)
2. TRANSFER  all files copied + verified into the temp tree
3. COMMIT    (success only)
                - if the warehouse path exists it must be EMPTY (it is
                  removed) — a non-empty warehouse is rejected, never clobbered
                - rename(temp → warehouse)
4. ROLLBACK  (any failure)
                - remove_dir_all(temp)  ⇒ no partial data ever appears at
                  the warehouse path
                - audit event AtomicRollbackTriggered
                - metric citadel_transfer_atomic_rollbacks_total
```

Because consumers only write inside the temp tree until commit, observers of
the warehouse path see either the old state or the fully new state — never a
half-transfer. In non-atomic mode (`atomic_transfer = false`), files go
straight to the warehouse and rollback on failure removes exactly the files
recorded in the manifest (still no *partial file* exposure: each file is
written to a temp file and renamed into place).

---

## Error Collection & Cancellation

All errors flow through a single counting point, `TransferContext`:

```rust
TransferContext { cancel_token: CancellationToken, error_count: AtomicU32, max_errors }
```

- `record_error()` increments the counter; when the **Nth** error arrives
  (N = `max_errors_before_fail`, minimum 1) the cancellation token fires.
  Semantics are exact: with N = 2, errors 1..1 are collected and processing
  continues; error 2 cancels.
- Collector errors go through `emit_error`, which:
  - returns `None` for **sub-threshold** errors — the error is forwarded on
    the channel (reported in the final result) and the walk **continues**;
  - returns `Some(err)` when this error **reached the threshold** — the
    walk aborts immediately with that specific error.
- Worker (copy) failures also `record_error()` at their origin.
- `Cancelled` items are *mechanism*, never *cause*: they are filtered out of
  the reported error list.

**Success criterion:** `success = producer_ok && !cancelled`. That is:

- A transfer **succeeds** when the tree was fully walked and the error count
  stayed *below* the threshold. Sub-threshold errors (e.g. one broken
  symlink with `max_errors_before_fail = 5`) are skipped, the rest of the
  tree transfers, and they are reported in `TransferResult.errors`.
- A transfer **fails** when the threshold is reached, or when the producer
  aborts structurally (symlink loop, recursion/depth/count limits, closed
  channel) — those always fail even though they may not have reached the
  error count.

Every failure from `transfer_files` is returned as
`TransferError::TransferFailed { errors, audit_events }` — the wrapper
carries the full distinct-error list **and** the security audit trail, so
callers (`citadel-job`, `StorageManager`) can log each error and forward
audit events even when the transfer never completed.

---

## Checksum Verification

Per file, **after** the copy:

1. SHA-512 is computed over the byte stream *during* the copy.
2. The on-disk temp file is then read back and hashed independently.
3. The two digests must match; the temp file is only renamed to its final
   target after this passes.

This verifies the actual on-disk state (detecting write-side corruption)
rather than trusting the in-memory digest. A mismatch removes the temp file
and fails the worker (counted toward the threshold).

---

## Cleanup Behavior

| Outcome | Warehouse | Inbox |
|---------|-----------|-------|
| Success (atomic) | temp dir renamed into place | deleted entirely (`remove_dir_all`) |
| Success (non-atomic) | files committed in place | deleted entirely |
| Failure (atomic) | temp dir deleted — **no partial data** | **untouched** (lock restored on drop) |
| Failure (non-atomic) | manifest files removed — no partial data | **untouched** |

Source deletion is scoped: `cleanup_source` only removes paths *inside* the
canonical inbox (external symlink sources are explicitly skipped), then
removes the inbox directory wholesale. On any failure, sources are never
touched.

---

## Resource Limits

| Config | Default | Exceeded → |
|--------|---------|------------|
| `max_recursion_depth` | 25 | `MaxRecursionDepthExceeded` (structural fail) |
| `max_path_length` | 4096 bytes | `PathTooLong` per entry (threshold-gated) |
| `max_total_files` | 100 000 | `MaxTotalFilesExceeded` (structural fail) |
| `max_total_symlinks` | 1 000 | `MaxTotalSymlinksExceeded` (structural fail) |
| `max_symlink_depth` | 10 | `MaxSymlinkDepthExceeded` per link (threshold-gated) |
| `max_symlink_references` | 10 | `MaxSymlinkReferencesExceeded` per link (threshold-gated) — symlink-farm defense: the same resolved inode referenced through more than N links |
| `max_errors_before_fail` | 1 | — (threshold for the above) |
| disk space (statvfs, +10% buffer) | — | `InsufficientDiskSpace` (pre-transfer, structural) |

---

## Pre-Transfer Integrity Scan

With `pre_transfer_integrity_scan = true` (default), a **read-only** walk of
the whole tree runs before any data is copied:

- resolves every symlink chain (same validation as the collector),
- rejects external symlinks, loops, invalid targets, and count-limit
  violations,
- **fails fast** with `InboxIntegrityFailed` on a broken symlink or any
  violation — nothing is copied, no temp dir is created, the inbox is
  untouched,
- sums file sizes, which feeds the pre-transfer **disk space check**.

It trades a second O(tree) pass for: (a) early failure before allocating the
temp tree, (b) a trustworthy size estimate for the space check, and (c) a
security checkpoint whose timestamp bounds the TOCTOU window.

---

## Configuration Examples

```bash
# Development (permissive)
CITADEL_TRANSFER_PARALLELISM=2
CITADEL_TRANSFER_CHUNK_SIZE_MB=8
CITADEL_INBOX_ALLOW_EXTERNAL_SYMLINKS=true
CITADEL_INBOX_MAX_RECURSION_DEPTH=50
CITADEL_INBOX_MAX_TOTAL_FILES=1000000
CITADEL_INBOX_PRE_TRANSFER_INTEGRITY_SCAN=false   # faster small experiments

# Production (high-security)
CITADEL_INBOX_ALLOW_EXTERNAL_SYMLINKS=false
CITADEL_INBOX_MAX_RECURSION_DEPTH=10
CITADEL_INBOX_MAX_TOTAL_FILES=50000
CITADEL_INBOX_ATOMIC_TRANSFER=true
CITADEL_INBOX_PRE_TRANSFER_INTEGRITY_SCAN=true
```

`InboxConfig` is nested in `CitadelConfig` (`config.inbox`) and loaded from
the `CITADEL_INBOX_*` environment variables in `CitadelConfig::from_env()`.
Limit values of `0` mean "unlimited" (`None`). The executor forwards these
to job containers via `CitadelConfig::inbox_env_vars()`.

---

## Test Examples

The test suite in [`tests/mod.rs`](../tests/mod.rs) (`recursive_transfer_tests`)
covers all of the above. Representative fixtures:

```rust
// External symlink (sibling tempdir)
let external = tempfile::TempDir::new().unwrap();
fs::write(external.path().join("external_file.txt"), "external content").unwrap();
let inbox = fixture.inbox_base.join("inbox");
fs::create_dir_all(&inbox).unwrap();
std::os::unix::fs::symlink(
    external.path().join("external_file.txt"),
    inbox.join("link_to_external"),
).unwrap();
// allow_external_symlinks = false  → ExternalSymlinkNotAllowed
// allow_external_symlinks = true   → succeeds; external source SURVIVES

// Symlink loop (dir link back to ancestor)
fs::create_dir_all(inbox.join("sub")).unwrap();
std::os::unix::fs::symlink(fs::canonicalize(&inbox).unwrap(), inbox.join("sub/loop")).unwrap();
// → SymlinkLoop in TransferFailed.errors; inbox untouched; no temp dirs left

// Chain cycle (a → b → a)
std::os::unix::fs::symlink(inbox.join("link_b"), inbox.join("link_a")).unwrap();
std::os::unix::fs::symlink(inbox.join("link_a"), inbox.join("link_b")).unwrap();
// → SymlinkLoop (not MaxSymlinkDepthExceeded) via the visited-link set
```

Notes when writing transfer tests:

- Create **all** symlinks before calling `transfer_files` — the inbox is
  locked (chmod 555) at start.
- Do not pre-create the warehouse dir for atomic tests (or keep it empty);
  a non-empty existing warehouse path is rejected.
- `Path::exists()` follows symlinks — use `fs::symlink_metadata` to check
  that a *broken* symlink is still present.
- `max_errors_before_fail = N` fails on the Nth error; fewer than N errors
  ⇒ success with `result.errors` populated.
