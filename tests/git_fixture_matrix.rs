use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use herdr_workbench::git::{GitStatusProvider, SourceControlGroup, StatusCode};
use tempfile::TempDir;

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git fixture command");
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
        .to_string()
}

fn init(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
}

fn write(root: &Path, path: impl AsRef<Path>, content: &str) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", message]);
}

fn entry(
    snapshot: &herdr_workbench::git::GitSnapshot,
    path: impl AsRef<Path>,
) -> &herdr_workbench::git::GitEntry {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.path == path.as_ref())
        .unwrap_or_else(|| panic!("missing Git entry {}", path.as_ref().display()))
}

#[test]
fn clean_unborn_and_complete_worktree_status_matrix() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path();
    init(root);

    let unborn = GitStatusProvider::new(root).refresh().unwrap();
    assert!(unborn.branch.unborn);
    assert_eq!(unborn.branch.name.as_deref(), Some("main"));

    for path in [
        "dual.txt",
        "unstaged.txt",
        "delete-staged.txt",
        "delete-unstaged.txt",
        "rename-origin.txt",
        "type-change.txt",
        "copy-source.txt",
    ] {
        write(root, path, &format!("{path}\n"));
    }
    commit_all(root, "base");
    let clean = GitStatusProvider::new(root).refresh().unwrap();
    assert!(clean.entries.is_empty());
    assert!(clean.branch.upstream.is_none());

    write(root, "dual.txt", "staged\n");
    git(root, &["add", "dual.txt"]);
    write(root, "dual.txt", "working\n");
    write(root, "unstaged.txt", "working\n");
    write(root, "added.txt", "added\n");
    git(root, &["add", "added.txt"]);
    write(root, "untracked.txt", "untracked\n");
    git(root, &["rm", "delete-staged.txt"]);
    fs::remove_file(root.join("delete-unstaged.txt")).unwrap();
    git(root, &["mv", "rename-origin.txt", "renamed.txt"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::remove_file(root.join("type-change.txt")).unwrap();
        symlink("dual.txt", root.join("type-change.txt")).unwrap();
    }

    git(root, &["config", "status.renames", "copies"]);
    fs::copy(root.join("copy-source.txt"), root.join("copy.txt")).unwrap();
    fs::copy(root.join("copy-source.txt"), root.join("copy-two.txt")).unwrap();
    git(root, &["rm", "copy-source.txt"]);
    git(root, &["add", "copy.txt", "copy-two.txt"]);

    let snapshot = GitStatusProvider::new(root).refresh().unwrap();
    let dual = entry(&snapshot, "dual.txt");
    assert_eq!(
        (dual.index_status, dual.worktree_status),
        (StatusCode::Modified, StatusCode::Modified)
    );
    assert_eq!(
        entry(&snapshot, "unstaged.txt").worktree_status,
        StatusCode::Modified
    );
    assert_eq!(
        entry(&snapshot, "added.txt").index_status,
        StatusCode::Added
    );
    assert!(entry(&snapshot, "untracked.txt").untracked);
    assert_eq!(
        entry(&snapshot, "delete-staged.txt").index_status,
        StatusCode::Deleted
    );
    assert_eq!(
        entry(&snapshot, "delete-unstaged.txt").worktree_status,
        StatusCode::Deleted
    );
    let renamed = entry(&snapshot, "renamed.txt");
    assert_eq!(renamed.index_status, StatusCode::Renamed);
    assert_eq!(
        renamed.rename_origin.as_deref(),
        Some(Path::new("rename-origin.txt"))
    );
    #[cfg(unix)]
    assert_eq!(
        entry(&snapshot, "type-change.txt").worktree_status,
        StatusCode::TypeChanged
    );
    let copied = snapshot
        .entries
        .iter()
        .find(|entry| entry.index_status == StatusCode::Copied)
        .expect("repository configured for copy detection emits a copied record");
    assert_eq!(copied.index_status, StatusCode::Copied);
    assert_eq!(
        copied.rename_origin.as_deref(),
        Some(Path::new("copy-source.txt"))
    );

    let groups = snapshot.groups();
    assert!(
        groups[&SourceControlGroup::StagedChanges]
            .iter()
            .any(|entry| entry.path == Path::new("dual.txt"))
    );
    assert!(
        groups[&SourceControlGroup::Changes]
            .iter()
            .any(|entry| entry.path == Path::new("dual.txt"))
    );
}

#[test]
fn ignored_and_unusual_names_are_lossless() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path();
    init(root);
    write(
        root,
        ".gitignore",
        "ignored-file\nignored-dir/\ncontents-only/*\n",
    );
    commit_all(root, "ignore rules");

    for path in [
        "ignored-file",
        "ignored-dir/inside.txt",
        "contents-only/inside.txt",
        "space name.txt",
        "tab\tname.txt",
        "line\nname.txt",
        "ümlaut-文件.txt",
        "-leading-dash.txt",
    ] {
        write(root, path, "fixture\n");
    }

    let snapshot = GitStatusProvider::new(root).refresh().unwrap();
    assert!(entry(&snapshot, "ignored-file").ignored);
    assert!(entry(&snapshot, "ignored-dir").ignored);
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.ignored && entry.path.starts_with("contents-only"))
    );
    for path in [
        "space name.txt",
        "tab\tname.txt",
        "line\nname.txt",
        "ümlaut-文件.txt",
        "-leading-dash.txt",
    ] {
        assert!(entry(&snapshot, path).untracked, "{path:?}");
    }
}

