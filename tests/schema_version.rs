// SPDX-License-Identifier: MIT OR Apache-2.0
//
// An index written by an older semcode does not hold what a newer one
// extracts. It has to say so, or every later run trusts it and skips the
// files that would have produced the missing rows.
use semcode::{DatabaseManager, SCHEMA_VERSION};

async fn manager(dir: &std::path::Path) -> DatabaseManager {
    let db = DatabaseManager::new(
        dir.join(".semcode.db").to_str().unwrap(),
        dir.to_string_lossy().into_owned(),
    )
    .await
    .unwrap();
    db.create_tables().await.unwrap();
    db
}

#[tokio::test]
async fn a_fresh_index_is_current() {
    let dir = tempfile::tempdir().unwrap();
    let db = manager(dir.path()).await;

    assert_eq!(
        db.stored_schema_version().await.unwrap(),
        Some(SCHEMA_VERSION)
    );
    assert!(!db.index_predates_reader().await.unwrap());
}

#[tokio::test]
async fn an_index_that_predates_versioning_is_not_current() {
    // What an older semcode leaves behind: tables with rows in them and no
    // record of what wrote them. Creating the missing table must not claim
    // the rows are as new as the build that found them.
    let dir = tempfile::tempdir().unwrap();
    let db = manager(dir.path()).await;
    drop(db);

    let path = dir.path().join(".semcode.db").join("schema_meta.lance");
    std::fs::remove_dir_all(&path).expect("schema_meta should exist to be removed");

    let db = manager(dir.path()).await;
    assert_eq!(db.stored_schema_version().await.unwrap(), Some(0));
    assert!(
        db.index_predates_reader().await.unwrap(),
        "an index of unknown vintage was taken for a current one"
    );

    // Once the tree has been read again, it holds what this build writes.
    db.record_schema_version().await.unwrap();
    assert!(!db.index_predates_reader().await.unwrap());
}
