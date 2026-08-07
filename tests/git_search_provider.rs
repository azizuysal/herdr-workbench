use std::{fs, process::Command, sync::atomic::AtomicBool};

use herdr_workbench::{
    git::{GitStatusProvider, SourceControlGroup, StatusCode, parse_porcelain_v2},
    search::{SearchMode, SearchProvider, SearchQuery, SearchUpdate},
};
use tempfile::tempdir;

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn git_output(root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn repository() -> tempfile::TempDir {
    let root = tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(root.path(), &["config", "user.name", "Test"]);
    fs::write(root.path().join("tracked.txt"), "base\n").unwrap();
    git(root.path(), &["add", "tracked.txt"]);
    git(root.path(), &["commit", "-qm", "initial"]);
    root
}

#[test]
fn porcelain_parser_preserves_dual_status_rename_origin_and_raw_paths() {
    let input = b"# branch.oid abc\x00# branch.head main\x00# branch.upstream origin/main\x00# branch.ab +2 -3\x00# stash 1\x001 MM N... 100644 100644 100644 a b odd\tname\x002 R. N... 100644 100644 100644 a b R100 new\nname\x00old name\x00! ignored-dir/\x00";
    let snapshot = parse_porcelain_v2("/workspace".into(), input).unwrap();
    assert_eq!(snapshot.branch.ahead, 2);
    assert_eq!(snapshot.branch.behind, 3);
    assert_eq!(snapshot.branch.stash_count, 1);
    assert_eq!(snapshot.entries[0].index_status, StatusCode::Modified);
    assert_eq!(snapshot.entries[0].worktree_status, StatusCode::Modified);
    assert_eq!(snapshot.entries[0].badge(), "M");
    assert_eq!(
        snapshot.entries[1].rename_origin.as_deref(),
        Some(std::path::Path::new("old name"))
    );
    assert_eq!(
        snapshot.groups()[&SourceControlGroup::StagedChanges].len(),
        2
    );
    assert_eq!(
        snapshot.directories[std::path::Path::new("")].status,
        herdr_workbench::git::DirectoryStatus::ModifiedOrTypeChanged
    );
}

#[test]
fn git_snapshot_keeps_staged_and_unstaged_membership_and_ignored_paths() {
    let repository = repository();
    let root = repository.path();
    fs::write(root.join("tracked.txt"), "staged\n").unwrap();
    git(root, &["add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "unstaged\n").unwrap();
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    fs::create_dir(root.join("ignored")).unwrap();
    fs::write(root.join("ignored/file.txt"), "nope").unwrap();
    fs::write(root.join("untracked file.txt"), "new\n").unwrap();
    let provider = GitStatusProvider::new(root);
    let snapshot = provider.refresh().unwrap();
    assert_eq!(
        snapshot.groups()[&SourceControlGroup::StagedChanges].len(),
        1
    );
    assert_eq!(snapshot.groups()[&SourceControlGroup::Changes].len(), 1);
    assert_eq!(snapshot.groups()[&SourceControlGroup::Untracked].len(), 2); // .gitignore + new file
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.ignored && entry.path == std::path::Path::new("ignored/"))
    );
}

#[test]
fn git_failure_retains_the_last_valid_snapshot_and_conflicts_remain_distinct() {
    let repository = repository();
    let provider = GitStatusProvider::new(repository.path());
    let valid = provider.refresh().unwrap();
    fs::rename(
        repository.path().join(".git"),
        repository.path().join("git-moved"),
    )
    .unwrap();
    assert!(provider.refresh().is_err());
    assert_eq!(provider.last_valid(), Some(valid));

    let input = b"u AA N... 100644 100644 100644 100644 a b c conflicted\0";
    let snapshot = parse_porcelain_v2("/workspace".into(), input).unwrap();
    let entry = &snapshot.entries[0];
    assert!(entry.conflict.is_some());
    assert_eq!(
        snapshot.groups()[&SourceControlGroup::MergeChanges],
        vec![entry]
    );
    assert_eq!(entry.badge(), "C");
}

#[test]
fn history_is_newest_first_and_commit_preview_contains_metadata_stat_and_patch() {
    let repository = repository();
    let root = repository.path();
    fs::write(root.join("tracked.txt"), "base\nsecond\n").unwrap();
    git(root, &["add", "tracked.txt"]);
    git(root, &["commit", "-qm", "second change"]);
    let head = git_output(root, &["rev-parse", "HEAD"]);

    let provider = GitStatusProvider::new(root);
    let history = provider.history().unwrap();

    assert_eq!(history.len(), 2);
    assert_eq!(history[0].oid, head);
    assert_eq!(history[0].summary, "second change");
    assert_eq!(history[1].summary, "initial");
    let preview = provider.commit_preview(&history[0].oid).unwrap();
    assert!(preview.contains(&format!("commit {}", history[0].oid)));
    assert!(preview.contains("second change"));
    assert!(preview.contains("tracked.txt | 1 +"));
    assert!(preview.contains("+second"));
}

#[test]
fn unborn_repository_has_an_empty_history() {
    let repository = tempdir().unwrap();
    git(repository.path(), &["init", "-q"]);

    assert!(
        GitStatusProvider::new(repository.path())
            .history()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn search_is_sanitized_root_bound_and_respects_ignored_toggle() {
    let root = tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), "/ignored/\n").unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".git/private-log"), "needle").unwrap();
    fs::create_dir(root.path().join("ignored")).unwrap();
    fs::write(root.path().join("ignored/no.txt"), "needle").unwrap();
    fs::write(root.path().join("visible.txt"), "needle\x1b[31m\n").unwrap();
    let provider = SearchProvider::new(root.path());
    let query = SearchQuery {
        text: "needle".into(),
        mode: SearchMode::Literal,
        case_sensitive: true,
        include_ignored: false,
    };
    let results = provider.search(&query, &AtomicBool::new(false)).unwrap();
    assert_eq!(results.files.len(), 1);
    assert!(!results.files[0].matches[0].snippet.contains('\x1b'));
    let included = provider
        .search(
            &SearchQuery {
                include_ignored: true,
                ..query
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(included.files.len(), 2);
    assert!(
        included
            .files
            .iter()
            .all(|file| !file.path.starts_with(".git"))
    );
}

#[test]
fn oversized_content_files_are_skipped_without_masking_results() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("match.txt"), "needle").unwrap();
    let oversized = fs::File::create(root.path().join("artifact.bin")).unwrap();
    oversized.set_len(8 * 1024 * 1024 + 1).unwrap();

    let results = SearchProvider::new(root.path())
        .search(
            &SearchQuery {
                text: "needle".into(),
                mode: SearchMode::Literal,
                case_sensitive: true,
                include_ignored: true,
            },
            &AtomicBool::new(false),
        )
        .unwrap();

    assert!(results.errors.is_empty());
    assert_eq!(results.files.len(), 1);
    assert_eq!(results.files[0].path, std::path::Path::new("match.txt"));
}

