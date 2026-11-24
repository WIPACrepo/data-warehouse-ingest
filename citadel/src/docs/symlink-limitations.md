# Symlink Limitations & Edge Cases

Known limitations, edge cases, and attack vectors around symlink handling in
the recursive transfer, and how each is mitigated (or consciously accepted).
See [Recursive Transfer](recursive-transfer.md) for the happy-path design and
[Security Considerations](security-considerations.md) for the full threat model.

## Table of Contents

- [Platform Specificity](#platform-specificity)
- [Bind Mount Detection](#bind-mount-detection)
- [Symlink Chain Resolution](#symlink-chain-resolution)
- [TOCTOU Vulnerabilities](#toctou-vulnerabilities)
- [Symlink Farm Attacks](#symlink-farm-attacks)
- [Risks of `allow_external_symlinks = true`](#risks-of-allow_external_symlinks--true)
- [Other Edge Cases](#other-edge-cases)

---

## Platform Specificity

The implementation is **Linux-only by design**:

- Loop detection and symlink-farm counting rely on `(st_dev, st_ino)` pairs
  via `std::os::unix::fs::MetadataExt` (`dev_ino`).
- External detection's device comparison uses `st_dev` (see
  [Bind Mount Detection](#bind-mount-detection)).
- Inbox locking is `chmod 555` (Unix permissions); the non-Unix code path
  degrades to a no-op lock.
- `statvfs` (disk space check) is a Unix call (`nix::sys::statvfs`); on
  non-Unix platforms the check is skipped.

On macOS, `st_ino`/`st_dev` exist but filesystem semantics (e.g.
`st_dev` for bind mounts, `rename` over non-empty targets) differ enough
that behavior is untested and unsupported. On Windows, symlinks, `chmod`,
and the whole ownership model do not apply.

## Bind Mount Detection

A bind mount lets an adversary place a *different* filesystem at a path
*inside* the inbox tree. Path-prefix checks alone are then defeated: the
canonical path still starts with the inbox root, but the contents are not
the inbox's.

Mitigation — **two-layer external check** (both must pass to be "internal"):

1. **Inode/identity layer (primary for loops):** directory recursion
   compares resolved `(dev, ino)` against the active chain, so a bind
   mount *inside* the inbox that loops back to an ancestor is detected.
2. **Canonical path layer (primary for external check):** the resolved
   target must start with the canonical inbox root.
3. **`st_dev` comparison (bind-mount specific):** the resolved target's
   device number must equal the **inbox root's** device number. A bind
   mounted subtree canonicalizes under the inbox path but carries a
   different `st_dev`, so it is classified external and rejected under the
   default `allow_external_symlinks = false`.

Limitation: a bind mount of the *same* filesystem onto itself (same
`st_dev`, different mount point) is indistinguishable from a normal
directory by `(dev, ino)` + `st_dev`. Detecting it would require comparing
`/proc/self/mountinfo` or `statx` mount IDs — considered out of scope for
the controlled-environment threat model (see
[Security Considerations](security-considerations.md)).

## Symlink Chain Resolution

Chains are resolved hop-by-hop with `read_link()` + `symlink_metadata()`
(never a single blind `canonicalize`), which enables per-hop validation and
bounded work. Limits and behaviors:

| Case | Result |
|------|--------|
| Chain longer than `max_symlink_depth` (default 10) | `MaxSymlinkDepthExceeded` |
| A hop's target contains null byte, control char (ASCII 0–31, tab allowed), or a `..` component | `InvalidSymlinkTarget` |
| A hop's target does not exist | `BrokenSymlink` |
| A link re-appears within the same resolution (`a → b → a`) | `SymlinkLoop` (visited-link set) |
| Relative target | resolved against the link's *current* parent at that hop |

A resolved chain's **final** target is canonicalized once more, so the
external check operates on the fully-resolved path.

Note: depth is counted in *hops of this resolution*, independent of
`max_recursion_depth` (which counts directory nesting). A depth-8 chain
inside a depth-24 tree is fine; a depth-12 chain is not, regardless of
nesting.

## TOCTOU Vulnerabilities

Time-of-check-to-time-of-use gaps exist by nature (check metadata, later
open the path). What the design guarantees, and what it does not:

**Mitigated:**

- *Check phase* uses only non-following syscalls (`lstat` via
  `symlink_metadata`, `read_link`) so a swapped-in target cannot redirect
  the *check* itself.
- The inbox is locked (`chmod 555`) before the walk; the optional
  pre-transfer integrity scan re-validates the tree immediately before
  copying, shrinking the window.
- Each copy is checksum-verified against the on-disk target and only then
  renamed into place: a swap *during* copy yields either the pre-swap or
  post-swap full content — never torn data in the warehouse.
- External symlink sources are never deleted, so a swap that redirects a
  link to a file outside the inbox cannot cause deletion of that file
  (cleanup is scoped to the canonical inbox).

**Accepted (documented risk):**

- An actor who can modify the inbox *while* the transfer runs must already
  bypass the chmod lock (i.e. be root/owner with elevated privilege) or
  have write access granted by configuration. Such an actor can point a
  link at arbitrary readable files; with `allow_external_symlinks = false`
  the read is refused *at check time*, but a swap *after* the check could
  expose that file's content in the warehouse (no deletion, no path
  traversal — targets are still validated for `..`/control chars and the
  warehouse path is derived from the *inbox-relative* link position).
- `flock()` and `inotify` were explicitly rejected for locking/monitoring
  (see plan "Design Decisions" #10/#17); the atomic transfer pattern plus
  scan is the compensating control.

## Symlink Farm Attacks

A "farm" is many symlinks pointing at the same inode, used to inflate
copied data or exhaust resources:

- **Per-inode reference limit** (`max_symlink_references`, default 10): the
  resolved target's `(dev, ino)` is counted globally across the whole
  transfer; exceeding the limit yields
  `MaxSymlinkReferencesExceeded` for the extra references (threshold-gated).
- **Total symlink limit** (`max_total_symlinks`, default 1000): bounds the
  number of links the tree may contain at all (structural fail).
- **Total file limit** (`max_total_files`, default 100 000) plus the
  **disk space check** bound the total bytes a farm could force.
- Directory farms (many links to one directory) additionally hit the
  active-chain sharing semantics: each link's recursion re-copies the
  shared directory's contents, so farms amplify I/O — the limits above are
  what keeps that bounded.

## Risks of `allow_external_symlinks = true`

Enabling external symlinks is a **security-critical** configuration change:

1. **Read amplification** — the transfer reads (and copies into the
   warehouse, where Citadel's principals can access it) any file the
   job's user can read, regardless of path.
2. **Information disclosure into the warehouse** — external content lands
   under the *inbox-relative* link path in the protected base; downstream
   consumers (catalog, LTA) see it as ordinary published data.
3. **Data-exfiltration primitive** — a malicious submitter who can create
   links could stage reads of files they are not otherwise entitled to
   publish (bounded by the job user's filesystem ACLs).
4. **External directories are fully recursed** — every file under an
   external directory symlink is copied (and counted against
   `max_total_files`); a link to `/` would copy the entire filesystem if
   limits allowed it.

The external check still applies (target is resolved and validated for
`..`/control characters, depth-bounded), and the *source* is never
deleted. But the *content boundary* of the inbox is gone. **Default is
`false`; keep it `false` unless there is an explicit, reviewed reason.**

## Other Edge Cases

- **Empty subdirectories**: created in the warehouse (producer-side
  `create_dir_all` on directory entry), so structure is preserved even
  with zero files.
- **Non-regular file types** (sockets, FIFOs, devices): skipped with a
  warning; not copied, not errored.
- **File replaced between scan and copy**: copy reads whatever is at the
  path then (link or file); checksum verification is against the copied
  bytes, so consistency of the *unit* (one file) holds.
- **Warehouse path exists**: atomic commit requires it to be *empty*
  (removed then renamed over) or absent; a non-empty existing warehouse is
  rejected with `InvalidWarehousePath`-class error — existing published
  data is never clobbered.
- **Very deep but valid trees**: bounded by `max_recursion_depth`; the
  collector is iterative-recursive (async recursion), so depth also
  interacts with the task's stack — the default of 25 is well inside
  safe bounds.
- **Hard links inside the inbox**: two paths, one inode — both are copied
  as regular files (hard-link identity is not preserved in the warehouse);
  no loop risk (directories cannot be hard-linked on POSIX).
- **Symlink to a file that is also a real file's target** (e.g. `a.txt`
  and `link → a.txt`): each is copied independently; the inode reference
  counter treats them as two references to the same inode.
- **Inode reuse**: `(dev, ino)` tracking is per-transfer; an inode recycled
  *during* the transfer (file deleted + recreated) could in theory collide
  with an earlier identity. This would require modification under the
  chmod lock (accepted risk, see TOCTOU).
