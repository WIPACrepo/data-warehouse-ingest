use async_trait::async_trait;
use mockall::automock;
use mongodb::{
    bson::Document,
    results::{InsertOneResult, UpdateResult, DeleteResult},
};

use crate::models::InboxDB;

#[automock]
#[async_trait]
pub trait InboxStore: Send + Sync {
    async fn insert_one(
        &self,
        inbox: InboxDB,
    ) -> Result<InsertOneResult, mongodb::error::Error>;

    async fn find_one(
        &self,
        filter: Document,
    ) -> Result<Option<InboxDB>, mongodb::error::Error>;
    
    async fn update_one(
        &self,
        filter: Document,
        update: Document,
    ) -> Result<UpdateResult, mongodb::error::Error>;
    
    async fn delete_one(
        &self,
        filter: Document,
    ) -> Result<DeleteResult, mongodb::error::Error>;
}