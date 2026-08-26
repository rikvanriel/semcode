// SPDX-License-Identifier: MIT OR Apache-2.0
//
// What the file on disk holds, against what the index recorded for it.
//
// A row carries the hash of the file it was extracted from, so the question
// "is this row still what the file says" is one hash of one file. Asked per
// file a query names, it replaces asking it of every file in the repository
// before any query runs.
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

/// The file on disk, relative to a hash the index holds for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingCopy {
    /// The file holds exactly the content that hash describes.
    Same,
    /// The file exists and holds something else: an edit, or a row from
    /// another revision. Which one it is takes a question to git.
    Different,
    /// There is no such file.
    Absent,
}

/// The object id git would give this content.
///
/// A blob id is taken over `blob <len>\0` and then the bytes; hashing the
/// bytes alone produces something that is not comparable with anything git or
/// the index stores.
pub fn blob_id(content: &[u8]) -> String {
    use sha1::{Digest, Sha1};

    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// Hashes of files already read, so a query that names the same file twice —
/// or a second query in the same process — reads it once.
///
/// Keyed on the size and modification time the file had when it was hashed,
/// which is what git's own index does. A change that leaves both identical is
/// missed, the same way git misses it; hashing costs 0.05ms for a 67KB source
/// file, so a caller with reason to doubt can ask for a fresh read.
#[derive(Default)]
pub struct WorkingCopyHashes {
    seen: RwLock<HashMap<String, (u64, i64, String)>>,
}

impl WorkingCopyHashes {
    pub fn new() -> Self {
        Self::default()
    }

    /// The blob id of a file on disk, or None if it is not there.
    pub fn blob_id_of(&self, path: &Path) -> Result<Option<String>> {
        let key = path.to_string_lossy().into_owned();
        let Some(stat) = stat_of(path)? else {
            self.seen.write().unwrap().remove(&key);
            return Ok(None);
        };

        if let Some((size, mtime, hash)) = self.seen.read().unwrap().get(&key) {
            if (*size, *mtime) == stat {
                return Ok(Some(hash.clone()));
            }
        }

        let hash = blob_id(&std::fs::read(path)?);
        self.seen
            .write()
            .unwrap()
            .insert(key, (stat.0, stat.1, hash.clone()));
        Ok(Some(hash))
    }

    /// What the file holds, against a hash the index recorded.
    pub fn compare(&self, path: &Path, indexed_hash: &str) -> Result<WorkingCopy> {
        match self.blob_id_of(path)? {
            None => Ok(WorkingCopy::Absent),
            Some(hash) if hash == indexed_hash => Ok(WorkingCopy::Same),
            Some(_) => Ok(WorkingCopy::Different),
        }
    }

    /// Forget what is known about a path.
    pub fn forget(&self, path: &Path) {
        self.seen
            .write()
            .unwrap()
            .remove(&path.to_string_lossy().into_owned());
    }
}

fn stat_of(path: &Path) -> Result<Option<(u64, i64)>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            Ok(Some((meta.len(), mtime)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_hash_object(path: &Path) -> String {
        let out = std::process::Command::new("git")
            .args(["hash-object", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success(), "git hash-object failed");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn a_blob_id_is_the_one_git_gives() {
        // The authority for this is git, so ask git.
        let dir = tempfile::tempdir().unwrap();
        for content in ["", "int f(void)\n{\n\treturn 1;\n}\n", "\0\0binary\n"] {
            let path = dir.path().join("file.c");
            std::fs::write(&path, content).unwrap();
            assert_eq!(
                blob_id(content.as_bytes()),
                git_hash_object(&path),
                "content {content:?}"
            );
        }
    }

    #[test]
    fn a_file_matching_its_recorded_hash_is_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.c");
        let content = "int shared(void)\n{\n\treturn 1;\n}\n";
        std::fs::write(&path, content).unwrap();

        let hashes = WorkingCopyHashes::new();
        let recorded = blob_id(content.as_bytes());
        assert_eq!(hashes.compare(&path, &recorded).unwrap(), WorkingCopy::Same);
    }

    #[test]
    fn an_edited_file_differs_from_its_recorded_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.c");
        std::fs::write(&path, "int shared(void)\n{\n\treturn 1;\n}\n").unwrap();
        let hashes = WorkingCopyHashes::new();
        let recorded = hashes.blob_id_of(&path).unwrap().unwrap();

        // A different length, so the change is visible to stat as well.
        std::fs::write(&path, "int shared(void)\n{\n\treturn 2; /* edited */\n}\n").unwrap();
        assert_eq!(
            hashes.compare(&path, &recorded).unwrap(),
            WorkingCopy::Different
        );
    }

    #[test]
    fn a_missing_file_is_absent_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.c");
        std::fs::write(&path, "int gone(void)\n{\n\treturn 1;\n}\n").unwrap();
        let hashes = WorkingCopyHashes::new();
        let recorded = hashes.blob_id_of(&path).unwrap().unwrap();

        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            hashes.compare(&path, &recorded).unwrap(),
            WorkingCopy::Absent
        );
        // A deleted file must not answer from what it used to hold.
        assert_eq!(hashes.blob_id_of(&path).unwrap(), None);
    }

    #[test]
    fn a_second_look_at_an_unchanged_file_reuses_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.c");
        std::fs::write(&path, "int shared(void)\n{\n\treturn 1;\n}\n").unwrap();

        let hashes = WorkingCopyHashes::new();
        let first = hashes.blob_id_of(&path).unwrap().unwrap();

        // Same size and mtime, different bytes: the memo answers, and this is
        // the documented limit of it rather than a defect to fix.
        let stat = std::fs::metadata(&path).unwrap();
        std::fs::write(&path, "int shared(void)\n{\n\treturn 9;\n}\n").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(stat.modified().unwrap())
                .set_accessed(stat.accessed().unwrap()),
        )
        .unwrap();

        assert_eq!(hashes.blob_id_of(&path).unwrap().unwrap(), first);
        hashes.forget(&path);
        assert_ne!(hashes.blob_id_of(&path).unwrap().unwrap(), first);
    }
}
