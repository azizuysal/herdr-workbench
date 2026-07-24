//! Presentation-only Git decoration rules. Parsing stays in `git`; this model
//! deliberately accepts compact status coordinates so it remains deterministic.

use crate::theme::Palette;
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitState {
    #[default]
    Clean,
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Ignored,
    Conflict,
}
impl GitState {
    pub fn priority(self) -> u8 {
        match self {
            Self::Conflict => 8,
            Self::Deleted => 7,
            Self::Modified | Self::TypeChanged => 6,
            Self::Added | Self::Untracked => 5,
            Self::Renamed | Self::Copied => 4,
            Self::Ignored => 3,
            Self::Clean => 0,
        }
    }
    pub fn badge(self) -> Option<char> {
        match self {
            Self::Conflict => Some('C'),
            Self::Deleted => Some('D'),
            Self::Modified => Some('M'),
            Self::Added => Some('A'),
            Self::Untracked => Some('U'),
            Self::Renamed => Some('R'),
            Self::Copied => Some('C'),
            Self::TypeChanged => Some('T'),
            Self::Ignored | Self::Clean => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type changed",
            Self::Untracked => "untracked",
            Self::Ignored => "ignored",
            Self::Conflict => "conflict",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GitCoordinates {
    pub index: GitState,
    pub worktree: GitState,
}
impl GitCoordinates {
    pub fn aggregate(self) -> GitState {
        if self.index.priority() >= self.worktree.priority() {
            self.index
        } else {
            self.worktree
        }
    }
    pub fn display_state(self) -> GitState {
        if self.index == GitState::Conflict || self.worktree == GitState::Conflict {
            GitState::Conflict
        } else if self.worktree != GitState::Clean {
            self.worktree
        } else {
            self.index
        }
    }
    pub fn badge(self) -> String {
        self.display_state()
            .badge()
            .map_or_else(String::new, |badge| badge.to_string())
    }
    pub fn description(self) -> String {
        let mut pieces = Vec::new();
        if self.index != GitState::Clean {
            pieces.push(format!("staged {}", self.index.label()));
        }
        if self.worktree != GitState::Clean {
            pieces.push(format!("working tree {}", self.worktree.label()));
        }
        if pieces.is_empty() {
            "clean".into()
        } else {
            pieces.join("; ")
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowDecoration {
    pub icon_color: Color,
    pub name_color: Color,
    pub badge_color: Color,
    pub directory_circle: Option<Color>,
    pub dim: bool,
}
pub fn semantic_color(state: GitState, p: &Palette) -> Color {
    match state {
        GitState::Conflict | GitState::Deleted => p.red,
        GitState::Modified => p.yellow,
        GitState::Added | GitState::Untracked => p.green,
        GitState::Renamed => p.blue,
        GitState::Copied => p.teal,
        GitState::TypeChanged => p.peach,
        GitState::Ignored => p.overlay0,
        GitState::Clean => p.text,
    }
}
pub fn decorate_file(icon_color: Color, status: GitCoordinates, p: &Palette) -> RowDecoration {
    let state = status.display_state();
    let color = semantic_color(state, p);
    RowDecoration {
        icon_color,
        name_color: color,
        badge_color: color,
        directory_circle: None,
        dim: state == GitState::Ignored,
    }
}
pub fn decorate_directory(icon_color: Color, aggregate: GitState, p: &Palette) -> RowDecoration {
    let muted = aggregate == GitState::Ignored;
    RowDecoration {
        icon_color: if muted { p.overlay0 } else { icon_color },
        name_color: if muted { p.overlay0 } else { p.text },
        badge_color: p.text,
        directory_circle: if matches!(aggregate, GitState::Clean | GitState::Ignored) {
            None
        } else {
            Some(semantic_color(aggregate, p))
        },
        dim: muted,
    }
}
/// Selection changes only background and focus adds an accent rail; semantic fg is retained.
pub fn row_style(decoration: RowDecoration, selected: bool, focused: bool, p: &Palette) -> Style {
    let mut style = Style::default().fg(decoration.name_color);
    if selected {
        style = style.bg(if focused { p.surface1 } else { p.surface0 });
    }
    if decoration.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}
pub fn aggregate_descendants(states: impl IntoIterator<Item = GitState>) -> GitState {
    states
        .into_iter()
        .max_by_key(|state| state.priority())
        .unwrap_or(GitState::Clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dual_state_uses_one_worktree_badge_without_losing_its_description() {
        let s = GitCoordinates {
            index: GitState::Added,
            worktree: GitState::Modified,
        };
        assert_eq!(s.badge(), "M");
        assert_eq!(s.description(), "staged added; working tree modified");
    }
    #[test]
    fn conflict_badge_overrides_other_coordinates() {
        let s = GitCoordinates {
            index: GitState::Conflict,
            worktree: GitState::Modified,
        };
        assert_eq!(s.badge(), "C");
    }
    #[test]
    fn changed_directory_has_circle_not_badge() {
        let d = decorate_directory(Color::Blue, GitState::Modified, &Palette::catppuccin());
        assert!(d.directory_circle.is_some());
    }
    #[test]
    fn ignored_directory_is_muted_without_circle() {
        let palette = Palette::catppuccin();
        let d = decorate_directory(Color::Blue, GitState::Ignored, &palette);
        assert_eq!(d.icon_color, palette.overlay0);
        assert_eq!(d.name_color, palette.overlay0);
        assert!(d.dim);
        assert_eq!(d.directory_circle, None);
    }
    #[test]
    fn precedence_matches_contract() {
        assert_eq!(
            aggregate_descendants([GitState::Ignored, GitState::Renamed, GitState::Deleted]),
            GitState::Deleted
        );
    }
}