#[test]
fn background_content_search_publishes_root_files_before_descending() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("aaa")).unwrap();
    fs::write(root.path().join("aaa/nested.txt"), "needle").unwrap();
    fs::write(root.path().join("root.txt"), "needle").unwrap();

    let handle = SearchProvider::new(root.path())
        .start(SearchQuery {
            text: "needle".into(),
            mode: SearchMode::Literal,
            case_sensitive: true,
            include_ignored: true,
        })
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let first = loop {
        match handle.try_recv() {
            Ok(update) => break update,
            Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                std::thread::yield_now();
            }
            Err(error) => panic!("content search did not publish a result: {error}"),
        }
    };

    assert_eq!(
        first,
        SearchUpdate::Match(herdr_workbench::search::SearchFileResult {
            path: "root.txt".into(),
            matches: vec![herdr_workbench::search::SearchMatch {
                line: 1,
                snippet: "needle".into(),
            }],
        })
    );
    handle.cancel();
}

#[test]
fn invalid_regex_is_an_actionable_local_error() {
    let root = tempdir().unwrap();
    let error = match SearchProvider::new(root.path()).start(SearchQuery {
        text: "(".into(),
        mode: SearchMode::Regex,
        case_sensitive: false,
        include_ignored: false,
    }) {
        Ok(_) => panic!("invalid expression unexpectedly started"),
        Err(error) => error,
    };
    assert!(error.message.starts_with("invalid regular expression:"));
}

#[test]
fn pre_cancelled_search_never_walks_or_returns_raw_content() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("visible.txt"), "needle\x1b[31m").unwrap();
    let cancelled = AtomicBool::new(true);
    let results = SearchProvider::new(root.path())
        .search(
            &SearchQuery {
                text: "needle".into(),
                mode: SearchMode::Literal,
                case_sensitive: true,
                include_ignored: false,
            },
            &cancelled,
        )
        .unwrap();
    assert!(results.cancelled);
    assert!(results.files.is_empty());
}