#[test]
fn every_porcelain_unmerged_combination_preserves_stage_metadata() {
    let combinations: [(&str, &[u8]); 7] = [
        ("DD", &[1]),
        ("AU", &[2]),
        ("UD", &[1, 2]),
        ("UA", &[3]),
        ("DU", &[1, 3]),
        ("AA", &[2, 3]),
        ("UU", &[1, 2, 3]),
    ];

    for (xy, stages) in combinations {
        let fixture = TempDir::new().unwrap();
        let root = fixture.path();
        init(root);
        write(root, "blob.txt", "stage\n");
        let oid = git(root, &["hash-object", "-w", "blob.txt"]);
        git(root, &["read-tree", "--empty"]);

        let mut child = Command::new("git")
            .args(["update-index", "--index-info"])
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            for stage in stages {
                writeln!(stdin, "100644 {oid} {stage}\tconflict-{xy}.txt").unwrap();
            }
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let snapshot = GitStatusProvider::new(root).refresh().unwrap();
        let conflicted = entry(&snapshot, format!("conflict-{xy}.txt"));
        let conflict = conflicted.conflict.as_ref().expect("unmerged metadata");
        assert_eq!(
            (conflict.index, conflict.worktree),
            (xy.as_bytes()[0] as char, xy.as_bytes()[1] as char)
        );
        assert_eq!(conflict.modes.len(), 4);
        assert_eq!(conflict.stages.len(), 3);
        assert_eq!(
            snapshot.groups()[&SourceControlGroup::MergeChanges].len(),
            1
        );
    }
}

#[test]
fn branch_stash_worktree_nested_repository_and_submodule_matrix() {
    let fixture = TempDir::new().unwrap();
    let repository = fixture.path().join("repository");
    let remote = fixture.path().join("remote.git");
    let peer = fixture.path().join("peer");
    let linked = fixture.path().join("linked");
    init(&repository);
    write(&repository, "tracked.txt", "base\n");
    commit_all(&repository, "base");
    fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare", "--initial-branch=main"]);
    git(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repository, &["push", "-u", "origin", "main"]);

    git(
        fixture.path(),
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
    );
    git(&peer, &["config", "user.name", "Fixture"]);
    git(&peer, &["config", "user.email", "fixture@example.invalid"]);
    write(&peer, "peer.txt", "behind\n");
    commit_all(&peer, "peer");
    git(&peer, &["push"]);

    write(&repository, "local.txt", "ahead\n");
    commit_all(&repository, "local");
    git(&repository, &["fetch", "origin"]);
    write(&repository, "tracked.txt", "stash\n");
    git(&repository, &["stash", "push", "-m", "fixture"]);
    let diverged = GitStatusProvider::new(&repository).refresh().unwrap();
    assert_eq!((diverged.branch.ahead, diverged.branch.behind), (1, 1));
    assert_eq!(diverged.branch.stash_count, 1);
    assert_eq!(diverged.branch.upstream.as_deref(), Some("origin/main"));

    git(
        &repository,
        &["worktree", "add", linked.to_str().unwrap(), "HEAD"],
    );
    let worktree = GitStatusProvider::new(&linked).refresh().unwrap();
    assert!(worktree.root.ends_with("linked"));
    git(&linked, &["checkout", "--detach"]);
    assert!(
        GitStatusProvider::new(&linked)
            .refresh()
            .unwrap()
            .branch
            .detached
    );

    let nested = repository.join("nested");
    init(&nested);
    write(&nested, "nested.txt", "nested\n");
    commit_all(&nested, "nested");
    let nested_parent = GitStatusProvider::new(&repository).refresh().unwrap();
    assert!(
        nested_parent
            .entries
            .iter()
            .any(|entry| entry.path.starts_with("nested"))
    );

    let submodule_source = fixture.path().join("submodule-source");
    init(&submodule_source);
    write(&submodule_source, "module.txt", "module\n");
    commit_all(&submodule_source, "module");
    git(
        &repository,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            submodule_source.to_str().unwrap(),
            "module",
        ],
    );
    commit_all(&repository, "submodule");
    write(&repository.join("module"), "module.txt", "modified\n");
    let submodule = GitStatusProvider::new(&repository).refresh().unwrap();
    let module = entry(&submodule, "module");
    assert!(module.submodule.is_submodule);
    assert!(module.submodule.modified);
}

#[test]
fn leading_dash_never_becomes_a_git_option_in_diff_preview() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path();
    init(root);
    write(root, "-option.txt", "base\n");
    commit_all(root, "base");
    write(root, "-option.txt", "changed\n");
    let provider = GitStatusProvider::new(root);
    let snapshot = provider.refresh().unwrap();
    let changed = entry(&snapshot, PathBuf::from("-option.txt"));
    let preview = provider
        .preview(changed, SourceControlGroup::Changes)
        .unwrap();
    assert!(preview.contains("-base"));
    assert!(preview.contains("+changed"));
}
