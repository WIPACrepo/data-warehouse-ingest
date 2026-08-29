use async_trait::async_trait;
use mongodb::{
    Collection,
    bson::Document,
    options::FindOptions,
    results::{DeleteResult, InsertOneResult, UpdateResult},
};

use crate::{models::InboxDB, traits::InboxStore};

pub struct MongoInboxStore {
    collection: Collection<InboxDB>,
}

impl MongoInboxStore {
    pub fn new(collection: Collection<InboxDB>) -> Self {
        Self { collection }
    }
}

#[async_trait]
impl InboxStore for MongoInboxStore {
    async fn insert_one(&self, inbox: InboxDB) -> Result<InsertOneResult, mongodb::error::Error> {
        self.collection.insert_one(inbox).await
    }

    async fn find_one(&self, filter: Document) -> Result<Option<InboxDB>, mongodb::error::Error> {
        self.collection.find_one(filter).await
    }

    async fn find_many(
        &self,
        filter: Document,
        options: FindOptions,
    ) -> Result<Vec<InboxDB>, mongodb::error::Error> {
        let cursor = self.collection.find(filter).with_options(options).await?;
        use futures_util::TryStreamExt;
        cursor.try_collect().await
    }

    async fn count(&self, filter: Document) -> Result<u64, mongodb::error::Error> {
        self.collection.count_documents(filter).await
    }

    async fn update_one(
        &self,
        filter: Document,
        update: Document,
    ) -> Result<UpdateResult, mongodb::error::Error> {
        self.collection.update_one(filter, update).await
    }

    async fn delete_one(&self, filter: Document) -> Result<DeleteResult, mongodb::error::Error> {
        self.collection.delete_one(filter).await
    }
}
