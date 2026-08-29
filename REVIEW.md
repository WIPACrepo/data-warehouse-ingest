# Code Review: data-warehouse-ingest

**Reviewer**: senior Rust engineer
**Scope**: `barbican`, `citadel`, `common` workspace crates, with emphasis on
**consistency between the documentation (`src/docs/*.md`, root `README.md`) and
the source code**.

Method: docs read first, then implementation, verifying specific claims
(endpoint paths, status codes, env vars, feature flags, module listings, test
file names) via targeted greps. Files reviewed include all of `common/src`,
`barbican/src`, `citadel/src`, both `tests/` trees, and every doc file.

Findings are ordered by severity within each section.

---

## 1. Critical findings (data integrity / correctness)

### 1.1 Optimistic locking is broken — version conflicts are never detected (Barbican)

Docs (`common/src/docs/models.md`, `barbican/src/docs/state-machine.md`) specify
that a state transition against a stale `version` must fail with
`AppError::Conflict` → HTTP 409:

> a transition attempted against a stale version should be rejected (Barbican
> surfaces this as an `AppError::Conflict` → HTTP `409`).

The implementation claims to do this (e.g. `transition/create.rs`):

```rust
state.store
    .update_one(doc! { "_id": id, "version": inbox.version }, update)
    .await
    .map_err(|e| {
        if e.to_string().contains("modified concurrently") {
            AppError::Conflict("Inbox state was modified concurrently".to_string())
        } else {
            AppError::DatabaseError(e.to_string())
        }
    })?;
```

This is wrong in two ways:

1. `MongoDB.update_one` with a filter including `"version": N` does **not**
   error when the filter matches zero documents. It returns an `UpdateResult`
   with `matched_count == 0`, which the code **silently discards**. A stale
   transition therefore "succeeds", the handler returns the (stale) inbox it
   never wrote — precisely the race the docs claim to prevent.
2. The string check `e.to_string().contains("modified concurrently")` can
   essentially never fire — a version mismatch is not an error and no driver
   error message contains that text. The `Conflict` branch and the doc's own
   `matched_count == 0` pattern (shown in `state-machine.md`'s Optimistic
   Locking section) are effectively dead code.

Every transition handler (`create_*`, `publish_*`, `delete_*`, all 12 in
`barbican/src/handlers/transition/`) has the same defect. **Fix**: inspect
`result.matched_count == 0` after the update and return `AppError::Conflict`,
exactly as the doc code sample shows.

### 1.2 Delete flow never reaches `Deleted`; publish never reaches `Published` (Citadel)

- **Delete**: `citadel/src/handlers/delete_inbox.rs` creates a `DeleteInbox`
  job, calls Barbican `delete_start` (`Ready`/`Failed` → `Deleting`), submits.
  `citadel-job` (`job_main.rs::execute_delete`) only does
  `remove_dir_all(inbox_path)` and **never calls `delete/complete` /
  `delete/failure`** to Barbican; the service-side callback
  (`job_callback.rs`) only updates the `JobDB`. Nothing ever invokes
  `BarbicanClient::delete_complete`/`delete_failure` (grep confirms zero
  callers). Inboxes are permanently stuck in `Deleting`; the documented
  `Deleted` terminal state is unreachable, and the documented automatic
  "inbox will be automatically deleted by the Citadel" never happens.
- **Publish**: likewise, `job_callback.rs` sets the job `Completed` on a
  successful callback but never calls `BarbicanClient::publish_complete`, so
  the inbox stays in `Publishing` forever. `publish_complete` and the
  `PublishCompleteRequest` struct exist but have no callers.
- **Create** is the only flow that transmits completion (via
  `create_inbox.rs` → `create_complete`), and even that is only reachable
  through `POST /api/v1/inbox/{id}/create`, expressible — but the `citadel-job`
  create path does not exist at all (jobs are only for publish/delete).

### 1.3 Duplicate-warehouse-path check is a TOCTOU race (Barbican `create_inbox`)

