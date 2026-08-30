// SPDX-License-Identifier: MIT OR Apache-2.0
//
// What the index holds when a file defines one name twice.
//
// A name is not one definition. arch/x86/coco/sev/core.c has
// `sev_es_play_dead` as a function under CONFIG_HOTPLUG_CPU and as a macro in
// the #else arm; drivers/gpu/drm/radeon/r600.c declares `r600_gpu_init` at
// line 112 and defines it at 1989. Keeping one row per name in a file
// discarded the other along with every call it makes, and which one survived
// depended on which arrived first.
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

async fn indexed_tree() -> (tempfile::TempDir, Arc<DatabaseManager>, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git_run(repo, &["init", "-q"]);

    std::fs::write(
        repo.join("core.c"),
        "#ifdef CONFIG_HOTPLUG_CPU\n\
static void play_dead(void)\n{\n\
\tplay_dead_common();\n\
\tsoft_restart_cpu();\n}\n\
#else\n\
#define play_dead\tnative_play_dead\n\
#endif\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("r600.c"),
        "static void gpu_init(struct device *dev);\n\n\
static void other(struct device *dev)\n{\n\
\tgpu_init(dev);\n}\n\n\
static void gpu_init(struct device *dev)\n{\n\
\tsetup_ring(dev);\n\
\tsetup_irq(dev);\n}\n",
    )
    .unwrap();
    git_run(repo, &["add", "."]);
    git_run(repo, &["commit", "-q", "-m", "two definitions per name"]);
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
async fn a_function_and_a_macro_of_one_name_both_reach_the_index() {
    let (_dir, db, sha) = indexed_tree().await;

    let rows = db
        .find_all_functions_git_aware("play_dead", &sha)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let body = rows
        .iter()
        .find(|f| f.calls.clone().unwrap_or_default().len() == 2)
        .unwrap_or_else(|| panic!("{rows:?}"));
    assert!(
        body.calls
            .clone()
            .unwrap_or_default()
            .iter()
            .any(|c| c == "soft_restart_cpu"),
        "{body:?}"
    );
}

#[tokio::test]
async fn a_prototype_does_not_displace_the_body_it_declares() {
    let (_dir, db, sha) = indexed_tree().await;

    // The body is the row a callee query has to be able to reach, whichever
    // of the two the storage happens to hold first.
    let callees = db
        .get_function_callees_git_aware("gpu_init", &sha)
        .await
        .unwrap();
    assert!(callees.iter().any(|c| c == "setup_ring"), "{callees:?}");
    assert!(callees.iter().any(|c| c == "setup_irq"), "{callees:?}");
}
