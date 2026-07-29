use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use herdr_workbench::config::{Config, ConfigError};
use herdr_workbench::file_tree::{FileTree, NodeKind, Preview};
use herdr_workbench::state::{DockSide, GitViewMode, PersistedState, SidebarView, StateError};
use herdr_workbench::workspace::WorkspaceRoot;
use tempfile::TempDir;

#[test]
fn config_is_strict_and_expands_only_an_explicit_argv_placeholder() {
    let fixture = TempDir::new().unwrap();
    let config_path = fixture.path().join("config.toml");
    fs::write(&config_path, "width = 32\nunknown = true\n").unwrap();
    assert!(matches!(
        Config::load(&config_path),
        Err(ConfigError::Parse { .. })
    ));

    fs::write(&config_path, "width = 32\ncontent_search_ignored = true\n").unwrap();
    assert!(matches!(
        Config::load(&config_path),
        Err(ConfigError::Parse { .. })
    ));

    fs::write(
        &config_path,
        "width = 32\n[open]\ncommand = [\"nvim\", \"{path}\"]\nplacement = \"tab\"\n",
    )
    .unwrap();
    let config = Config::load(&config_path).unwrap();
    assert_eq!(
        config.open_argv(Path::new("space name.rs")).unwrap(),
        Some(vec!["nvim".into(), "space name.rs".into()])
    );

    fs::write(
        &config_path,
        "[open]\ncommand = [\"nvim\", \"--file={path}\"]\n",
    )
    .unwrap();
    assert!(matches!(
        Config::load(&config_path),
        Err(ConfigError::Invalid { .. })
    ));
}

#[test]
fn configured_editor_wins_and_editor_environment_is_parsed_without_a_shell() {
    let configured: Config = toml::from_str(
        "[open]\ncommand = [\"nvim\", \"--clean\", \"{path}\"]\nplacement = \"tab\"\n",
    )
    .unwrap();
    assert_eq!(
        configured
            .open_argv_with_editor(
                Path::new("space name.rs"),
                Some(OsStr::new("ignored --flag"))
            )
            .unwrap(),
        Some(vec![
            "nvim".into(),
            "--clean".into(),
            "space name.rs".into()
        ])
    );

    let fallback = Config::default();
    assert_eq!(
        fallback
            .open_argv_with_editor(
                Path::new("space name.rs"),
                Some(OsStr::new("nvim --cmd 'set number'"))
            )
            .unwrap(),
        Some(vec![
            "nvim".into(),
            "--cmd".into(),
            "set number".into(),
            "space name.rs".into()
        ])
    );
    assert!(matches!(
        fallback.open_argv_with_editor(
            Path::new("space name.rs"),
            Some(OsStr::new("nvim 'unfinished"))
        ),
        Err(ConfigError::InvalidEditor(_))
    ));
    assert_eq!(
        fallback
            .open_argv_with_editor(Path::new("space name.rs"), None)
            .unwrap(),
        None
    );
}

#[test]
fn state_round_trips_atomically_and_corruption_requires_explicit_reset() {
    let fixture = TempDir::new().unwrap();
    let path = fixture.path().join("state.json");
    let mut state = PersistedState::default();
    let tab = state.tab_mut("/work/unicode-ß", "tab-1");
    tab.visible = true;
    tab.pane_id = Some("sidebar-pane".into());
    tab.dock_side = DockSide::Right;
    tab.view = SidebarView::SourceControl;
    tab.expanded.insert("src".into());
    tab.git_view_mode = GitViewMode::Tree;
    tab.git_tree_expanded.insert("changes\0src".into());
    tab.git_tree_initialized = true;
    tab.selection = Some("src/main.rs".into());
    tab.scroll = 9;
    state.save_atomic(&path).unwrap();
    assert_eq!(PersistedState::load(&path).unwrap(), state);

    fs::write(&path, b"not json").unwrap();
    assert!(matches!(
        PersistedState::load(&path),
        Err(StateError::Corrupt { .. })
    ));
    let reset = herdr_workbench::state::reset_corrupt_state(&path).unwrap();
    assert_eq!(PersistedState::load(&path).unwrap(), reset);
}

