use herdr_workbench::{
    decoration::{GitCoordinates, GitState},
    icons::{EntryKind, IconMode},
    render::{RenderModel, RenderRow, View, render},
    theme::Palette,
};
use ratatui::{buffer::Buffer, layout::Rect};

fn model(mode: IconMode) -> RenderModel {
    RenderModel {
        view: View::Explorer,
        icon_mode: mode,
        rows: vec![
            RenderRow {
                name: "src".into(),
                path: "src".into(),
                kind: EntryKind::Directory,
                git: GitCoordinates {
                    index: GitState::Clean,
                    worktree: GitState::Modified,
                },
                expanded: true,
                depth: 0,
                selected: true,
                focused: true,
            },
            RenderRow {
                name: "a-long-ignored-file-name.rs".into(),
                path: "src/a-long-ignored-file-name.rs".into(),
                kind: EntryKind::File,
                git: GitCoordinates {
                    index: GitState::Ignored,
                    worktree: GitState::Clean,
                },
                expanded: false,
                depth: 1,
                selected: false,
                focused: false,
            },
            RenderRow {
                name: "lib.rs".into(),
                path: "src/lib.rs".into(),
                kind: EntryKind::File,
                git: GitCoordinates {
                    index: GitState::Added,
                    worktree: GitState::Modified,
                },
                expanded: false,
                depth: 1,
                selected: false,
                focused: false,
            },
        ],
        ..RenderModel::default()
    }
}

#[test]
fn visual_system_width_matrix_keeps_badges_and_sanitizes() {
    for width in [20, 24, 32, 48, 80] {
        let area = Rect::new(0, 0, width, 8);
        let mut buffer = Buffer::empty(area);
        render(
            &mut buffer,
            area,
            &model(IconMode::Plain),
            &Palette::catppuccin(),
        );
        let snapshot = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(buffer[(width - 1, 3)].symbol(), "M", "width {width}");
        assert!(!snapshot.contains("AM"), "width {width}");
        assert!(!snapshot.contains('\u{1b}'), "width {width}");
    }
}

#[test]
fn visual_system_themes_and_icon_modes_render_without_control_sequences() {
    for palette in [
        Palette::catppuccin(),
        Palette::catppuccin_latte(),
        Palette::terminal(),
    ] {
        for mode in [IconMode::Plain, IconMode::NerdFont] {
            let area = Rect::new(0, 0, 48, 8);
            let mut buffer = Buffer::empty(area);
            render(&mut buffer, area, &model(mode), &palette);
            assert!(
                !buffer
                    .content
                    .iter()
                    .flat_map(|cell| cell.symbol().chars())
                    .any(char::is_control)
            );
        }
    }
}

#[test]
fn visual_system_plain_32_snapshot() {
    let area = Rect::new(0, 0, 32, 8);
    let mut buffer = Buffer::empty(area);
    render(
        &mut buffer,
        area,
        &model(IconMode::Plain),
        &Palette::catppuccin(),
    );
    let lines = (0..area.height)
        .map(|row| {
            (0..area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(lines);
}
