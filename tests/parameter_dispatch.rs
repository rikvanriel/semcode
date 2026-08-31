// SPDX-License-Identifier: MIT OR Apache-2.0
//
// What a call through the enclosing function's own parameter can reach.
//
// `fn(...)` where `fn` is a parameter has no struct member to join on, so no
// registration names a candidate and the site reads as a dead end. The other
// half is what the function's own callers hand to that position.
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

async fn indexed() -> (tempfile::TempDir, Arc<DatabaseManager>, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git_run(repo, &["init", "-q"]);
    std::fs::write(
        repo.join("cb.c"),
        "static void handler_a(int x) { work_a(x); }\n\
static void handler_b(int x) { work_b(x); }\n\n\
static void run(int n, void (*fn)(int))\n{\n\
\tfn(n);\n}\n\n\
static void caller_one(void)\n{\n\
\trun(1, handler_a);\n}\n\n\
static void caller_two(void)\n{\n\
\trun(2, handler_b);\n}\n\n\
static void takes_a_value(int n, int limit)\n{\n\
\tuse(n, limit);\n}\n\n\
static void passes_a_value(void)\n{\n\
\ttakes_a_value(1, some_constant);\n}\n",
    )
    .unwrap();
    git_run(repo, &["add", "."]);
    git_run(repo, &["commit", "-q", "-m", "callbacks"]);
    git_run(repo, &["branch", "-M", "main"]);
    let sha = git::get_git_sha(repo).unwrap().unwrap();

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
    semcode::git_range::process_git_tree(repo, &sha, &extensions, db.clone(), false, 1)
        .await
        .unwrap();
    (dir, db, sha)
}

#[tokio::test]
async fn a_call_through_a_parameter_reaches_what_callers_hand_it() {
    let (_dir, db, sha) = indexed().await;

    let dispatches = db
        .find_parameter_dispatch_git_aware("run", &sha)
        .await
        .unwrap();
    assert_eq!(dispatches.len(), 1, "{dispatches:?}");
    let found = &dispatches[0];
    assert_eq!(found.parameter, "fn", "{found:?}");
    assert_eq!(found.position, 1, "{found:?}");
    let names: Vec<&str> = found
        .candidates
        .iter()
        .map(|(n, _, _)| n.as_str())
        .collect();
    assert!(names.contains(&"handler_a"), "{names:?}");
    assert!(names.contains(&"handler_b"), "{names:?}");
    // Every candidate says where a caller hands it over, so the reader can
    // check rather than take the list on faith.
    assert!(
        found
            .candidates
            .iter()
            .all(|(_, file, line)| file.ends_with("cb.c") && *line > 0),
        "{found:?}"
    );
}

#[tokio::test]
async fn an_argument_that_is_not_a_function_is_not_a_candidate() {
    // `some_constant` is handed to a parameter and names no function; a call
    // through that parameter would be reported reaching something that does
    // not exist.
    let (_dir, db, sha) = indexed().await;

    let dispatches = db
        .find_parameter_dispatch_git_aware("takes_a_value", &sha)
        .await
        .unwrap();
    assert!(dispatches.is_empty(), "{dispatches:?}");
}