`create_inbox.rs` does `find_one({warehouse_path})` then `insert_one`. The
README/MVP guarantees "if an existing inbox exists for the specified data
warehouse path, the new request will be rejected." Two concurrent creates for
the same path both pass the check and both insert. There is no unique index
creation anywhere (grep for `create_index` finds nothing) and no uniqueness
handling on insert. The documented guarantee is not upheld.

---

## 2. High: documentation says one thing, code does another

### 2.1 Metrics server bind address — doc wrong

`common/src/docs/infrastructure.md`: "`start_metrics_server()` — binds
`127.0.0.1:3001`…". `common/src/metrics.rs` binds `0.0.0.0:{port}` via
`format!("0.0.0.0:{port}")` and the port is configurable (Barbican 9091,
Citadel 9090), never 3001. Doc is stale on both address and port.

### 2.2 Barbican API reference documents endpoints/status codes the code doesn't implement

- `GET /api/v1/inbox` documented returning paginated envelope
  `{inboxes, total, limit, offset, has_more}` with `limit`/`offset` params; the
  handler returns a bare `Vec<InboxDB>` and the query struct uses `skip`, not
  `offset`.
- `POST /api/v1/inbox` documented → **202 Accepted**, body `{inbox_id, status,
  message}`; handler returns **200 OK** with the raw `InboxDB`.
- `DELETE /api/v1/inbox/{id}` documented → **202 Accepted** with JSON body;
  handler returns **204 No Content**.
- `POST /api/v1/inbox/{id}/publish` documented → 202 + envelope; handler
  requires a JSON body (currently empty `{}` type) and returns 200 OK +
  `InboxDB`. The API reference says **no request body** (§3.3 below).
- The "Create Failure" section is malformed/truncated mid-stream
  (`## Create Failure ... Request---` around line 265) — broken markdown.
- Error-response examples (`INVALID_TOKEN`, `INSUFFICIENT_ROLE`,
  `OPTIMISTIC_LOCK_FAILURE`, `INVALID_STATE_TRANSITION`) are all-caps codes,
  while the actual `AppError` serializes lowercase `snake_case` codes
  (`invalid_token`, `inbox_not_found`, ...) and includes `"error"` + `"message"`
  (no `"details"`). The doc shows a `details` object that the code never emits.

### 2.3 Citadel integration docs describe a different trigger contract

`barbican/src/clients/citadel.rs` posts to `.../api/v1/inbox/{id}/create`,
`/delete`, `/publish`. That matches Citadel's routes (`main.rs`), so the **code
is internally consistent**. But `barbican/src/docs/citadel-integration.md`
(§Automatic Triggers, §Service-to-Service Communication) documents the trigger
as `POST .../create/start` with a `{"job_id": ...}` body — i.e. the Barbican
internal-callback URL, not the Citadel trigger. The documented contract
diverges from the code.

### 2.4 Separate/duplicated auth modules in docs

- `barbican/src/docs/README.md` and `architecture.md` show a local
  `barbican/src/auth/` (`claims.rs`, `jwt_validator.rs`, `middleware.rs`).
  There is no `barbican/src/auth/` — auth lives in `common/src/auth/` and is
  re-exported. Docs are stale.
- `barbican/src/docs/README.md` lists `middleware/auth.rs` and
  `middleware/logging.rs`; `barbican/src/middleware/` contains only `mod.rs`.
- `barbican/src/docs/README.md` lists `test_utils/{fixtures.rs, mock_store.rs}`;
  actual: a single `test_utils/fixtures.rs`.
- `common/src/docs/overview.md` and `testing.md` list `config_tests_tests.rs`
  under `common/src/tests/`; that file does not exist.
- `citadel/src/docs/README.md` module tree lists `storage/lock.rs` (doesn't
  exist; locking is in `transfer.rs`) and `auth/{claims,mod,jwt_validator}.rs`
  (auth is in `common`).

### 2.5 README "Known issue" is fixed/stale

