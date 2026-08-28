// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Storage for file-scope variables of aggregate type: the ops tables a call
// dispatches through without the calling file declaring them.
use anyhow::Result;
use arrow::array::{ArrayRef, Int64Builder, RecordBatch, RecordBatchIterator, StringArray};
use arrow::array::{Int64Array, StringBuilder};
use futures::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;

use crate::database::get_column;
use crate::types::GlobalVariable;

pub struct GlobalStore {
    connection: Connection,
}

impl GlobalStore {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub async fn insert_batch(&self, globals: Vec<GlobalVariable>) -> Result<()> {
        if globals.is_empty() {
            return Ok(());
        }
        let table = self.connection.open_table("globals").execute().await?;

        let mut name = StringBuilder::new();
        let mut type_name = StringBuilder::new();
        let mut file_path = StringBuilder::new();
        let mut git_file_hash = StringBuilder::new();
        let mut line = Int64Builder::new();
        for global in &globals {
            name.append_value(&global.name);
            type_name.append_value(&global.type_name);
            file_path.append_value(&global.file_path);
            git_file_hash.append_value(&global.git_file_hash);
            line.append_value(global.line as i64);
        }

        let batch = RecordBatch::try_from_iter(vec![
            ("name", Arc::new(name.finish()) as ArrayRef),
            ("type_name", Arc::new(type_name.finish()) as ArrayRef),
            ("file_path", Arc::new(file_path.finish()) as ArrayRef),
            (
                "git_file_hash",
                Arc::new(git_file_hash.finish()) as ArrayRef,
            ),
            ("line", Arc::new(line.finish()) as ArrayRef),
        ])?;

        let mut merge_insert = table.merge_insert(&["file_path", "git_file_hash", "name"]);
        merge_insert
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        let schema = batch.schema();
        merge_insert
            .execute(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
            .await?;
        Ok(())
    }

    /// Every declaration of this name, at any revision.
    pub async fn find_by_name(&self, name: &str) -> Result<Vec<GlobalVariable>> {
        let table = self.connection.open_table("globals").execute().await?;
        let escaped = name.replace('\'', "''");
        let batches = table
            .query()
            .only_if(format!("name = '{escaped}'"))
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;

        let mut out = Vec::new();
        for batch in &batches {
            let name = get_column::<StringArray>(batch, "name")?;
            let type_name = get_column::<StringArray>(batch, "type_name")?;
            let file_path = get_column::<StringArray>(batch, "file_path")?;
            let git_file_hash = get_column::<StringArray>(batch, "git_file_hash")?;
            let line = get_column::<Int64Array>(batch, "line")?;
            for row in 0..batch.num_rows() {
                out.push(GlobalVariable {
                    name: name.value(row).to_string(),
                    type_name: type_name.value(row).to_string(),
                    file_path: file_path.value(row).to_string(),
                    git_file_hash: git_file_hash.value(row).to_string(),
                    line: line.value(row) as u32,
                });
            }
        }
        Ok(out)
    }
}
