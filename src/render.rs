//! Pure Ratatui rendering and hit testing. Application state owns navigation;
//! this module receives a visible slice only, so rendering is bounded by rows.

use crate::{
    decoration::{GitCoordinates, decorate_directory, decorate_file, row_style},
    icons::{EntryKind, IconMode, file_color, icon_for},
    state::GitViewMode,
    theme::Palette,
};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget, Wrap},
};
use unicode_width::UnicodeWidthStr;

const BUSY_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Explorer,
    SourceControl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchScope {
    #[default]
    Files,
    Contents,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderRow {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub git: GitCoordinates,
    pub expanded: bool,
    pub depth: u16,
    pub selected: bool,
    pub focused: bool,
}
#[derive(Clone, Debug)]
pub struct RenderModel {
    pub view: View,
    pub git_available: bool,
    pub rows: Vec<RenderRow>,
    pub offset: usize,
    pub icon_mode: IconMode,
    pub query: Option<String>,
    pub query_input_active: bool,
    pub search_scope: SearchScope,
    pub search_case_sensitive: bool,
    pub search_regex: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub help: bool,
    pub busy: bool,
    pub search_busy: bool,
    pub busy_frame: usize,
    pub git_view_mode: GitViewMode,
}
impl Default for RenderModel {
    fn default() -> Self {
        Self {
            view: View::Explorer,
            git_available: true,
            rows: Vec::new(),
            offset: 0,
            icon_mode: IconMode::Plain,
            query: None,
            query_input_active: false,
            search_scope: SearchScope::Files,
            search_case_sensitive: false,
            search_regex: false,
            notice: None,
            error: None,
            help: false,
            busy: false,
            search_busy: false,
            busy_frame: 0,
            git_view_mode: GitViewMode::Flat,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HitTargets {
    pub explorer: Option<Rect>,
    pub source_control: Option<Rect>,
    pub git_view_toggle: Option<Rect>,
    pub close: Option<Rect>,
    pub query_cursor: Option<(u16, u16)>,
    pub search_input: Option<Rect>,
    pub search_files: Option<Rect>,
    pub search_contents: Option<Rect>,
    pub search_case: Option<Rect>,
    pub search_regex: Option<Rect>,
    pub rows: Vec<(usize, Rect)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchControl {
    Input,
    Files,
    Contents,
    CaseSensitive,
    Regex,
}

impl HitTargets {
    pub fn row_at(&self, col: u16, row: u16) -> Option<usize> {
        self.rows
            .iter()
            .find(|(_, r)| r.x <= col && col < r.right() && r.y <= row && row < r.bottom())
            .map(|(i, _)| *i)
    }
    pub fn view_at(&self, col: u16, row: u16) -> Option<View> {
        let in_rect = |r: Option<Rect>| {
            r.is_some_and(|r| r.x <= col && col < r.right() && r.y <= row && row < r.bottom())
        };
        if in_rect(self.explorer) {
            Some(View::Explorer)
        } else if in_rect(self.source_control) {
            Some(View::SourceControl)
        } else {
            None
        }
    }

    pub fn search_control_at(&self, col: u16, row: u16) -> Option<SearchControl> {
        let contains = |area: Option<Rect>| {
            area.is_some_and(|area| {
                area.x <= col && col < area.right() && area.y <= row && row < area.bottom()
            })
        };
        if contains(self.search_input) {
            Some(SearchControl::Input)
        } else if contains(self.search_files) {
            Some(SearchControl::Files)
        } else if contains(self.search_contents) {
            Some(SearchControl::Contents)
        } else if contains(self.search_case) {
            Some(SearchControl::CaseSensitive)
        } else if contains(self.search_regex) {
            Some(SearchControl::Regex)
        } else {
            None
        }
    }

    pub fn git_view_toggle_at(&self, col: u16, row: u16) -> bool {
        self.git_view_toggle.is_some_and(|area| {
            area.x <= col && col < area.right() && area.y <= row && row < area.bottom()
        })
    }
}

pub fn render(
    buffer: &mut Buffer,
    area: Rect,
    model: &RenderModel,
    palette: &Palette,
) -> HitTargets {
    Block::default()
        .style(Style::default().bg(palette.panel_bg))
        .render(area, buffer);
    if area.width == 0 || area.height == 0 {
        return HitTargets::default();
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let mut targets = render_tabs(buffer, rows[0], model, palette);
    let content = render_content(buffer, rows[1], model, palette);
    targets.rows = content.rows;
    targets.query_cursor = content.query_cursor;
    targets.search_input = content.search.input;
    targets.search_files = content.search.files;
    targets.search_contents = content.search.contents;
    targets.search_case = content.search.case_sensitive;
    targets.search_regex = content.search.regex;
    targets
}

/// A `Widget` adapter for normal Ratatui `Frame::render_widget` integration.
/// Hit targets are returned by the buffer-level renderer for input dispatch.
pub struct Sidebar<'a> {
    pub model: &'a RenderModel,
    pub palette: &'a Palette,
}

impl Widget for Sidebar<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let _ = render(buffer, area, self.model, self.palette);
    }
}
fn render_tabs(buffer: &mut Buffer, area: Rect, m: &RenderModel, p: &Palette) -> HitTargets {
    Block::default()
        .style(Style::default().bg(p.surface_dim))
        .render(area, buffer);
    let close_width = area.width.min(2);
    let close = Rect::new(
        area.right().saturating_sub(close_width),
        area.y,
        close_width,
        1,
    );
    let git_view_width = if m.view == View::SourceControl {
        area.width.min(3)
    } else {
        0
    };
    let busy_width = u16::from(m.busy && !m.search_busy);
    let available = area
        .width
        .saturating_sub(close_width)
        .saturating_sub(git_view_width)
        .saturating_sub(busy_width);
    let tab_width = if available >= 10 {
        5
    } else if available >= 6 {
        3
    } else {
        1
    };
    let views = [View::Explorer, View::SourceControl];
    let mut targets = HitTargets::default();
    let mut next_x = area.x;
    for view in views {
        let remaining = area.x.saturating_add(available).saturating_sub(next_x);
        if remaining == 0 {
            break;
        }
        let width = tab_width.min(remaining);
        let tab = Rect::new(next_x, area.y, width, 1);
        next_x = next_x.saturating_add(width);
        let active = view == m.view;
        let icon = match (view, m.icon_mode) {
            (View::Explorer, IconMode::NerdFont) => "",
            (View::SourceControl, IconMode::NerdFont) => "",
            (View::Explorer, IconMode::Plain) => "E",
            (View::SourceControl, IconMode::Plain) => "G",
        };
        let icon = Span::styled(icon, Style::default().add_modifier(Modifier::BOLD));
        let foreground = if active && (view != View::SourceControl || m.git_available) {
            p.accent
        } else {
            p.overlay0
        };
        Paragraph::new(Line::from(icon))
            .alignment(Alignment::Center)
            .style(Style::default().fg(foreground).bg(if active {
                p.surface0
            } else {
                p.surface_dim
            }))
            .render(tab, buffer);
        match view {
            View::Explorer => targets.explorer = Some(tab),
            View::SourceControl => targets.source_control = Some(tab),
        }
    }
    let mut control_x = close.x;
    if git_view_width > 0 {
        control_x = control_x.saturating_sub(git_view_width);
        let toggle = Rect::new(control_x, area.y, git_view_width, 1);
        let icon = match (m.git_view_mode, m.icon_mode) {
            (GitViewMode::Flat, IconMode::NerdFont) => "",
            (GitViewMode::Tree, IconMode::NerdFont) => "",
            (GitViewMode::Flat, IconMode::Plain) => "L",
            (GitViewMode::Tree, IconMode::Plain) => "T",
        };
        Paragraph::new(icon)
            .alignment(Alignment::Center)
            .style(Style::default().fg(p.overlay1).bg(p.surface_dim))
            .render(toggle, buffer);
        targets.git_view_toggle = Some(toggle);
    }
    if busy_width > 0 && control_x > next_x {
        control_x = control_x.saturating_sub(1);
        Paragraph::new(BUSY_FRAMES[m.busy_frame % BUSY_FRAMES.len()])
            .style(Style::default().fg(p.yellow).bg(p.surface_dim))
            .render(Rect::new(control_x, area.y, 1, 1), buffer);
    }
    Paragraph::new(if close_width == 1 { "×" } else { " ×" })
        .style(Style::default().fg(p.overlay0).bg(p.surface_dim))
        .render(close, buffer);
    targets.close = Some(close);
    targets
}
struct RenderedContent {
    rows: Vec<(usize, Rect)>,
    query_cursor: Option<(u16, u16)>,
    search: SearchTargets,
}

#[derive(Default)]
struct SearchTargets {
    input: Option<Rect>,
    files: Option<Rect>,
    contents: Option<Rect>,
    case_sensitive: Option<Rect>,
    regex: Option<Rect>,
}

fn render_content(
    buffer: &mut Buffer,
    area: Rect,
    m: &RenderModel,
    p: &Palette,
) -> RenderedContent {
    let mut content_area = area;
    let mut query_cursor = None;
    let mut search = SearchTargets::default();
    if let Some(query) = &m.query
        && content_area.height > 0
    {
        let query_area = Rect::new(content_area.x, content_area.y, content_area.width, 1);
        search.input = Some(query_area);
        let icon = match m.icon_mode {
            IconMode::NerdFont => "",
            IconMode::Plain => "/",
        };
        let background = if m.query_input_active {
            p.surface1
        } else {
            p.surface0
        };
        Block::default()
            .style(Style::default().bg(background))
            .render(query_area, buffer);
        let icon_width = query_area.width.min(2);
        Paragraph::new(format!("{icon} "))
            .style(
                Style::default()
                    .fg(if m.query_input_active {
                        p.accent
                    } else {
                        p.overlay0
                    })
                    .bg(background),
            )
            .render(Rect::new(query_area.x, query_area.y, icon_width, 1), buffer);
        let spinner_width = u16::from(m.search_busy && query_area.width > icon_width);
        let input_area = Rect::new(
            query_area.x.saturating_add(icon_width),
            query_area.y,
            query_area
                .width
                .saturating_sub(icon_width)
                .saturating_sub(spinner_width),
            1,
        );
        if input_area.width > 0 {
            let query = sanitize(query);
            if query.is_empty() {
                Paragraph::new(match m.search_scope {
                    SearchScope::Files => "Filter files…",
                    SearchScope::Contents => "Search contents…",
                })
                .style(Style::default().fg(p.overlay0).bg(background))
                .render(input_area, buffer);
            } else {
                let query_width = UnicodeWidthStr::width(query.as_str());
                let cursor_limit = usize::from(input_area.width.saturating_sub(1));
                let scroll = query_width.saturating_sub(cursor_limit);
                Paragraph::new(query)
                    .style(Style::default().fg(p.text).bg(background))
                    .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                    .render(input_area, buffer);
                if m.query_input_active {
                    query_cursor = Some((
                        input_area
                            .x
                            .saturating_add(u16::try_from(query_width.min(cursor_limit)).unwrap()),
                        input_area.y,
                    ));
                }
            }
            if m.query_input_active && query_cursor.is_none() {
                query_cursor = Some((input_area.x, input_area.y));
            }
        }
        if spinner_width > 0 {
            Paragraph::new(BUSY_FRAMES[m.busy_frame % BUSY_FRAMES.len()])
                .style(Style::default().fg(p.yellow).bg(background))
                .render(
                    Rect::new(query_area.right().saturating_sub(1), query_area.y, 1, 1),
                    buffer,
                );
        }

        if content_area.height > 1 {
            let toolbar_area = Rect::new(
                content_area.x,
                content_area.y.saturating_add(1),
                content_area.width,
                1,
            );
            Block::default()
                .style(Style::default().bg(p.panel_bg))
                .render(toolbar_area, buffer);
            let wide_labels = toolbar_area.width >= 24;
            let mut x = toolbar_area.x;
            search.files = render_search_control(
                buffer,
                toolbar_area,
                &mut x,
                if wide_labels { "Files" } else { "F" },
                m.search_scope == SearchScope::Files,
                p,
            );
            search.contents = render_search_control(
                buffer,
                toolbar_area,
                &mut x,
                if wide_labels { "Contents" } else { "C" },
                m.search_scope == SearchScope::Contents,
                p,
            );
            let mut right = toolbar_area.right();
            search.regex = render_search_control_right(
                buffer,
                toolbar_area,
                x,
                &mut right,
                ".*",
                m.search_regex,
                p,
            );
            search.case_sensitive = render_search_control_right(
                buffer,
                toolbar_area,
                x,
                &mut right,
                "Aa",
                m.search_case_sensitive,
                p,
            );
        }
        content_area.y = content_area.y.saturating_add(2);
        content_area.height = content_area.height.saturating_sub(2);
    }
    if let Some(error) = &m.error {
        Paragraph::new(sanitize_multiline(error))
            .style(Style::default().fg(p.red).bg(p.panel_bg))
            .wrap(Wrap { trim: false })
            .render(content_area, buffer);
        return RenderedContent {
            rows: Vec::new(),
            query_cursor,
            search,
        };
    }
    if m.help {
        Paragraph::new(help_text(m.view))
            .style(Style::default().fg(p.text))
            .render(content_area, buffer);
        return RenderedContent {
            rows: Vec::new(),
            query_cursor,
            search,
        };
    }
    if let Some(notice) = &m.notice {
        Paragraph::new(sanitize_multiline(notice))
            .style(Style::default().fg(p.overlay0).bg(p.panel_bg))
            .wrap(Wrap { trim: false })
            .render(content_area, buffer);
        return RenderedContent {
            rows: Vec::new(),
            query_cursor,
            search,
        };
    }
    let mut hits = Vec::new();
    let max = usize::from(content_area.height);
    for (slot, (index, row)) in m
        .rows
        .iter()
        .enumerate()
        .skip(m.offset)
        .take(max)
        .enumerate()
    {
        let r = Rect::new(
            content_area.x,
            content_area.y + slot as u16,
            content_area.width,
            1,
        );
        hits.push((index, r));
        render_row(buffer, r, row, m.icon_mode, p)
    }
    if m.rows.is_empty() {
        let message = if m.query.is_some() {
            "No results"
        } else {
            "No entries"
        };
        Paragraph::new(message)
            .style(Style::default().fg(p.overlay0))
            .render(content_area, buffer)
    }
    RenderedContent {
        rows: hits,
        query_cursor,
        search,
    }
}

fn render_search_control(
    buffer: &mut Buffer,
    toolbar_area: Rect,
    x: &mut u16,
    label: &str,
    active: bool,
    p: &Palette,
) -> Option<Rect> {
    if *x >= toolbar_area.right() {
        return None;
    }
    let width = u16::try_from(UnicodeWidthStr::width(label))
        .unwrap_or(u16::MAX)
        .min(toolbar_area.right().saturating_sub(*x));
    if width == 0 {
        return None;
    }
    let area = Rect::new(*x, toolbar_area.y, width, 1);
    let style = if active {
        Style::default()
            .fg(p.accent)
            .bg(p.surface1)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0).bg(p.panel_bg)
    };
    Paragraph::new(label).style(style).render(area, buffer);
    *x = area.right().saturating_add(1);
    Some(area)
}

fn render_search_control_right(
    buffer: &mut Buffer,
    toolbar_area: Rect,
    minimum_x: u16,
    right: &mut u16,
    label: &str,
    active: bool,
    p: &Palette,
) -> Option<Rect> {
    if *right <= minimum_x {
        return None;
    }
    let width = u16::try_from(UnicodeWidthStr::width(label))
        .unwrap_or(u16::MAX)
        .min(right.saturating_sub(minimum_x));
    if width == 0 {
        return None;
    }
    let x = right.saturating_sub(width);
    let area = Rect::new(x, toolbar_area.y, width, 1);
    let style = if active {
        Style::default()
            .fg(p.accent)
            .bg(p.surface1)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0).bg(p.panel_bg)
    };
    Paragraph::new(label).style(style).render(area, buffer);
    *right = x.saturating_sub(1).max(minimum_x);
    Some(area)
}
fn render_row(buffer: &mut Buffer, area: Rect, row: &RenderRow, mode: IconMode, p: &Palette) {
    let icon = icon_for(&row.name, row.kind);
    let decoration = if row.kind == EntryKind::Directory {
        decorate_directory(file_color(icon, p), row.git.aggregate(), p)
    } else {
        decorate_file(file_color(icon, p), row.git, p)
    };
    let style = row_style(decoration, row.selected, row.focused, p);
    let badge = if row.kind == EntryKind::Directory {
        if decoration.directory_circle.is_some() {
            "●".to_string()
        } else {
            String::new()
        }
    } else {
        row.git.badge()
    };
    let indent = " ".repeat(usize::from(row.depth.saturating_mul(2)));
    let marker = if row.kind == EntryKind::Directory {
        if row.expanded { "⌄" } else { "›" }
    } else if row.focused {
        "│"
    } else {
        " "
    };
    let badge_width = UnicodeDisplayWidth(&badge).value();
    let fixed = indent.len() + 4 + badge_width;
    let width = usize::from(area.width).saturating_sub(fixed);
    let name = truncate(&sanitize(&row.name), width.max(1));
    let mut spans = vec![
        Span::styled(indent, style),
        Span::styled(
            marker,
            style.fg(if row.focused { p.accent } else { p.overlay0 }),
        ),
        Span::raw(" "),
        Span::styled(
            icon.glyph(mode),
            Style::default()
                .fg(decoration.icon_color)
                .bg(if row.selected {
                    if row.focused { p.surface1 } else { p.surface0 }
                } else {
                    p.panel_bg
                }),
        ),
        Span::raw(" "),
        Span::styled(name, style),
    ];
    let padding =
        usize::from(area.width).saturating_sub(fixed + UnicodeDisplayWidth(&row.name).value());
    if !badge.is_empty() {
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(
            badge,
            Style::default()
                .fg(if row.kind == EntryKind::Directory {
                    decoration.directory_circle.unwrap_or(p.text)
                } else {
                    decoration.badge_color
                })
                .bg(if row.selected {
                    if row.focused { p.surface1 } else { p.surface0 }
                } else {
                    p.panel_bg
                }),
        ));
    }
    Paragraph::new(Line::from(spans))
        .style(Style::default().bg(if row.selected {
            if row.focused { p.surface1 } else { p.surface0 }
        } else {
            p.panel_bg
        }))
        .render(area, buffer)
}
struct UnicodeDisplayWidth<'a>(&'a str);
impl UnicodeDisplayWidth<'_> {
    fn value(&self) -> usize {
        unicode_width::UnicodeWidthStr::width(self.0)
    }
}
pub fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() {
                if c == '\n' { ' ' } else { '�' }
            } else {
                c
            }
        })
        .collect()
}

