use std::collections::BTreeMap;

use herdr_workbench::{
    decoration::{GitCoordinates, GitState, decorate_directory},
    highlight::{HighlightRole, HighlightedText},
    icons::{EntryKind, IconMode},
    preview::{PreviewContent, PreviewDocument, render_preview},
    render::{RenderModel, RenderRow, SearchScope, View, render},
    state::{GitContentMode, GitViewMode},
    theme::{Appearance, Palette, ThemeOverrides, resolve},
};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};

fn row(
    name: &str,
    kind: EntryKind,
    git: GitCoordinates,
    expanded: bool,
    depth: u16,
    selected: bool,
) -> RenderRow {
    RenderRow {
        name: name.to_string(),
        path: name.to_string(),
        kind,
        git,
        expanded,
        depth,
        selected,
        focused: selected,
    }
}

fn render_text(width: u16, height: u16, model: &RenderModel, palette: &Palette) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    render(&mut buffer, area, model, palette);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_preview_text(
    width: u16,
    height: u16,
    document: &PreviewDocument,
    palette: &Palette,
) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    render_preview(&mut buffer, area, document, 0, 0, palette);
    (0..height)
        .map(|row| {
            (0..width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn explorer_model(mode: IconMode) -> RenderModel {
    let rows = vec![
        row(
            "src",
            EntryKind::Directory,
            GitCoordinates {
                index: GitState::Clean,
                worktree: GitState::Modified,
            },
            true,
            0,
            false,
        ),
        row(
            "collapsed",
            EntryKind::Directory,
            GitCoordinates {
                index: GitState::Clean,
                worktree: GitState::Added,
            },
            false,
            0,
            false,
        ),
        row(
            "selected-dual-state.rs",
            EntryKind::File,
            GitCoordinates {
                index: GitState::Added,
                worktree: GitState::Modified,
            },
            false,
            1,
            true,
        ),
        row(
            ".ignored-with-a-long-name",
            EntryKind::File,
            GitCoordinates {
                index: GitState::Clean,
                worktree: GitState::Ignored,
            },
            false,
            1,
            false,
        ),
        row(
            "clean.toml",
            EntryKind::File,
            GitCoordinates::default(),
            false,
            0,
            false,
        ),
    ];
    RenderModel {
        rows,
        icon_mode: mode,
        ..RenderModel::default()
    }
}

#[test]
fn explorer_width_and_icon_snapshot_matrix() {
    let mut output = String::new();
    for mode in [IconMode::Plain, IconMode::NerdFont] {
        for width in [20, 24, 32, 48, 80] {
            output.push_str(&format!("\n== {mode:?} {width} ==\n"));
            output.push_str(&render_text(
                width,
                9,
                &explorer_model(mode),
                &Palette::catppuccin(),
            ));
            output.push('\n');
        }
    }
    insta::assert_snapshot!("visual_contract_explorer_widths_and_icons", output);
}

#[test]
fn directory_aggregation_semantic_snapshot_matrix() {
    let palette = Palette::catppuccin();
    let states = [
        ("clean", GitState::Clean),
        ("ignored", GitState::Ignored),
        ("renamed", GitState::Renamed),
        ("added", GitState::Added),
        ("modified", GitState::Modified),
        ("type changed", GitState::TypeChanged),
        ("deleted", GitState::Deleted),
        ("conflict", GitState::Conflict),
    ];
    let mut output = String::new();
    let mut rows = Vec::new();
    for (name, state) in states {
        let coordinates = GitCoordinates {
            index: state,
            worktree: GitState::Clean,
        };
        let decoration = decorate_directory(Color::Blue, coordinates.aggregate(), &palette);
        output.push_str(&format!(
            "{name}: name={:?} circle={:?} badge=none\n",
            decoration.name_color, decoration.directory_circle
        ));
        rows.push(row(
            name,
            EntryKind::Directory,
            coordinates,
            false,
            0,
            false,
        ));
    }
    output.push_str("\npriority mixed conflict+deleted => conflict\n");
    let mixed = GitCoordinates {
        index: GitState::Deleted,
        worktree: GitState::Conflict,
    };
    output.push_str(&format!("aggregate={:?}\n\n", mixed.aggregate()));
    output.push_str(&render_text(
        48,
        12,
        &RenderModel {
            rows,
            ..RenderModel::default()
        },
        &palette,
    ));
    insta::assert_snapshot!("visual_contract_directory_aggregation", output);
}

#[test]
fn search_state_snapshot_matrix() {
    let results = vec![
        row(
            "src/app.rs (2)",
            EntryKind::Directory,
            GitCoordinates::default(),
            true,
            0,
            false,
        ),
        row(
            "12: exact result",
            EntryKind::File,
            GitCoordinates::default(),
            false,
            1,
            true,
        ),
        row(
            "42: second result",
            EntryKind::File,
            GitCoordinates::default(),
            false,
            1,
            false,
        ),
    ];
    let cases = [
        (
            "entry",
            RenderModel {
                query: Some(String::new()),
                query_input_active: true,
                rows: vec![
                    row(
                        "src",
                        EntryKind::Directory,
                        GitCoordinates::default(),
                        false,
                        0,
                        false,
                    ),
                    row(
                        "Cargo.toml",
                        EntryKind::File,
                        GitCoordinates::default(),
                        false,
                        0,
                        true,
                    ),
                ],
                ..RenderModel::default()
            },
        ),
        (
            "active",
            RenderModel {
                query: Some("result".to_string()),
                query_input_active: true,
                search_scope: SearchScope::Contents,
                busy: true,
                search_busy: true,
                ..RenderModel::default()
            },
        ),
        (
            "results grouped by file",
            RenderModel {
                query: Some("result".to_string()),
                search_scope: SearchScope::Contents,
                search_case_sensitive: true,
                rows: results,
                ..RenderModel::default()
            },
        ),
        (
            "invalid expression",
            RenderModel {
                query: Some("(result".to_string()),
                query_input_active: true,
                search_scope: SearchScope::Contents,
                search_regex: true,
                error: Some("invalid regular expression: unclosed group".to_string()),
                ..RenderModel::default()
            },
        ),
        (
            "cancelled with prior results retained",
            RenderModel {
                query: Some("result".to_string()),
                search_scope: SearchScope::Contents,
                rows: vec![row(
                    "prior.rs (1)",
                    EntryKind::Directory,
                    GitCoordinates::default(),
                    true,
                    0,
                    true,
                )],
                ..RenderModel::default()
            },
        ),
    ];
    let output = cases
        .into_iter()
        .map(|(name, model)| {
            format!(
                "== {name} ==\n{}\n",
                render_text(48, 8, &model, &Palette::catppuccin())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!("visual_contract_search_states", output);
}

#[test]
fn source_control_groups_and_popup_snapshot_matrix() {
    let rows = vec![
        row(
            "Merge Changes (1)",
            EntryKind::Directory,
            GitCoordinates::default(),
            true,
            0,
            false,
        ),
        row(
            "conflict.rs",
            EntryKind::File,
            GitCoordinates {
                index: GitState::Conflict,
                worktree: GitState::Clean,
            },
            false,
            1,
            false,
        ),
        row(
            "Staged Changes (1)",
            EntryKind::Directory,
            GitCoordinates::default(),
            true,
            0,
            false,
        ),
        row(
            "dual.rs",
            EntryKind::File,
            GitCoordinates {
                index: GitState::Added,
                worktree: GitState::Clean,
            },
            false,
            1,
            true,
        ),
        row(
            "Changes (1)",
            EntryKind::Directory,
            GitCoordinates::default(),
            true,
            0,
            false,
        ),
        row(
            "dual.rs",
            EntryKind::File,
            GitCoordinates {
                index: GitState::Clean,
                worktree: GitState::Modified,
            },
            false,
            1,
            false,
        ),
        row(
            "Untracked (1)",
            EntryKind::Directory,
            GitCoordinates::default(),
            true,
            0,
            false,
        ),
        row(
            "new.rs",
            EntryKind::File,
            GitCoordinates {
                index: GitState::Clean,
                worktree: GitState::Untracked,
            },
            false,
            1,
            false,
        ),
    ];
    let group_model = RenderModel {
        view: View::SourceControl,
        rows,
        ..RenderModel::default()
    };
    let tree_model = RenderModel {
        view: View::SourceControl,
        git_view_mode: GitViewMode::Tree,
        rows: vec![
            row(
                "Staged Changes (1)",
                EntryKind::Directory,
                GitCoordinates::default(),
                true,
                0,
                false,
            ),
            row(
                "src",
                EntryKind::Directory,
                GitCoordinates {
                    index: GitState::Added,
                    worktree: GitState::Clean,
                },
                true,
                1,
                false,
            ),
            row(
                "dual.rs",
                EntryKind::File,
                GitCoordinates {
                    index: GitState::Added,
                    worktree: GitState::Clean,
                },
                false,
                2,
                true,
            ),
            row(
                "Changes (1)",
                EntryKind::Directory,
                GitCoordinates::default(),
                true,
                0,
                false,
            ),
            row(
                "src",
                EntryKind::Directory,
                GitCoordinates {
                    index: GitState::Clean,
                    worktree: GitState::Modified,
                },
                true,
                1,
                false,
            ),
            row(
                "dual.rs",
                EntryKind::File,
                GitCoordinates {
                    index: GitState::Clean,
                    worktree: GitState::Modified,
                },
                false,
                2,
                false,
            ),
        ],
        ..RenderModel::default()
    };
    let history_model = RenderModel {
        view: View::SourceControl,
        git_content_mode: GitContentMode::History,
        rows: vec![
            row(
                "d34db33 add local commit history · 2h",
                EntryKind::Commit,
                GitCoordinates::default(),
                false,
                0,
                true,
            ),
            row(
                "abc1234 fix Git refresh · 3d",
                EntryKind::Commit,
                GitCoordinates::default(),
                false,
                0,
                false,
            ),
        ],
        ..RenderModel::default()
    };
    let diff_document = PreviewDocument {
        title: "Changes · dual.rs".to_string(),
        content: PreviewContent::Text(
            HighlightedText::diff(
                std::path::Path::new("dual.rs"),
                &herdr_workbench::render::sanitize_multiline(
                    "diff --git a/dual.rs b/dual.rs\n@@ -1 +1 @@\n-old\n+new\u{1b}[31m",
                ),
            )
            .unwrap(),
        ),
        initial_line: 0,
        is_error: false,
        numbered_line_range: None,
    };
    let untracked_document = PreviewDocument {
        title: "Untracked · new.rs".to_string(),
        content: PreviewContent::Text(HighlightedText::plain(
            "untracked: new.rs\n   1 │ safe content",
            HighlightRole::Text,
        )),
        initial_line: 0,
        is_error: false,
        numbered_line_range: None,
    };
    let commit_document = PreviewDocument {
        title: "Commit d34db33".to_string(),
        content: PreviewContent::Text(
            HighlightedText::diff(
                std::path::Path::new("commit.diff"),
                "commit d34db33\nAuthor: Test\n\n    add local commit history\n\n src/git.rs | 2 ++\n@@ -1 +1,2 @@\n old\n+new",
            )
            .unwrap(),
        ),
        initial_line: 0,
        is_error: false,
        numbered_line_range: None,
    };
    let output = format!(
        "== flat groups and dual membership ==\n{}\n\n== tree groups and dual membership ==\n{}\n\n== history ==\n{}\n\n== diff ==\n{}\n\n== untracked ==\n{}\n\n== commit ==\n{}\n",
        render_text(48, 12, &group_model, &Palette::catppuccin()),
        render_text(48, 10, &tree_model, &Palette::catppuccin()),
        render_text(48, 6, &history_model, &Palette::catppuccin()),
        render_preview_text(48, 8, &diff_document, &Palette::catppuccin()),
        render_preview_text(48, 8, &untracked_document, &Palette::catppuccin()),
        render_preview_text(48, 10, &commit_document, &Palette::catppuccin())
    );
    assert!(!output.contains('\u{1b}'));
    insta::assert_snapshot!("visual_contract_source_control", output);
}

#[test]
fn help_and_actionable_error_snapshot_matrix() {
    let cases = [
        (
            "explorer help",
            RenderModel {
                help: true,
                ..RenderModel::default()
            },
        ),
        (
            "source control help",
            RenderModel {
                view: View::SourceControl,
                help: true,
                ..RenderModel::default()
            },
        ),
        (
            "source control history help",
            RenderModel {
                view: View::SourceControl,
                git_content_mode: GitContentMode::History,
                help: true,
                ..RenderModel::default()
            },
        ),
        (
            "non-Git workspace",
            RenderModel {
                view: View::SourceControl,
                git_available: false,
                notice: Some("No Git repository\nThis folder is not tracked by Git.".to_string()),
                ..RenderModel::default()
            },
        ),
        (
            "actionable runtime error",
            RenderModel {
                error: Some(
                    "git status failed; last valid status retained. Run git status in the root."
                        .to_string(),
                ),
                ..RenderModel::default()
            },
        ),
        (
            "config error",
            RenderModel {
                error: Some(
                    "invalid config /plugin/config.toml: width must be at least 24 columns"
                        .to_string(),
                ),
                ..RenderModel::default()
            },
        ),
        (
            "state recovery",
            RenderModel {
                error: Some(
                    "state file is corrupt; move it aside explicitly before reset".to_string(),
                ),
                ..RenderModel::default()
            },
        ),
    ];
    let output = cases
        .into_iter()
        .map(|(name, model)| {
            let height = if model.help { 22 } else { 10 };
            format!(
                "== {name} ==\n{}\n",
                render_text(48, height, &model, &Palette::catppuccin())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!("visual_contract_help_and_errors", output);
}

#[test]
fn every_theme_and_custom_override_snapshot_matrix() {
    let builtins = [
        "terminal",
        "catppuccin",
        "catppuccin-latte",
        "tokyo-night",
        "tokyo-night-day",
        "dracula",
        "nord",
        "gruvbox",
        "gruvbox-light",
        "one-dark",
        "one-light",
        "solarized",
        "solarized-light",
        "kanagawa",
        "kanagawa-lotus",
        "rose-pine",
        "rose-pine-dawn",
        "vesper",
    ];
    let mut output = String::new();
    for name in builtins {
        let palette = resolve(name, false, None, None, None, &ThemeOverrides::default()).palette;
        output.push_str(&format!(
            "{name}: bg={:?} text={:?} accent={:?} green={:?} yellow={:?} red={:?}\n",
            palette.panel_bg,
            palette.text,
            palette.accent,
            palette.green,
            palette.yellow,
            palette.red
        ));
        for mode in [IconMode::Plain, IconMode::NerdFont] {
            let rendered = render_text(32, 5, &explorer_model(mode), &palette);
            output.push_str(&format!(
                "  {mode:?}: {}\n",
                rendered.lines().next().unwrap_or_default()
            ));
        }
    }

    let mut colors = BTreeMap::new();
    colors.insert("accent".to_string(), Color::Rgb(1, 2, 3));
    colors.insert("panel_bg".to_string(), Color::Rgb(4, 5, 6));
    let overrides = ThemeOverrides { colors };
    for (label, appearance) in [
        ("custom-dark", Appearance::Dark),
        ("custom-light", Appearance::Light),
    ] {
        let resolved = resolve("catppuccin", true, None, None, Some(appearance), &overrides);
        output.push_str(&format!(
            "{label}: name={} bg={:?} accent={:?}\n",
            resolved.name, resolved.palette.panel_bg, resolved.palette.accent
        ));
    }
    let fallback = resolve(
        "catppuccin",
        true,
        None,
        None,
        None,
        &ThemeOverrides::default(),
    );
    output.push_str(&format!(
        "auto-unknown: name={} diagnostic={}\n",
        fallback.name,
        fallback.diagnostic.unwrap_or_default()
    ));
    insta::assert_snapshot!("visual_contract_all_themes", output);
}