README §"Known issue" says
`common::tests::auth::config_tests::test_from_env_with_custom_values` fails
because it asserts `allowed_algorithms.contains("RS384")` while the parser
yields only RS256. The actual test now sets `ALLOWED_ALGORITHMS=RS256,RS384`
and the parser splits on commas, so it passes. Remove the warning (or re-check).

### 2.6 Metric contracts documented but not implemented / mismatched labels

- `barbican/src/docs/configuration.md` shows metrics `barbican_inboxes_total{status}`,
  `barbican_api_requests_total{endpoint,method,status}` / `axios` — none of
  these are defined anywhere. The Barbican `/metrics` endpoint serves only the
  default `axum-prometheus` recorder (`metrics_app()`).
- `citadel/src/docs/configuration.md` "Security Monitoring" and
  `api-reference.md` document `error_type` labels like
  `warehouse_path_validation_failed`, `external_symlink_rejected`; the code
  increments `transfer_errors_total` with labels such as
  `external_symlink_not_allowed`, `max_symlink_references_exceeded`,
  `inbox_integrity_failed`. The documented label vocabulary does not match the
  code, and no metric is incremented for several `SecurityEventType`s
  (e.g. warehouse-path validation failure) despite the documented alert list.
- Several registered Citadel counters are never incremented anywhere:
  `jobs_total`, `jobs_duration`, `barbican_api_errors_total`,
  `circuit_breaker_state` (grep confirms). Dead metrics vs documented
  observability surface.

### 2.7 Citadel `api-reference.md` details vs code

- `POST /api/v1/inbox/{id}/create` documented response `201` with
  `"status": "active"`; handler returns `"status": "ready"`.
- Callback section documents `X-Callback-Secret` behavior and TTL index —
  secret behavior matches; the TTL index/nonce expiry does not (see §3.3).

---

## 3. High: latent bugs and code smells

### 3.1 `list_jobs`: sort/limit options built then discarded (Citadel)

```rust
let _options = mongodb::options::FindOptions::builder()
    .limit(params.limit as i64)
    .sort(doc! { "created_at": -1 })
    .build();
let mut cursor = jobs_coll.find(filter).await ...;
```

`_options` is never passed to `find`. `limit` is parsed and honored, sorting is
not. API doc says "Jobs are sorted by created_at descending" — not true.

### 3.2 Barbican `publish_inbox` requires a body the docs say isn't needed

`Json(_input): Json<PublishRequest>` with `PublishRequest {}` means a
body-less `POST /api/v1/inbox/{id}/publish` returns 400. The API reference
documents no request body. Either `Option<Json<...>>` or document the `{}`.

### 3.3 Callback nonce TTL not enforced (Citadel)

`CallbackNonceDB.expires_at` is written but never read
(`validate_and_store_nonce` only checks existence). `authentication.md` says
"stored with a 5-minute expiry" and documents a TTL index on
`callback_nonces.expires_at`; no TTL index is created anywhere and no expiry
check exists. Consequences: (a) nonces never expire — a captured valid callback
cannot be replayed *after the timestamp window*, but the nonce table grows
without bound; (b) the documented replay window is not implemented.

### 3.4 Callback overwrite race + no state callback (Citadel)

- `job_callback` uses `replace_one({job_id})` with no version filter, so a
  delayed/stale callback can overwrite a newer job state despite `JobDB.version`
  existing — the optimistic-locking concept documented for jobs isn't applied.
- On `completed`+success the inbox is never transitioned (see §1.2).
- Catalog/LTA notify: matches doc ("LTA only after catalog"), but errors are
  swallowed with `.ok()` — doc claims "catalog notification must succeed first",
  which the swallow effectively defeats (the failure is logged only).

### 3.5 Transfer temp-file naming can collide for aliased inputs

