# Verification result — PLAN.md implementation (COMPLETE, 2026-08-29)

Verdict: **PLAN.md is ~90% implemented. All 5 test suites pass and builds are
clean, but the claim "PLAN fully implemented" is overstated.** Four concrete
gaps remain (all are plan-mandated items), plus one test-isolation flake.

## VERIFIED — BUILD + TESTS (all green)

| Check | Result |
|-------|--------|
| `cargo build --workspace` | ✅ |
| `cargo build --workspace --release` | ✅ |
| `cargo test -p common --features auth` | ✅ 57 passed (single-threaded) |
| `cargo test -p barbican --features test-utils` | ✅ 31 + 1 doctest |
| `cargo test -p citadel --lib` | ✅ 77 passed |
| `cargo test -p citadel --bin citadel-job` | ✅ 3 passed |
| `cargo test -p citadel --test integration --features integration-tests --no-run` | ✅ compiles |
| `cargo fmt --all -- --check` | ✅ |
| `cargo machete` | ✅ 0 unused deps |
| `cargo doc --no-deps` | ✅ no unresolved links |
| `cargo clippy --all-features` | ⚠️ 23 warnings |
| `cargo clippy --all-features -- --deny warnings` | ❌ **FAILS** (5 → common) — Phase 5.4 NOT satisfied |

Note on common tests: `tests::auth::config_tests::test_from_env_with_defaults` and
`test_from_env_with_custom_values` FAIL when run multi-threaded due to a test-isolation
race: `test_validate_rejects_localhost_in_production` sets the global `PRODUCTION=true`
env var which leaks into the parallel `from_env` tests. With `--test-threads=1` all 57
pass. This is a pre-existing test-hygiene bug, not a PLAN regression.

## VERIFIED — CODE ITEMS IMPLEMENTED (inspection)

- **P1.1** `apply_versioned_update` helper; all 12 transition handlers use it; dead
  `"modified concurrently"` string-check removed.
- **P1.2/D1** callback: optimistic-locked JobDB write first → Barbican
  `forward_terminal_transition` (publish/delete complete/failure); non-fatal 200; poller
  `reconcile_inbox_transition` self-heal. D2 trigger-failure fallback in all 3 external
  handlers.
- **P1.4** 202 + `{inbox_id,status,message}` for create/delete/publish; publish has no body;
  list_inboxes pagination envelope + `InboxStore::count` + `MongoInboxStore` impl.
- **P1.5/D3** `ensure_inbox_indexes` unique `uniq_warehouse_path` in serve; create_inbox maps
  11000 → 409; mock duplicate test present.
- **P2.1** list_jobs passes FindOptions (sort created_at desc, limit).
- **P2.2** `ensure_citadel_indexes` TTL `ttl_expires_at`; nonce expired-reuse;
  NONCE_TTL_SECS/TIMESTAMP_SKEW_SECS named constants.
- **P2.3 part 1** callback `persist_terminal_job_update` optimistic-locked (`{job_id,
  version}` + `$inc`); idempotent ack on 0 matched.
- **D10** `copy_file_worker` direct write (truncate+write+flush), checksum-mismatch removes
  partial target; no `with_extension`/tmp anywhere.
- **P3.5/D7** common `auth_middleware` returns `AppError` (JSON `{error,message}`) — 401/403/
  503 bodies correct.
- **P3.6** transfer.rs increments `warehouse_path_validation_failed` / `inbox_lock_failed`.
- **P4.1** ProgressTracker wired through transfer.rs + job_main (bail-out poll + progress via
  Citadel `POST /api/v1/job/progress` + job_progress.rs forward helpers).
- **P4.2/D8** TooManyRequests (429) + quota check/rollback in citadel publish/delete;
  JobQueue/JobProcessor deleted.
- **P4.3** barbican metrics.rs (gauge + request counter + `start_metrics_server_with_router`).
- **P4.4** circuit breaker (open/half-open) + jobs_total CounterVec + jobs_duration +
  `barbican_api_errors_total` (reg. name) + `notify_errors_total`.
