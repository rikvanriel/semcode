// SPDX-License-Identifier: MIT OR Apache-2.0
pub mod branches;
pub mod calls;
mod connection;
pub mod content;
mod functions;
pub mod processed_files;
mod schema;
pub mod search;
mod symbol_filename;
mod types;
mod vectors;

pub use connection::DatabaseManager;

use anyhow::Result;
use arrow::array::RecordBatch;

/// Look up a column by name and downcast to the expected Arrow array type.
pub(crate) fn get_column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("missing column '{name}' in batch"))?
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| anyhow::anyhow!("column '{name}' has unexpected type"))
}

/// Parse the `calls` column: a JSON array of callee names.
///
/// An unparsable value is an error rather than an empty list. Callers used to
/// discard the parse error, which turned a malformed or unrecognised value into
/// a function that appears to call nothing, indistinguishable from one that
/// really calls nothing.
pub(crate) fn parse_call_list(json: &str) -> Result<Vec<String>> {
    serde_json::from_str::<Vec<String>>(json).map_err(|e| {
        let preview: String = json.chars().take(120).collect();
        anyhow::anyhow!("call list is not a JSON array of names ({e}): {preview}")
    })
}

#[cfg(test)]
mod tests {
    use super::parse_call_list;

    #[test]
    fn parses_a_list_of_names() {
        assert_eq!(
            parse_call_list(r#"["memcpy","kmalloc"]"#).unwrap(),
            vec!["memcpy".to_string(), "kmalloc".to_string()]
        );
        assert!(parse_call_list("[]").unwrap().is_empty());
    }

    #[test]
    fn rejects_values_that_are_not_a_list_of_names() {
        // Truncated, and an object where a list belongs.
        assert!(parse_call_list(r#"["memcpy""#).is_err());
        assert!(parse_call_list(r#"{"calls":["memcpy"]}"#).is_err());

        // A future encoding that carries more than a name. Silently reporting
        // no calls for these is exactly the failure this rejects.
        assert!(parse_call_list(r#"[{"name":"read","kind":"member"}]"#).is_err());
    }

    #[test]
    fn error_names_the_offending_value() {
        let err = parse_call_list("not json").unwrap_err().to_string();
        assert!(err.contains("not json"), "unhelpful error: {err}");
    }
}
