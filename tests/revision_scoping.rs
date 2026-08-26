// SPDX-License-Identifier: MIT OR Apache-2.0
//
// What a query answers when the index holds more revisions than the caller
// asked about.
use semcode::{git, DatabaseManager};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn git_run(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Semcode Test")
        .env("GIT_AUTHOR_EMAIL", "semcode@example.com")
        .env("GIT_COMMITTER_NAME", "Semcode Test")
        .env("GIT_COMMITTER_EMAIL", "semcode@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn head_of(repo: &Path) -> String {
    git::get_git_sha(repo).unwrap().unwrap()
}

/// A repository with two branches, and an index holding both.
///
/// `shared.c` is on both branches. `only_on_topic.c` is on `topic` alone, so
/// a query that says it wants `main` is asking about a tree that never
/// contained `topic_only`.
async fn two_branches() -> (tempfile::TempDir, Arc<DatabaseManager>, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();

    git_run(repo, &["init", "-q"]);
    std::fs::write(
        repo.join("shared.c"),
        "int shared_function(void)\n{\n\
\tint a = 1;\n\
\tint b = 2;\n\
\tint c = a + b + 42;\n\
\tchar buf[64];\n\
\tsnprintf(buf, sizeof(buf), \"%d\", c);\n\
\treturn c + (int)strlen(buf);\n}\n",
    )
    .unwrap();
    git_run(repo, &["add", "."]);
    git_run(repo, &["commit", "-q", "-m", "shared"]);
    git_run(repo, &["branch", "-M", "main"]);
    let main_sha = head_of(repo);

    git_run(repo, &["checkout", "-q", "-b", "topic"]);
    std::fs::write(
        repo.join("only_on_topic.c"),
        "int topic_only(void)\n{\n\
\tint x = 10;\n\
\tint y = 20;\n\
\tint z = x * y + 7;\n\
\tchar tmp[64];\n\
\tsnprintf(tmp, sizeof(tmp), \"topic %d\", z);\n\
\treturn z + (int)strlen(tmp);\n}\n",
    )
    .unwrap();
    git_run(repo, &["add", "."]);
    git_run(repo, &["commit", "-q", "-m", "topic"]);
    let topic_sha = head_of(repo);

    let db = Arc::new(
        DatabaseManager::new(
            repo.join(".semcode.db").to_str().unwrap(),
            repo.to_string_lossy().into_owned(),
        )
        .await
        .unwrap(),
    );
    db.create_tables().await.unwrap();

    let extensions = ["c".to_string(), "h".to_string()];
    for sha in [&main_sha, &topic_sha] {
        semcode::git_range::process_git_tree(repo, sha, &extensions, db.clone(), false, 1)
            .await
            .unwrap();
    }

    // Leave the checkout on main: the caller asks about main, and main is
    // what is on disk.
    git_run(repo, &["checkout", "-q", "main"]);

    (dir, db, main_sha, topic_sha)
}

#[tokio::test]
async fn a_function_from_another_branch_is_answered_for_this_one() {
    // Pinned, not endorsed.
    //
    // `topic_only` exists only on `topic`. Asked for at `main`, the file it
    // lives in does not exist, so no path resolves, and the lookup falls back
    // to every revision in the index rather than reporting an absence:
    //
    //     if resolved_hashes.is_empty() {
    //         tracing::warn!("No files resolved for '{}' at commit '{}'", name, git_sha);
    //         // Fallback: get all functions and filter implementations
    //
    // The warning goes to a log, and the caller is told about a function that
    // is not in the tree it named. This test records that, and is flipped by
    // the patch that makes the revision decide.
    let (_dir, db, main_sha, _topic_sha) = two_branches().await;

    let found = db
        .find_all_functions_git_aware("topic_only", &main_sha)
        .await
        .unwrap();

    assert!(
        !found.is_empty(),
        "the fallback is gone; flip this test to assert the absence"
    );
    assert_eq!(found[0].name, "topic_only");
}

#[tokio::test]
async fn a_function_on_this_branch_is_answered_for_it() {
    // The control: whatever the patch does to the case above, this one has to
    // keep working, at either revision.
    let (_dir, db, main_sha, topic_sha) = two_branches().await;

    for sha in [&main_sha, &topic_sha] {
        let found = db
            .find_all_functions_git_aware("shared_function", sha)
            .await
            .unwrap();
        assert_eq!(found.len(), 1, "at {sha}: {found:?}");
        assert_eq!(found[0].file_path, "shared.c");
    }
}

#[tokio::test]
async fn a_function_added_on_a_branch_is_answered_for_that_branch() {
    let (_dir, db, _main_sha, topic_sha) = two_branches().await;

    let found = db
        .find_all_functions_git_aware("topic_only", &topic_sha)
        .await
        .unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].file_path, "only_on_topic.c");
}
