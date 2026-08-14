use crate::db::db_collection;

#[test]
fn test_db_collection_returns_collection() {
    // We can't easily test this without a real client, but we can verify the function exists
    // and has the right signature by referencing it
    let _func = db_collection::<mongodb::bson::Document>;
}