`copy_file_worker` names the temp file
`target.with_extension(format!("tmp.{}", std::process::id()))` for every
in-flight copy in the same process. If two entries in the tree resolve to the
same target (hardlinks, or a file+file-symlink aliasing the same target path),
two concurrent copies open the same temp name and can corrupt each other.
In practice the manifest dedupes by path string, so equality requires identical
relative path strings; still, the same inode via two filenames is not deduped,
and the counterpart creates a temp with `with_extension` (replacing an existing
extension). Consider a per-file unique temp name (uuid suffix).

### 3.6 `parse_bool` accepts `1/0/yes/no/on/off` while docs say booleans reject
non-`true/false`

`citadel/src/docs/configuration.md` claims: "booleans reject
non-`true/false` strings". `parse_bool` accepts `"1","0","yes","no","on","off"`
too. Doc is wrong (code is more permissive).

### 3.7 Feature-flag / dependency drift

- README: "`common` — `integration-tests` (enables integration test suite)".
  In `common/Cargo.toml`, `integration-tests = []` enables nothing — the
  integration tests use `#![cfg(feature = "integration-tests")]`, so the flag
  itself is a bare marker. That's a real (if minor) doc/code nuance; the README
  phrasing implies it enables a suite.
- `barbican/Cargo.toml` depends on `common` with `auth` + `test-utils`; README
  says the same. OK. But `test-utils` pulls `rsa` + `rand` + `auth` into the
  Barbican *runtime* build, and `mockall` is a runtime (not dev-) dependency of
  Barbican and `common` — `MockInboxStore` is only re-exported under
  `#[cfg(any(test, feature = "test-utils"))]` in `common`, so it is not a
  runtime cost for common, but in Barbican `common` is compiled with
  `test-utils` always (Barbican has no `test-utils` feature gate), shipping
  mock/RSA/rand code into the production binary. Consider a feature-gated
  `barbican` test-utils.
- `common/Cargo.toml` `test-utils = ["auth", "dep:rsa", "dep:rand"]` matches
  README ("enables `auth` plus RSA key generation"). OK.
- `citadel/Cargo.toml` — `integration-tests = []` same bare-marker pattern.

### 3.8 Misc smaller items

