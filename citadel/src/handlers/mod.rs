//! HTTP handlers for Citadel API

pub mod health;
pub mod ready;
pub mod create_inbox;
pub mod publish_inbox;
pub mod delete_inbox;
pub mod get_job;
pub mod list_jobs;
pub mod cancel_job;
pub mod job_callback;