- **P4.5** kubernetes.rs uses kube_config_path + kube_namespace; test present.
- **P4.7** barbican dev-deps feature-gated; `test-utils` optional feature; rsa/rand out of
  prod graph (confirmed: barbican Cargo.toml has no rsa/rand, common feature-gated + machete
  ignored).
- **P4.8** `Claims::has_role` iterates without cloning.
- **D4(iii)** citadel create returns 201 + `{inbox_id, inbox_path, status:"ready"}`.
- Docs P3.1–P3.7 / D9 reinterpretation: internal-role / JobQueue / tmp-name references
  gone from active docs (only intentional "removed" notes remain; two diagrams in
  `01-design-plan.md` + `common/src/docs/README.md` still show "JWT (internal role)" at line
  47/61 — minor stale diagram stragglers).
- REVIEW.md §5: all 7 priorities marked resolved with mapping (commit ec53890).

## GAPS — NOT IMPLEMENTED (plan-mandated)

1. **P1.1 conflict tests (tests item) FAILED / MISSING.** The plan requires "one conflict
   test per operation file" asserting 409 + `{"error":"conflict"}` when `update_one`
   returns `matched_count: 0`. NO such test exists in `barbican/src/tests/handlers/*`. The
   existing CONFLICT assertions cover different preconditions (already-publishing, not
   deletable). **(Behavior is correct; test coverage missing.)**

2. **P2.3 item 2 NOT implemented.** `cancel_job.rs` still uses two `replace_one({job_id})`
   with no `version` filter / `$inc`; poller `detect_timed_out_jobs` +
   `reconcile_orphaned_jobs` use `update_one({job_id}, ...)` with no version filter / `$inc`.
   Plan explicitly required adding version + `$inc` for consistency with documented
   `JobDB.version` optimistic locking.

3. **P4.6/D9 "no role gate" on the 12 internal routes NOT implemented.** `barbican/src/
   lib.rs` applies ONE auth layer with `required_roles: config.required_roles` (default
   `read,write`) to ALL routes including the 12 internal `*/{start,progress,complete,
   failure}`. A Citadel service token without `read`/`write` → **403**, contradicting D9
   ("public-Bearer, no role gate") and the docs (`authentication.md` says "none — valid
   Bearer JWT only"). The P4.6 router-level test (internal action endpoint succeeds with any
   valid Bearer token) is also missing — `main_tests.rs` only tests `/health`.
   **This is a real functional contradiction between code and docs/plan.**

4. **P3.5 item 3 test requirement MISSING.** `common/src/tests/auth/middleware_tests.rs`
   only tests header extraction / claims; it does NOT assert the 401/403/503 JSON response
   bodies (status + `{error,message}`) that P3.5 required updating to.

5. **Phase 5.4 clippy `--deny warnings` FAILS** — 23 warnings, 5 errors in `common` when
   denied (jwks_cache type_complexity ×2, jwt_validator, db.rs, metrics.rs closures).
   HANDOFF claimed "19 pre-existing warnings" and called clippy optional, but the PLAN's
   step 5.4 explicitly lists `cargo clippy --locked -- --deny warnings` as a gate. Note
   several warnings are in files touched by this work (create_inbox.rs:96, job_callback.rs:
   581, list_jobs.rs:68, integrations/barbican.rs:435/474/682, citadel.rs:167, mock.rs:46),
   so "none from this work" in HANDOFF is not accurate.

## Minor / informational

- P1.5 Mongo-gated duplicate test: plan marked optional ("or extend root README CI notes");
  README notes the index. Acceptable.
- P1.2 callback unit test (trait-mock) is NOT implemented, but the same mock machinery IS
  used by `job_progress.rs` tests (`MockInboxTransitioner`), and forwarding logic is
  exercised there. The callback-specific assertion (publish_complete called with
  files/bytes/path) is not unit-tested.
- Poller reconciliation unit test: not present (plan said "skip gracefully"). Acceptable.
- Two stale diagram labels "JWT (internal role)" remain in `01-design-plan.md:47` and
  `common/src/docs/README.md:61` (historical design docs).
