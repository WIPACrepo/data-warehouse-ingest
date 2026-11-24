use async_trait::async_trait;
use mongodb::{
    bson::Document,
    options::FindOptions,
    results::{DeleteResult, InsertOneResult, UpdateResult},
};

use crate::models::InboxDB;

/// Storage abstraction for inbox documents.
///
/// `MockInboxStore` is generated only for tests and the `test-utils` feature,
/// so `mockall` stays out of production dependency graphs (REVIEW #8).
#[cfg_attr(any(test, feature = "test-utils"), mockall::automock)]
#[async_trait]
pub trait InboxStore: Send + Sync {
    /// Liveness check of the backing database (used by readiness probes).
    async fn ping(&self) -> Result<(), mongodb::error::Error>;

    async fn insert_one(&self, inbox: InboxDB) -> Result<InsertOneResult, mongodb::error::Error>;

    async fn find_one(&self, filter: Document) -> Result<Option<InboxDB>, mongodb::error::Error>;

    async fn find_many(
        &self,
        filter: Document,
        options: FindOptions,
    ) -> Result<Vec<InboxDB>, mongodb::error::Error>;

    async fn count(&self, filter: Document) -> Result<u64, mongodb::error::Error>;

    async fn update_one(
        &self,
        filter: Document,
        update: Document,
    ) -> Result<UpdateResult, mongodb::error::Error>;

    async fn delete_one(&self, filter: Document) -> Result<DeleteResult, mongodb::error::Error>;
}
