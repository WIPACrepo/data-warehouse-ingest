//! Job queue and execution system
//!
//! Job types/status live in [`types`]; execution backends live in [`executor`].
//! Queue rate limits are enforced at submission time by the handlers against
//! MongoDB (`count_documents` of in-flight jobs) — see
//! `handlers/{publish_inbox,delete_inbox}.rs`. There is no in-process queue or
//! processor module.

pub mod types;
pub mod executor;