pub fn sanitize_multiline(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\n' => "\n".chars().collect::<Vec<_>>(),
            '\t' => "    ".chars().collect(),
            '\r' => Vec::new(),
            value if value.is_control() => "�".chars().collect(),
            value => vec![value],
        })
        .collect()
}

fn help_text(view: View) -> &'static str {
    match view {
        View::Explorer => {
            "Explorer\n↑/↓, j/k  move\n←/→, h/l  fold/open\nEnter/Space  preview\no  external edit\nf  file manager\ny  copy path\n/  search\ni  ignored entries\nr  refresh\nd  switch dock side\n1/2, Tab  change view\nq  close sidebar\n? / Esc  close help\n\nSearch\nTab/Shift+Tab  scope\nAlt+C  case sensitive\nAlt+R  regular expression\nEnter  navigate results\nEsc  return to Explorer"
        }
        View::SourceControl => {
            "Source Control\n↑/↓, j/k  move\n←/→, h/l  fold/open\nEnter  fold/preview\nv  tree/list view\nf  file manager\ny  copy path\nr  refresh\nd  switch dock side\n1/2, Tab  change view\nq  close sidebar\n? / Esc  close help"
        }
    }
}
pub fn truncate(value: &str, width: usize) -> String {
    if unicode_width::UnicodeWidthStr::width(value) <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let mut output = String::new();
    for c in value.chars() {
        if unicode_width::UnicodeWidthStr::width(output.as_str())
            + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
            > width - 1
        {
            break;
        }
        output.push(c)
    }
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoration::GitState;
    use ratatui::buffer::Buffer;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn every_width_renders_and_badges_survive() {
        for width in [20, 24, 32, 48, 80] {
            let area = Rect::new(0, 0, width, 8);
            let mut b = Buffer::empty(area);
            let mut m = RenderModel::default();
            m.rows.push(RenderRow {
                name: "very-long-file-name.rs".into(),
                path: "x".into(),
                kind: EntryKind::File,
                git: GitCoordinates {
                    index: GitState::Added,
                    worktree: GitState::Modified,
                },
                expanded: false,
                depth: 0,
                selected: true,
                focused: true,
            });
            render(&mut b, area, &m, &Palette::catppuccin());
            assert_eq!(b[(width - 1, 1)].symbol(), "M");
        }
    }

    #[test]
    fn directory_circles_align_with_file_badges_at_the_right_edge() {
        assert_eq!(UnicodeWidthStr::width("●"), 1);
        let area = Rect::new(0, 0, 32, 4);
        let mut buffer = Buffer::empty(area);
        let status = GitCoordinates {
            index: GitState::Clean,
            worktree: GitState::Modified,
        };
        let model = RenderModel {
            rows: vec![
                RenderRow {
                    name: "directory".into(),
                    path: "directory".into(),
                    kind: EntryKind::Directory,
                    git: status,
                    expanded: false,
                    depth: 0,
                    selected: false,
                    focused: false,
                },
                RenderRow {
                    name: "file.rs".into(),
                    path: "file.rs".into(),
                    kind: EntryKind::File,
                    git: status,
                    expanded: false,
                    depth: 0,
                    selected: false,
                    focused: false,
                },
            ],
            ..RenderModel::default()
        };

        render(&mut buffer, area, &model, &Palette::catppuccin());

        assert_eq!(buffer[(area.width - 1, 1)].symbol(), "●");
        assert_eq!(buffer[(area.width - 1, 2)].symbol(), "M");
    }

    #[test]
    fn git_view_control_stays_separate_from_tabs_close_and_file_badges() {
        assert_eq!(UnicodeWidthStr::width(""), 1);
        assert_eq!(UnicodeWidthStr::width(""), 1);
        for width in [20, 24, 32, 48, 80] {
            let area = Rect::new(0, 0, width, 4);
            let mut buffer = Buffer::empty(area);
            let model = RenderModel {
                view: View::SourceControl,
                icon_mode: IconMode::NerdFont,
                rows: vec![RenderRow {
                    name: "nested/changed.rs".into(),
                    path: "nested/changed.rs".into(),
                    kind: EntryKind::File,
                    git: GitCoordinates {
                        index: GitState::Clean,
                        worktree: GitState::Modified,
                    },
                    expanded: false,
                    depth: 1,
                    selected: true,
                    focused: true,
                }],
                ..RenderModel::default()
            };

            let targets = render(&mut buffer, area, &model, &Palette::catppuccin());
            let source = targets.source_control.expect("Source Control tab");
            let toggle = targets.git_view_toggle.expect("Git view control");
            let close = targets.close.expect("close control");
            assert!(source.right() <= toggle.x);
            assert!(toggle.right() <= close.x);
            assert_eq!(buffer[(width - 1, 1)].symbol(), "M");
        }
    }

    #[test]
    fn codicon_tabs_are_wide_icon_only_and_share_the_top_row() {
        let area = Rect::new(0, 0, 32, 6);
        let mut buffer = Buffer::empty(area);
        let mut model = RenderModel {
            view: View::SourceControl,
            icon_mode: IconMode::NerdFont,
            ..RenderModel::default()
        };
        model.rows.push(RenderRow {
            name: "result.rs".into(),
            path: "result.rs".into(),
            kind: EntryKind::File,
            git: GitCoordinates::default(),
            expanded: false,
            depth: 0,
            selected: true,
            focused: true,
        });

        let palette = Palette::catppuccin();
        let targets = render(&mut buffer, area, &model, &palette);

        let explorer = targets.explorer.expect("Explorer tab");
        let source_control = targets.source_control.expect("Source Control tab");
        let git_view_toggle = targets.git_view_toggle.expect("Git view toggle");
        assert_eq!(explorer.y, 0);
        assert_eq!(source_control.y, 0);
        assert!(explorer.x < source_control.x);
        assert_eq!(explorer.width, 5);
        assert_eq!(source_control.width, 5);
        assert_eq!(targets.rows[0].1, Rect::new(0, 1, 32, 1));
        let explorer_icon_x = explorer.x + explorer.width / 2;
        let source_control_icon_x = source_control.x + source_control.width / 2;
        assert_eq!(buffer[(explorer_icon_x, 0)].fg, palette.overlay0);
        assert_eq!(buffer[(explorer_icon_x, 0)].bg, palette.surface_dim);
        assert_eq!(buffer[(source_control_icon_x, 0)].fg, palette.accent);
        assert_eq!(buffer[(source_control_icon_x, 0)].bg, palette.surface0);
        assert!(
            buffer[(source_control_icon_x, 0)]
                .modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            !buffer[(source_control.x, 0)]
                .modifier
                .contains(Modifier::BOLD)
        );
        let top_row = (0..area.width)
            .map(|column| buffer[(column, 0)].symbol())
            .collect::<String>();
        assert!(top_row.contains(''));
        assert!(top_row.contains(''));
        assert!(top_row.contains(''));
        assert!(!top_row.chars().any(|character| character.is_ascii_digit()));
        assert!(!top_row.contains('+'));
        assert!(!top_row.contains("Explorer"));
        assert!(!top_row.contains("Search"));
        assert!(!top_row.contains("Source Control"));
        assert!(targets.git_view_toggle_at(git_view_toggle.x, git_view_toggle.y));

        model.git_view_mode = GitViewMode::Tree;
        let mut tree_buffer = Buffer::empty(area);
        let tree_targets = render(&mut tree_buffer, area, &model, &palette);
        let tree_toggle = tree_targets.git_view_toggle.expect("tree view toggle");
        assert_eq!(
            tree_buffer[(tree_toggle.x + tree_toggle.width / 2, tree_toggle.y)].symbol(),
            ""
        );
    }

    #[test]
    fn non_search_busy_indicator_uses_pinned_dots_rotation_in_header() {
        let area = Rect::new(0, 0, 32, 4);
        let palette = Palette::catppuccin();
        for (frame, symbol) in BUSY_FRAMES.iter().enumerate() {
            let mut buffer = Buffer::empty(area);
            let model = RenderModel {
                busy: true,
                busy_frame: frame,
                ..RenderModel::default()
            };

            let targets = render(&mut buffer, area, &model, &palette);
            let close = targets.close.expect("close control");
            let spinner = &buffer[(close.x - 1, close.y)];
            assert_eq!(spinner.symbol(), *symbol);
            assert_eq!(spinner.fg, palette.yellow);
            assert_eq!(buffer[(close.x + 1, close.y)].symbol(), "×");
            assert!(targets.source_control.expect("source control tab").right() < close.x);
        }
    }

    #[test]
    fn search_busy_indicator_is_right_aligned_inside_query_without_overlapping_cursor() {
        let area = Rect::new(0, 0, 32, 4);
        let palette = Palette::catppuccin();
        for (frame, symbol) in BUSY_FRAMES.iter().enumerate() {
            let mut buffer = Buffer::empty(area);
            let model = RenderModel {
                query: Some("a-query-long-enough-to-scroll".to_string()),
                query_input_active: true,
                busy: true,
                search_busy: true,
                busy_frame: frame,
                ..RenderModel::default()
            };

            let targets = render(&mut buffer, area, &model, &palette);
            let input = targets.search_input.expect("search input");
            let spinner_x = input.right() - 1;
            let spinner = &buffer[(spinner_x, input.y)];
            assert_eq!(spinner.symbol(), *symbol);
            assert_eq!(spinner.fg, palette.yellow);
            assert_eq!(spinner.bg, palette.surface1);
            assert_eq!(targets.query_cursor, Some((spinner_x - 1, input.y)));

            let close = targets.close.expect("close control");
            assert_ne!(buffer[(close.x - 1, close.y)].symbol(), *symbol);
            assert_eq!(buffer[(close.x + 1, close.y)].symbol(), "×");
        }
    }

    #[test]
    fn content_uses_every_row_below_the_tabs() {
        let area = Rect::new(0, 0, 32, 4);
        let mut buffer = Buffer::empty(area);
        let model = RenderModel {
            rows: (0..3)
                .map(|index| RenderRow {
                    name: format!("file-{index}.rs"),
                    path: format!("file-{index}.rs"),
                    kind: EntryKind::File,
                    git: GitCoordinates::default(),
                    expanded: false,
                    depth: 0,
                    selected: index == 0,
                    focused: index == 0,
                })
                .collect(),
            ..RenderModel::default()
        };

        let targets = render(&mut buffer, area, &model, &Palette::catppuccin());

        assert_eq!(targets.rows.len(), 3);
        assert_eq!(targets.rows[2].1, Rect::new(0, 3, 32, 1));
        assert_eq!(buffer[(4, 3)].symbol(), "f");
    }

    #[test]
    fn makefile_icon_uses_text_contrast_on_dark_and_light_palettes() {
        let area = Rect::new(0, 0, 32, 3);
        let model = RenderModel {
            icon_mode: IconMode::NerdFont,
            rows: vec![RenderRow {
                name: "Makefile".into(),
                path: "Makefile".into(),
                kind: EntryKind::File,
                git: GitCoordinates::default(),
                expanded: false,
                depth: 0,
                selected: false,
                focused: false,
            }],
            ..RenderModel::default()
        };

        for palette in [Palette::catppuccin(), Palette::catppuccin_latte()] {
            let mut buffer = Buffer::empty(area);
            render(&mut buffer, area, &model, &palette);
            assert_eq!(buffer[(2, 1)].symbol(), "");
            assert_eq!(buffer[(2, 1)].fg, palette.text);
        }
    }

    #[test]
    fn unavailable_source_control_tab_and_notice_are_muted() {
        let area = Rect::new(0, 0, 32, 6);
        let mut buffer = Buffer::empty(area);
        let model = RenderModel {
            view: View::SourceControl,
            git_available: false,
            icon_mode: IconMode::NerdFont,
            notice: Some("No Git repository\nThis folder is not tracked by Git.".to_string()),
            ..RenderModel::default()
        };
        let palette = Palette::catppuccin();

        let targets = render(&mut buffer, area, &model, &palette);
        let source_control = targets.source_control.expect("Source Control tab");

        assert_eq!(buffer[(source_control.x + 1, 0)].fg, palette.overlay0);
        assert_eq!(buffer[(0, 1)].fg, palette.overlay0);
        assert_eq!(buffer[(0, 1)].symbol(), "N");
    }

    #[test]
    fn inline_search_query_uses_the_search_codicon_and_offsets_result_hits() {
        let area = Rect::new(0, 0, 32, 6);
        let mut buffer = Buffer::empty(area);
        let model = RenderModel {
            icon_mode: IconMode::NerdFont,
            query: Some("needle".to_string()),
            query_input_active: true,
            rows: vec![RenderRow {
                name: "result.rs".into(),
                path: "result.rs".into(),
                kind: EntryKind::File,
                git: GitCoordinates::default(),
                expanded: false,
                depth: 0,
                selected: true,
                focused: true,
            }],
            ..RenderModel::default()
        };

        let targets = render(&mut buffer, area, &model, &Palette::catppuccin());

        assert_eq!(buffer[(0, 1)].symbol(), "");
        assert_eq!(targets.query_cursor, Some((8, 1)));
        assert_eq!(targets.rows[0].1, Rect::new(0, 3, 32, 1));
        assert_eq!(unicode_width::UnicodeWidthStr::width(""), 1);
        assert_eq!(unicode_width::UnicodeWidthStr::width(""), 1);
        assert_eq!(unicode_width::UnicodeWidthStr::width(""), 1);
    }

    #[test]
    fn empty_active_search_has_placeholder_accent_and_cursor() {
        let area = Rect::new(0, 0, 32, 4);
        let mut buffer = Buffer::empty(area);
        let model = RenderModel {
            query: Some(String::new()),
            query_input_active: true,
            ..RenderModel::default()
        };
        let palette = Palette::catppuccin();

        let targets = render(&mut buffer, area, &model, &palette);
        let row = (0..area.width)
            .map(|column| buffer[(column, 1)].symbol())
            .collect::<String>();

        assert!(row.contains("Filter files…"));
        assert_eq!(buffer[(0, 1)].fg, palette.accent);
        assert_eq!(buffer[(0, 1)].bg, palette.surface1);
        assert_eq!(targets.query_cursor, Some((2, 1)));
    }

    #[test]
    fn search_toolbar_separates_scope_from_right_aligned_options() {
        let area = Rect::new(0, 0, 32, 5);
        let mut buffer = Buffer::empty(area);
        let model = RenderModel {
            query: Some(String::new()),
            query_input_active: true,
            search_scope: SearchScope::Contents,
            search_case_sensitive: true,
            search_regex: true,
            ..RenderModel::default()
        };
        let palette = Palette::catppuccin();

        let targets = render(&mut buffer, area, &model, &palette);
        let toolbar = (0..area.width)
            .map(|column| buffer[(column, 2)].symbol())
            .collect::<String>();

        assert!(toolbar.starts_with("Files Contents"));
        assert!(toolbar.ends_with("Aa .*"));
        assert!(!toolbar.contains("Ign"));
        assert_eq!(targets.search_control_at(0, 1), Some(SearchControl::Input));
        assert_eq!(targets.search_control_at(0, 2), Some(SearchControl::Files));
        assert_eq!(
            targets.search_control_at(6, 2),
            Some(SearchControl::Contents)
        );
        assert_eq!(
            targets.search_control_at(27, 2),
            Some(SearchControl::CaseSensitive)
        );
        assert_eq!(targets.search_control_at(30, 2), Some(SearchControl::Regex));
        assert_eq!(targets.search_control_at(21, 2), None);
        assert_eq!(
            targets.search_regex.expect("regex control").right(),
            area.right()
        );
        assert!(
            targets.search_contents.expect("Contents control").right()
                < targets.search_case.expect("case control").x
        );
        assert_eq!(buffer[(6, 2)].fg, palette.accent);
        assert_eq!(buffer[(27, 2)].fg, palette.accent);
        assert_eq!(buffer[(30, 2)].fg, palette.accent);
    }

    #[test]
    fn search_options_remain_right_aligned_at_responsive_widths() {
        for width in [20, 24, 32, 48] {
            let area = Rect::new(0, 0, width, 5);
            let mut buffer = Buffer::empty(area);
            let model = RenderModel {
                query: Some(String::new()),
                search_scope: SearchScope::Contents,
                ..RenderModel::default()
            };

            let targets = render(&mut buffer, area, &model, &Palette::catppuccin());
            let contents = targets.search_contents.expect("Contents control");
            let case = targets.search_case.expect("case control");
            let regex = targets.search_regex.expect("regex control");

            assert!(contents.right() < case.x, "overlap at width {width}");
            assert_eq!(case.right().saturating_add(1), regex.x);
            assert_eq!(regex.right(), area.right());
        }
    }

    #[test]
    fn sanitizer_removes_escape_sequences() {
        assert!(!sanitize("a\u{1b}[31mb").contains('\u{1b}'));
        assert_eq!(sanitize_multiline("a\nb\tc\r\u{1b}"), "a\nb    c�");
    }

    #[test]
    fn widget_renders_on_test_backend() {
        let mut terminal = Terminal::new(TestBackend::new(24, 6)).expect("test terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Sidebar {
                        model: &RenderModel::default(),
                        palette: &Palette::default(),
                    },
                    frame.area(),
                )
            })
            .expect("draw");
    }
}
