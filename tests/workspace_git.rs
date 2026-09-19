use std::{fs, path::Path, process::Command};

use herdr_workbench::{
    git::StatusCode,
    workspace_git::{WorkspaceGitProvider, WorkspaceGitSnapshot},
};
use tempfile::TempDir;

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run fixture Git command");
    assert!(
        output.status.success(),
        "git {} failed in {}:\n{}",
        arguments.join(" "),
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("fixture output is UTF-8")
        .trim()
        .to_owned()
}

fn init(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
}

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", message]);
}

fn changed_repository(root: &Path) {
    init(root);
    write(root, "shared.txt", "base\n");
    write(root, "old.txt", "rename me\n");
    commit_all(root, "initial");
    write(root, "shared.txt", "staged\n");
    git(root, &["add", "shared.txt"]);
    write(root, "shared.txt", "working tree\n");
    git(root, &["mv", "old.txt", "renamed.txt"]);
    write(root, ".gitignore", "ignored/\n");
    write(root, "ignored/secret.txt", "ignored\n");
}

fn repository_paths(snapshot: &WorkspaceGitSnapshot) -> Vec<std::path::PathBuf> {
    snapshot
        .repositories
        .iter()
        .map(|repository| repository.root.clone())
        .collect()
}

#[test]
fn an_empty_non_git_workspace_has_an_empty_successful_snapshot() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();

    let provider = WorkspaceGitProvider::new(&workspace);
    let snapshot = provider.refresh().unwrap();
    assert!(snapshot.repositories.is_empty());
    assert!(snapshot.combined.entries.is_empty());
    assert!(snapshot.combined.directories.is_empty());
    assert!(snapshot.errors.is_empty());
    assert!(provider.history().unwrap().is_empty());
}

#[test]
fn empty_git_cache_markers_do_not_report_repository_errors() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    write(&workspace, ".build/uv-cache/sdists-v9/.git", "");

    for root in [&workspace, &workspace.join(".build/uv-cache/sdists-v9")] {
        let provider = WorkspaceGitProvider::new(root);
        let snapshot = provider.refresh().unwrap();
        assert!(snapshot.errors.is_empty(), "{:?}", snapshot.errors);
        assert!(snapshot.repositories.is_empty());
        assert!(snapshot.combined.entries.is_empty());
        assert!(provider.history().unwrap().is_empty());
    }
}

#[test]
fn empty_git_markers_do_not_hide_sibling_or_child_repositories() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    write(&workspace, "cache/.git", "");
    let child = workspace.join("cache/child");
    let sibling = workspace.join("sibling");
    changed_repository(&child);
    changed_repository(&sibling);

    let provider = WorkspaceGitProvider::new(&workspace);
    let snapshot = provider.refresh().unwrap();
    assert!(snapshot.errors.is_empty(), "{:?}", snapshot.errors);
    assert_eq!(
        repository_paths(&snapshot),
        vec![
            child.canonicalize().unwrap(),
            sibling.canonicalize().unwrap()
        ]
    );
    for prefix in ["cache/child", "sibling"] {
        assert!(snapshot.combined.entries.iter().any(|entry| {
            entry.path == Path::new(prefix).join("shared.txt")
                && entry.index_status == StatusCode::Modified
                && entry.worktree_status == StatusCode::Modified
        }));
    }
    let history = provider.history().unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].repository, Path::new("cache/child"));
    assert_eq!(history[1].repository, Path::new("sibling"));
}

#[test]
fn corrupt_git_marker_reports_an_error_without_hiding_healthy_siblings() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let healthy = workspace.join("healthy");
    let corrupt = workspace.join("corrupt");
    changed_repository(&healthy);
    fs::create_dir_all(&corrupt).unwrap();
    fs::write(corrupt.join(".git"), "not a Git directory\n").unwrap();

    let snapshot = WorkspaceGitProvider::new(&workspace).refresh().unwrap();
    assert_eq!(
        repository_paths(&snapshot),
        vec![healthy.canonicalize().unwrap()]
    );
    assert_eq!(snapshot.errors.len(), 1);
    assert!(
        snapshot.errors[0]
            .command
            .contains(&corrupt.display().to_string())
    );
}

#[test]
fn an_ancestor_repository_created_after_provider_open_does_not_expand_workspace_scope() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let provider = WorkspaceGitProvider::new(&workspace);

    init(fixture.path());
    let snapshot = provider.refresh().unwrap();
    assert!(snapshot.repositories.is_empty());
    assert!(snapshot.errors.is_empty());
    assert_eq!(snapshot.combined.root, workspace.canonicalize().unwrap());
}

#[test]
fn combines_sibling_repositories_without_losing_repository_relative_state() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let first = workspace.join("containers/first");
    let second = workspace.join("second");
    changed_repository(&first);
    changed_repository(&second);

    let snapshot = WorkspaceGitProvider::new(&workspace).refresh().unwrap();
    assert_eq!(
        repository_paths(&snapshot),
        vec![
            first.canonicalize().unwrap(),
            second.canonicalize().unwrap()
        ]
    );
    assert!(snapshot.errors.is_empty());
    assert_eq!(snapshot.combined.root, workspace.canonicalize().unwrap());
    assert_eq!(snapshot.combined.branch, Default::default());

    for repository in &snapshot.repositories {
        assert!(repository.root.is_absolute());
        let shared = repository
            .entries
            .iter()
            .find(|entry| entry.path == Path::new("shared.txt"))
            .unwrap();
        assert_eq!(shared.index_status, StatusCode::Modified);
        assert_eq!(shared.worktree_status, StatusCode::Modified);
        let renamed = repository
            .entries
            .iter()
            .find(|entry| entry.path == Path::new("renamed.txt"))
            .unwrap();
        assert_eq!(renamed.rename_origin.as_deref(), Some(Path::new("old.txt")));
        assert!(repository.entries.iter().any(|entry| entry.ignored));
    }

    for prefix in [Path::new("containers/first"), Path::new("second")] {
        let shared = snapshot
            .combined
            .entries
            .iter()
            .find(|entry| entry.path == prefix.join("shared.txt"))
            .unwrap();
        assert_eq!(shared.index_status, StatusCode::Modified);
        assert_eq!(shared.worktree_status, StatusCode::Modified);
        let renamed = snapshot
            .combined
            .entries
            .iter()
            .find(|entry| entry.path == prefix.join("renamed.txt"))
            .unwrap();
        assert_eq!(
            renamed.rename_origin.as_deref(),
            Some(prefix.join("old.txt").as_path())
        );
        assert!(snapshot.combined.directories.contains_key(prefix));
    }
}

