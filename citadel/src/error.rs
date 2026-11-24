//! Citadel error types — thin alias to [`common::AppError`]
//!
//! All error handling in Citadel uses `common::AppError` directly. This module
//! exists only to keep `crate::error::Result` as a convenience type alias so
//! call sites don't need to change their import path.

pub use common::AppError;

pub type Result<T> = std::result::Result<T, AppError>;