#[test]
fn root_bound_tree_is_lazy_sorted_and_sanitizes_preview() {
    let fixture = fixture();
    let long_line = format!("{}complete", "x".repeat(300));
    fs::write(fixture.path().join("long.txt"), &long_line).unwrap();
    let workspace = WorkspaceRoot::resolve(fixture.path()).unwrap();
    let mut tree = FileTree::new(workspace.clone(), true, false);
    assert_eq!(tree.visit_count(), 0);
    assert_eq!(tree.visible_nodes().len(), 1);
    tree.expand(Path::new("")).unwrap();
    let names: Vec<_> = tree
        .visible_nodes()
        .iter()
        .skip(1)
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(names[..3], ["adir", "Zoo", ".dotfile"]);
    assert!(names.contains(&"space name-ß.txt"));
    assert!(names.contains(&"control\\nname.txt"));
    let link = tree
        .visible_nodes()
        .into_iter()
        .find(|node| node.name == "outside-link")
        .unwrap();
    assert_eq!(link.kind, NodeKind::Symlink);
    assert!(link.symlink_target.is_some());
    assert!(workspace.resolve_path(Path::new("outside-link")).is_err());

    match tree.preview(Path::new("text.txt")).unwrap() {
        Preview::Text { rendered, .. } => {
            assert!(rendered.contains("   1  safe\\x1b[31mtext"));
        }
        Preview::Binary { .. } => panic!("text fixture was classified as binary"),
    }
    match tree.preview(Path::new("long.txt")).unwrap() {
        Preview::Text {
            source, truncated, ..
        } => {
            assert_eq!(source, long_line);
            assert!(!truncated);
            assert!(!source.contains('…'));
        }
        Preview::Binary { .. } => panic!("long text fixture was classified as binary"),
    }
    assert!(matches!(
        tree.preview(Path::new("binary.bin")).unwrap(),
        Preview::Binary { .. }
    ));
}

#[test]
fn initial_large_tree_render_does_not_visit_ten_thousand_files() {
    let fixture = TempDir::new().unwrap();
    for index in 0..10_000 {
        fs::write(fixture.path().join(format!("file-{index:05}")), "x").unwrap();
    }
    let workspace = WorkspaceRoot::resolve(fixture.path()).unwrap();
    let mut tree = FileTree::new(workspace, true, false);
    assert_eq!(tree.visit_count(), 0);
    assert_eq!(tree.visible_nodes().len(), 1);
    tree.expand(Path::new("")).unwrap();
    assert_eq!(tree.visit_count(), 10_000);
}

#[test]
fn root_git_directory_is_hidden_by_default_and_can_be_shown() {
    let fixture = TempDir::new().unwrap();
    fs::create_dir(fixture.path().join(".git")).unwrap();
    fs::create_dir(fixture.path().join("nested")).unwrap();
    fs::create_dir_all(fixture.path().join("nested/.git")).unwrap();
    let workspace = WorkspaceRoot::resolve(fixture.path()).unwrap();
    let mut tree = FileTree::new(workspace, true, false);

    tree.expand(Path::new("")).unwrap();
    assert!(
        tree.visible_nodes()
            .iter()
            .all(|node| node.path != Path::new(".git"))
    );

    tree.expand(Path::new("nested")).unwrap();
    assert!(
        tree.visible_nodes()
            .iter()
            .any(|node| node.path == Path::new("nested/.git"))
    );

    tree.set_show_git_directory(true);
    tree.refresh();
    assert!(
        tree.visible_nodes()
            .iter()
            .any(|node| node.path == Path::new(".git"))
    );
}

fn fixture() -> TempDir {
    let fixture = TempDir::new().unwrap();
    fs::create_dir(fixture.path().join("adir")).unwrap();
    fs::create_dir(fixture.path().join("Zoo")).unwrap();
    fs::write(fixture.path().join(".dotfile"), "dotfile").unwrap();
    fs::write(fixture.path().join("space name-ß.txt"), "unicode").unwrap();
    fs::write(fixture.path().join("control\nname.txt"), "control").unwrap();
    fs::write(fixture.path().join("text.txt"), "safe\x1b[31mtext\n").unwrap();
    fs::write(fixture.path().join("binary.bin"), [0, 1, 2]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/tmp", fixture.path().join("outside-link")).unwrap();
    fixture
}