- `config.rs` (Citadel): `kube_config_path` is read but **never used** by
  `KubernetesExecutor` (`kube::Client::try_default` ignores it) and
  `kube_namespace` is also unused there (hardcoded `"citadel-jobs"`)
  — config docs document both as effective settings. Similarly for Barbican:
  `internal_roles`, `mongodb_timeout_secs`, `mongodb_retry_max`,
  `inbox_base_path`, `keycloak_client_secret`/`token_url` are parsed but never
  used by any code path (grep) — several are documented as active protections
  (e.g. path validation via `inbox_base_path` is documented in
  `configuration.md` §Path Validation but no such validation exists in
  Barbican's handlers).
- `progress.rs` (`ProgressTracker`) and `jobs/queue.rs` / `jobs/processor.rs`
  are dead code — nothing calls them (handlers insert/submit directly).
  Docs present the "job processor with per-type rate-limited semaphores" as the
  active mechanism (`README.md` module tree). The queue rate limits
  (`CITADEL_QUEUE_*_RATE_LIMIT`) are therefore not actually enforced anywhere.
- `auth` claims: `common::Claims.has_role` does `contains(&role.to_string())` —
  allocating per call; minor.
- `README.md` says workspace `resolver = "3"` and edition 2024 — matches.
- `common/src/db.rs`: `db_setup` signature is `(uri, name)`; several docs say
  it "reads DATABASE_URL/DATABASE_NAME env vars" (`infrastructure.md`) — stale,
  though `db_tests.rs` reads env vars itself. Barbican calls `db_setup_with_shutdown`
  with config values. The doc description is inconsistent with the API.
- Citadel `handlers/ready.rs` returns 503 status + body for the failure case —
  matches the documented 503 example. Good.
- `create_inbox.rs` (Citadel) — described as fetching via the Barbican API in
  the README workflow, but the handler reads the shared MongoDB collection
  directly and calls Barbican only for the state transition; the docs'
  "fetch inbox metadata via the Barbican API" is not what the code does
  (multiple `README`/`api-reference` "Behavior" steps).

---

## 4. Verification notes (what actually matches the docs)

- `InboxDB`, `InboxStatus` (8-state, snake_case), `InboxProgress`, `InboxError`,
  `CreateInbox` shapes match `common/src/docs/models.md` exactly.
- `InboxStore` trait and `MongoInboxStore` match `infrastructure.md`; the mock is
  wired correctly.
- `AppError` HTTP status mappings in `error.rs` match the status table in
  `infrastructure.md` (404/409/409/400/422/400/409/422; also Job/Storage/etc.
  variants). The response body is `{error, message}` — doc's "details" field
  is the mismatch noted in §2.2.
- Barbican router routes match `lib.rs` listing, and the internal action
  endpoints (`create|publish|delete/{start,progress,complete,failure}`) match
  the API reference's path list.
- Auth middleware behavior (Bearer check, role gate, 401/403/503) matches
  `authentication.md` closely.
- Citadel route table in `api-reference.md` matches `main.rs` routes exactly.
- `publish_inbox`/`delete_inbox` (Citadel) insert job, transition via Barbican,
  submit to executor, respond 202 — matches docs except the terminal-state
  gap (§1.2) and the discarded options (§3.1).
- Callback secret constant-time comparison and absent-secret fallback match the
  callback security docs.
- Integration tests in `citadel/tests/integration.rs` and `common/tests/db_tests.rs`
  are `integration-tests`-gated as documented.

---

## 5. Recommended priority order

> **Resolution status (2026-08-29):** all 7 priorities addressed. Mapping:
> **1** → implemented in `PLAN.md` P1.1 (optimistic-locking helper
> `apply_versioned_update`; zero matched docs → `409 Conflict`);
> **2** → P1.2 + D1 (callback handler forwards terminal transitions via
> `BarbicanClient`, poller self-heals; `Published`/`Deleted` reachable);
> **3** → P1.5 + D3 (unique `uniq_warehouse_path` index at startup, duplicate →
> 409); **4** → P2.1 + P2.2 (`FindOptions` wired, nonce TTL index + expired-
> reuse); **5** → Phase 3 + P1.4 (all endpoint/status-code/module-tree/metrics
> docs aligned, README known-issue removed); **6** → P4.7 (Barbican dev-only
> deps feature-gated; `rsa`/`rand` out of prod graph); **7** → P4.1/P4.2/P4.5/
> P3.4 (`ProgressTracker` wired, `JobQueue`/`JobProcessor` deleted, unused
> config fields documented/removed).

1. **§1.1** Fix optimistic locking (`matched_count == 0` → `Conflict`). This is
   the documented concurrency contract and it silently does nothing.
2. **§1.2** Wire `publish_complete` / `delete_complete` / `delete_failure`
   (and `publish_failure`) from the callback handler and `citadel-job` so
   `Published`/`Deleted` are reachable; delete the stale "auto-delete" doc
   claims if intentionally deferred.
3. **§1.3** Unique index on `warehouse_path` (or accept-and-document eventual
   consistency).
4. **§3.1 / §3.3** Pass `FindOptions` to `find`; add the nonce TTL index + expiry
   check (or remove `expires_at` and the doc claims).
5. **§2** Bring the endpoint/status-code/module/metrics docs in line with the
   code (or the code with the docs) across the `api-reference.md` files,
   module trees, and the README known-issue note.
6. **§3.7** Feature-gate Barbican's dev-only deps so production builds don't
   carry `mockall`/`rsa`/`rand`.
7. **§3.8** Either wire up or delete the dead machinery (`ProgressTracker`,
   `JobProcessor`/`JobQueue`, unused config fields) and correct the docs that
   present them as active.

---

*Review date: 2026-08-27. Workspace state matches `Cargo.toml` (`resolver = "3"`,
edition 2024, Rust 1.85+).*
