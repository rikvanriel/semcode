// SPDX-License-Identifier: MIT OR Apache-2.0
//! Database utilities for path processing and connection management

use std::path::Path;

/// Process database path argument according to semcode's database location rules
///
/// This function implements the standard semcode database path resolution logic:
/// 1. If `database_arg` is provided:
///    - If it's a directory, look for `.semcode.db` within it
///    - Otherwise, use the path as-is (direct database path)
/// 2. If `database_arg` is None, check the `SEMCODE_DB` environment variable
///    (same directory/suffix semantics as the `-d` flag)
/// 3. If neither is set:
///    - For indexing operations: prefer `source_dir/.semcode.db`, fallback to current directory
///    - For query operations: use current directory `./.semcode.db`
///
/// # Arguments
/// * `database_arg` - Optional database path from command line (-d flag)
/// * `source_dir` - Optional source directory for indexing operations
///
/// # Returns
/// String representation of the database path to use
pub fn process_database_path(database_arg: Option<&str>, source_dir: Option<&Path>) -> String {
    match database_arg {
        Some(path) => resolve_path(path),
        None => {
            // Check SEMCODE_DB environment variable before falling back to
            // source-dir or current-dir defaults.
            if let Ok(env_path) = std::env::var("SEMCODE_DB") {
                let env_path = env_path.trim();
                if !env_path.is_empty() {
                    return resolve_path(env_path);
                }
            }

            match source_dir {
                Some(source_path) => {
                    // For indexing operations: prefer source directory unless it's current directory
                    let source_semcode_db = source_path.join(".semcode.db");
                    if source_path != Path::new(".") {
                        source_semcode_db.to_string_lossy().to_string()
                    } else {
                        // Source is current directory, use current directory
                        "./.semcode.db".to_string()
                    }
                }
                None => {
                    // For query operations: use current directory
                    "./.semcode.db".to_string()
                }
            }
        }
    }
}

/// Normalize a database path: append `.semcode.db` to directories, pass
/// paths that already end with `.semcode.db` through unchanged, and
/// return anything else as-is.
fn resolve_path(path: &str) -> String {
    let path_obj = Path::new(path);

    if path.ends_with(".semcode.db") || is_database(path_obj) {
        path.to_string()
    } else {
        path_obj.join(".semcode.db").to_string_lossy().to_string()
    }
}

/// Whether a path is itself a database rather than a directory holding one.
///
/// Asked of what is on disk, so that one argument names one database whatever
/// order the commands run in. Deciding on whether the directory merely exists
/// splits it in two: `-d dir` writes the database at `dir` when nothing is
/// there yet, and every later command reads `dir/.semcode.db`, which is
/// empty, so a freshly written index answers every query with "not found".
fn is_database(path: &Path) -> bool {
    path.join("functions.lance").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    /// Serializes tests that read or write the SEMCODE_DB environment variable.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_process_database_path_with_explicit_path() {
        // A path naming the database itself is used as given.
        let result = process_database_path(Some("/path/to/.semcode.db"), None);
        assert_eq!(result, "/path/to/.semcode.db");
    }

    #[test]
    fn test_process_database_path_with_directory() {
        // Anything else names a directory to hold one, whether or not it is
        // there yet: the indexer creates it, and a later query must be told
        // the same place.
        let result = process_database_path(Some("/existing/dir"), None);
        assert_eq!(result, "/existing/dir/.semcode.db");
    }

    #[test]
    fn test_process_database_path_no_args_no_source() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("SEMCODE_DB").ok();
        std::env::remove_var("SEMCODE_DB");

        let result = process_database_path(None, None);
        assert_eq!(result, "./.semcode.db");

        if let Some(v) = saved {
            std::env::set_var("SEMCODE_DB", v);
        }
    }

    #[test]
    fn test_process_database_path_no_args_with_source() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("SEMCODE_DB").ok();
        std::env::remove_var("SEMCODE_DB");

        let source_path = Path::new("/source/code");
        let result = process_database_path(None, Some(source_path));
        assert_eq!(result, "/source/code/.semcode.db");

        if let Some(v) = saved {
            std::env::set_var("SEMCODE_DB", v);
        }
    }

    #[test]
    fn test_process_database_path_current_dir_source() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("SEMCODE_DB").ok();
        std::env::remove_var("SEMCODE_DB");

        let source_path = Path::new(".");
        let result = process_database_path(None, Some(source_path));
        assert_eq!(result, "./.semcode.db");

        if let Some(v) = saved {
            std::env::set_var("SEMCODE_DB", v);
        }
    }

    #[test]
    fn test_env_var_used_when_no_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("SEMCODE_DB").ok();
        std::env::set_var("SEMCODE_DB", "/data/my-project.semcode.db");

        let result = process_database_path(None, None);
        assert_eq!(result, "/data/my-project.semcode.db");

        // Also overrides source_dir fallback
        let result = process_database_path(None, Some(Path::new("/source/code")));
        assert_eq!(result, "/data/my-project.semcode.db");

        match saved {
            Some(v) => std::env::set_var("SEMCODE_DB", v),
            None => std::env::remove_var("SEMCODE_DB"),
        }
    }

    #[test]
    fn test_flag_overrides_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("SEMCODE_DB").ok();
        std::env::set_var("SEMCODE_DB", "/env/path.semcode.db");

        let result = process_database_path(Some("/flag/path.semcode.db"), None);
        assert_eq!(result, "/flag/path.semcode.db");

        match saved {
            Some(v) => std::env::set_var("SEMCODE_DB", v),
            None => std::env::remove_var("SEMCODE_DB"),
        }
    }

    #[test]
    fn test_empty_env_var_ignored() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("SEMCODE_DB").ok();
        std::env::set_var("SEMCODE_DB", "");

        let result = process_database_path(None, None);
        assert_eq!(result, "./.semcode.db");

        match saved {
            Some(v) => std::env::set_var("SEMCODE_DB", v),
            None => std::env::remove_var("SEMCODE_DB"),
        }
    }
}

#[cfg(test)]
mod one_argument_one_database {
    use super::*;

    #[test]
    fn a_directory_that_does_not_exist_yet_holds_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("index");
        assert_eq!(
            resolve_path(fresh.to_str().unwrap()),
            fresh.join(".semcode.db").to_string_lossy()
        );
    }

    #[test]
    fn the_same_argument_names_the_same_database_once_it_exists() {
        // What the indexer wrote is what a later query must read.
        let dir = tempfile::tempdir().unwrap();
        let arg = dir.path().join("index");
        let written = resolve_path(arg.to_str().unwrap());
        std::fs::create_dir_all(&written).unwrap();
        std::fs::create_dir_all(Path::new(&written).join("functions.lance")).unwrap();

        assert_eq!(resolve_path(arg.to_str().unwrap()), written);
    }

    #[test]
    fn a_database_written_at_the_path_itself_is_read_there() {
        // Databases already written this way keep working.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("functions.lance")).unwrap();
        let arg = dir.path().to_str().unwrap();

        assert_eq!(resolve_path(arg), arg);
    }
}