#[test]
fn discovers_worktree_git_files_and_never_follows_directory_symlinks() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let source = fixture.path().join("source");
    init(&source);
    write(&source, "tracked.txt", "base\n");
    commit_all(&source, "initial");
    let linked = workspace.join("linked-worktree");
    git(
        &source,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    assert!(linked.join(".git").is_file());

    #[cfg(unix)]
    {
        let escaped = fixture.path().join("escaped-repository");
        changed_repository(&escaped);
        std::os::unix::fs::symlink(&escaped, workspace.join("escape")).unwrap();
    }

    let snapshot = WorkspaceGitProvider::new(&workspace).refresh().unwrap();
    assert_eq!(
        repository_paths(&snapshot),
        vec![linked.canonicalize().unwrap()]
    );
    assert!(snapshot.repositories[0].entries.is_empty());
}

#[test]
fn refresh_rediscovers_added_and_removed_repositories() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let first = workspace.join("first");
    changed_repository(&first);
    let provider = WorkspaceGitProvider::new(&workspace);
    assert_eq!(provider.refresh().unwrap().repositories.len(), 1);

    let second = workspace.join("nested/second");
    changed_repository(&second);
    let added = provider.refresh().unwrap();
    assert_eq!(
        repository_paths(&added),
        vec![
            first.canonicalize().unwrap(),
            second.canonicalize().unwrap()
        ]
    );

    fs::rename(second.join(".git"), second.join("git-moved")).unwrap();
    let removed = provider.refresh().unwrap();
    assert_eq!(
        repository_paths(&removed),
        vec![first.canonicalize().unwrap()]
    );
}

#[test]
fn a_failed_repository_retains_its_snapshot_while_healthy_repositories_refresh() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let failing = workspace.join("failing");
    let healthy = workspace.join("healthy");
    changed_repository(&failing);
    changed_repository(&healthy);
    let provider = WorkspaceGitProvider::new(&workspace);
    let valid = provider.refresh().unwrap();

    fs::remove_file(failing.join(".git/index")).unwrap();
    fs::create_dir(failing.join(".git/index")).unwrap();
    write(&healthy, "healthy.txt", "new\n");

    let refreshed = provider.refresh().unwrap();
    assert_eq!(refreshed.repositories.len(), 2);
    assert_eq!(refreshed.errors.len(), 1);
    assert!(
        refreshed.errors[0]
            .command
            .contains(&failing.display().to_string())
    );
    let prior_failing = valid
        .repositories
        .iter()
        .find(|snapshot| snapshot.root == failing.canonicalize().unwrap())
        .unwrap();
    let retained_failing = refreshed
        .repositories
        .iter()
        .find(|snapshot| snapshot.root == failing.canonicalize().unwrap())
        .unwrap();
    assert_eq!(retained_failing, prior_failing);
    assert!(
        refreshed
            .repositories
            .iter()
            .find(|snapshot| snapshot.root == healthy.canonicalize().unwrap())
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.path == Path::new("healthy.txt"))
    );
}

#[test]
fn history_is_grouped_by_repository_in_discovery_order() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let first = workspace.join("first");
    let second = workspace.join("second");
    init(&first);
    write(&first, "first.txt", "one\n");
    commit_all(&first, "first initial");
    write(&first, "first.txt", "two\n");
    commit_all(&first, "first latest");
    init(&second);
    write(&second, "second.txt", "one\n");
    commit_all(&second, "second initial");
    write(&second, "second.txt", "two\n");
    commit_all(&second, "second latest");

    let provider = WorkspaceGitProvider::new(&workspace);
    let history = provider.history().unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].repository, Path::new("first"));
    assert_eq!(history[0].commit.summary, "first latest");
    assert_eq!(history[1].repository, Path::new("first"));
    assert_eq!(history[2].repository, Path::new("second"));
    assert_eq!(history[2].commit.summary, "second latest");
    assert_eq!(provider.history_async().recv().unwrap().unwrap(), history);

    let refreshed = provider.refresh_async().recv().unwrap().unwrap();
    assert_eq!(refreshed.repositories.len(), 2);
}

#[test]
fn a_root_repository_owns_nested_directories() {
    let fixture = TempDir::new().unwrap();
    let workspace = fixture.path().join("workspace");
    let nested = workspace.join("nested");
    init(&workspace);
    write(&workspace, "root.txt", "base\n");
    commit_all(&workspace, "root initial");
    init(&nested);
    write(&nested, "nested.txt", "base\n");
    commit_all(&nested, "nested initial");

    let provider = WorkspaceGitProvider::new(&workspace);
    let snapshot = provider.refresh().unwrap();
    assert_eq!(
        repository_paths(&snapshot),
        vec![workspace.canonicalize().unwrap()]
    );
    let history = provider.history().unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].repository.as_os_str().is_empty());
}
