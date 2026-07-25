//! Palette-independent syntax classification for preview text.

use std::{path::Path, str::FromStr, sync::OnceLock};

use two_face::{
    re_exports::syntect::{
        easy::ScopeRegionIterator,
        highlighting::ScopeSelectors,
        parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet},
    },
    syntax,
};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HighlightRole {
    #[default]
    Text,
    Muted,
    Accent,
    Green,
    Yellow,
    Red,
    Blue,
    Teal,
    Peach,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub text: String,
    pub role: HighlightRole,
    pub bold: bool,
    pub italic: bool,
}

impl HighlightSpan {
    fn new(text: impl Into<String>, role: HighlightRole) -> Self {
        Self {
            text: text.into(),
            role,
            bold: false,
            italic: false,
        }
    }

    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HighlightLine {
    pub spans: Vec<HighlightSpan>,
}

impl HighlightLine {
    fn plain(text: impl Into<String>, role: HighlightRole) -> Self {
        Self {
            spans: vec![HighlightSpan::new(text, role)],
        }
    }

    pub fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.text.as_str()))
            .sum()
    }

    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HighlightedText {
    pub lines: Vec<HighlightLine>,
}

impl HighlightedText {
    pub fn plain(text: &str, role: HighlightRole) -> Self {
        Self {
            lines: text
                .lines()
                .map(|line| HighlightLine::plain(line, role))
                .collect(),
        }
        .with_empty_line()
    }

    pub fn source(
        path: &Path,
        source: &str,
        line_numbers: bool,
        truncated: bool,
    ) -> Result<Self, String> {
        let syntax_set = syntax_set();
        let syntax = detect_syntax(syntax_set, path, source);
        let mut parser = syntax.map(|syntax| SyntaxParser::new(syntax_set, syntax));
        let number_width = source.lines().count().max(1).to_string().len().max(4);
        let mut lines = Vec::new();

        for (index, line) in source.lines().enumerate() {
            let mut spans = Vec::new();
            if line_numbers {
                spans.push(HighlightSpan::new(
                    format!("{:>number_width$}  ", index + 1),
                    HighlightRole::Muted,
                ));
            }
            match parser.as_mut() {
                Some(parser) => spans.extend(parser.line(line)?),
                None => spans.push(HighlightSpan::new(line, HighlightRole::Text)),
            }
            lines.push(HighlightLine { spans });
        }
        if lines.is_empty() {
            lines.push(HighlightLine::default());
        }
        if truncated {
            let message = if line_numbers {
                format!("{:>number_width$}  … [preview truncated]", "")
            } else {
                "… [preview truncated]".to_string()
            };
            lines.push(HighlightLine::plain(message, HighlightRole::Muted));
        }
        Ok(Self { lines })
    }

    pub fn diff(path: &Path, diff: &str) -> Result<Self, String> {
        let syntax_set = syntax_set();
        let syntax = detect_syntax(syntax_set, path, "");
        let mut old_parser = syntax.map(|syntax| SyntaxParser::new(syntax_set, syntax));
        let mut new_parser = syntax.map(|syntax| SyntaxParser::new(syntax_set, syntax));
        let mut lines = Vec::new();

        for line in diff.lines() {
            let highlighted = if line.starts_with("diff --git ")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("similarity index ")
                || line.starts_with("rename from ")
                || line.starts_with("rename to ")
                || line.starts_with("new file mode ")
                || line.starts_with("deleted file mode ")
            {
                HighlightLine::plain(line, HighlightRole::Muted)
            } else if line.starts_with("@@") {
                old_parser = syntax.map(|syntax| SyntaxParser::new(syntax_set, syntax));
                new_parser = syntax.map(|syntax| SyntaxParser::new(syntax_set, syntax));
                HighlightLine {
                    spans: vec![HighlightSpan::new(line, HighlightRole::Accent).bold()],
                }
            } else if line.starts_with("\\ No newline") {
                HighlightLine::plain(line, HighlightRole::Muted)
            } else if let Some(code) = line.strip_prefix('+') {
                let mut spans = vec![HighlightSpan::new("+", HighlightRole::Green).bold()];
                spans.extend(highlight_code_line(new_parser.as_mut(), code)?);
                HighlightLine { spans }
            } else if let Some(code) = line.strip_prefix('-') {
                let mut spans = vec![HighlightSpan::new("-", HighlightRole::Red).bold()];
                spans.extend(highlight_code_line(old_parser.as_mut(), code)?);
                HighlightLine { spans }
            } else if let Some(code) = line.strip_prefix(' ') {
                let old_spans = highlight_code_line(old_parser.as_mut(), code)?;
                let new_spans = highlight_code_line(new_parser.as_mut(), code)?;
                let mut spans = vec![HighlightSpan::new(" ", HighlightRole::Muted)];
                spans.extend(if new_parser.is_some() {
                    new_spans
                } else {
                    old_spans
                });
                HighlightLine { spans }
            } else if line.starts_with("unmerged ") {
                HighlightLine {
                    spans: vec![HighlightSpan::new(line, HighlightRole::Red).bold()],
                }
            } else if line == "[diff preview truncated]" {
                HighlightLine::plain(line, HighlightRole::Muted)
            } else {
                HighlightLine::plain(line, HighlightRole::Text)
            };
            lines.push(highlighted);
        }
        Ok(Self { lines }.with_empty_line())
    }

    pub fn prepend(mut self, line: HighlightLine) -> Self {
        self.lines.insert(0, line);
        self
    }

    pub fn width(&self) -> usize {
        self.lines
            .iter()
            .map(HighlightLine::width)
            .max()
            .unwrap_or(0)
    }

    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(HighlightLine::plain_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn with_empty_line(mut self) -> Self {
        if self.lines.is_empty() {
            self.lines.push(HighlightLine::default());
        }
        self
    }
}

pub fn label(text: impl Into<String>, role: HighlightRole) -> HighlightLine {
    HighlightLine {
        spans: vec![HighlightSpan::new(text, role).bold()],
    }
}

fn highlight_code_line(
    parser: Option<&mut SyntaxParser<'_>>,
    code: &str,
) -> Result<Vec<HighlightSpan>, String> {
    match parser {
        Some(parser) => parser.line(code),
        None => Ok(vec![HighlightSpan::new(code, HighlightRole::Text)]),
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(syntax::extra_newlines)
}

fn detect_syntax<'a>(
    syntax_set: &'a SyntaxSet,
    path: &Path,
    source: &str,
) -> Option<&'a SyntaxReference> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    syntax_set
        .find_syntax_by_extension(file_name)
        .or_else(|| syntax_set.find_syntax_by_extension(extension))
        .or_else(|| {
            source
                .lines()
                .next()
                .and_then(|line| syntax_set.find_syntax_by_first_line(line))
        })
}

struct SyntaxParser<'a> {
    syntax_set: &'a SyntaxSet,
    state: ParseState,
    stack: ScopeStack,
    selectors: &'static Selectors,
}

impl<'a> SyntaxParser<'a> {
    fn new(syntax_set: &'a SyntaxSet, syntax: &'a SyntaxReference) -> Self {
        Self {
            syntax_set,
            state: ParseState::new(syntax),
            stack: ScopeStack::new(),
            selectors: selectors(),
        }
    }

    fn line(&mut self, line: &str) -> Result<Vec<HighlightSpan>, String> {
        let parsed = format!("{line}\n");
        let operations = self
            .state
            .parse_line(&parsed, self.syntax_set)
            .map_err(|error| format!("cannot highlight preview: {error}"))?;
        let mut spans = Vec::new();
        for (region, operation) in ScopeRegionIterator::new(&operations, &parsed) {
            self.stack
                .apply(operation)
                .map_err(|error| format!("cannot highlight preview: {error}"))?;
            let region = region.strip_suffix('\n').unwrap_or(region);
            if region.is_empty() {
                continue;
            }
            let (role, bold, italic) = self.selectors.classify(self.stack.as_slice());
            spans.push(HighlightSpan {
                text: region.to_string(),
                role,
                bold,
                italic,
            });
        }
        if spans.is_empty() {
            spans.push(HighlightSpan::new(line, HighlightRole::Text));
        }
        Ok(spans)
    }
}

struct Selectors {
    invalid: ScopeSelectors,
    comment: ScopeSelectors,
    string: ScopeSelectors,
    constant: ScopeSelectors,
    keyword: ScopeSelectors,
    function: ScopeSelectors,
    type_name: ScopeSelectors,
    tag: ScopeSelectors,
    heading: ScopeSelectors,
    inserted: ScopeSelectors,
    deleted: ScopeSelectors,
}

impl Selectors {
    fn new() -> Self {
        let parse = |value| ScopeSelectors::from_str(value).expect("valid scope selector");
        Self {
            invalid: parse("invalid, invalid.illegal, invalid.deprecated"),
            comment: parse("comment"),
            string: parse("string, constant.other.symbol"),
            constant: parse(
                "constant.numeric, constant.language, constant.character, constant.other",
            ),
            keyword: parse("keyword, storage, punctuation.definition.keyword"),
            function: parse("entity.name.function, support.function, meta.function-call"),
            type_name: parse(
                "entity.name.class, entity.name.struct, entity.name.enum, entity.name.type, support.type, storage.type",
            ),
            tag: parse("entity.name.tag, entity.other.attribute-name"),
            heading: parse("markup.heading"),
            inserted: parse("markup.inserted"),
            deleted: parse("markup.deleted"),
        }
    }

    fn classify(
        &self,
        stack: &[two_face::re_exports::syntect::parsing::Scope],
    ) -> (HighlightRole, bool, bool) {
        if self.invalid.does_match(stack).is_some() {
            (HighlightRole::Red, true, false)
        } else if self.comment.does_match(stack).is_some() {
            (HighlightRole::Muted, false, true)
        } else if self.deleted.does_match(stack).is_some() {
            (HighlightRole::Red, false, false)
        } else if self.inserted.does_match(stack).is_some() {
            (HighlightRole::Green, false, false)
        } else if self.heading.does_match(stack).is_some() {
            (HighlightRole::Accent, true, false)
        } else if self.string.does_match(stack).is_some() {
            (HighlightRole::Green, false, false)
        } else if self.constant.does_match(stack).is_some() {
            (HighlightRole::Peach, false, false)
        } else if self.keyword.does_match(stack).is_some() {
            (HighlightRole::Accent, true, false)
        } else if self.function.does_match(stack).is_some() {
            (HighlightRole::Blue, false, false)
        } else if self.type_name.does_match(stack).is_some() {
            (HighlightRole::Yellow, false, false)
        } else if self.tag.does_match(stack).is_some() {
            (HighlightRole::Teal, false, false)
        } else {
            (HighlightRole::Text, false, false)
        }
    }
}

fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();
    SELECTORS.get_or_init(Selectors::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_source_uses_semantic_roles_and_line_numbers() {
        let highlighted = HighlightedText::source(
            Path::new("src/main.rs"),
            "pub fn answer() -> u32 { 42 } // result",
            true,
            false,
        )
        .expect("highlight");

        let roles = highlighted.lines[0]
            .spans
            .iter()
            .map(|span| span.role)
            .collect::<Vec<_>>();
        assert!(roles.contains(&HighlightRole::Accent));
        assert!(roles.contains(&HighlightRole::Blue));
        assert!(roles.contains(&HighlightRole::Peach));
        assert!(roles.contains(&HighlightRole::Muted));
        assert_eq!(highlighted.lines[0].spans[0].text, "   1  ".to_string());
    }

    #[test]
    fn diff_preserves_git_roles_while_highlighting_code() {
        let highlighted = HighlightedText::diff(
            Path::new("src/main.rs"),
            "@@ -1 +1 @@\n-pub fn old() { 1 }\n+pub fn new() { 2 }",
        )
        .expect("highlight");

        assert_eq!(highlighted.lines[1].spans[0].role, HighlightRole::Red);
        assert_eq!(highlighted.lines[2].spans[0].role, HighlightRole::Green);
        assert!(
            highlighted.lines[2]
                .spans
                .iter()
                .any(|span| span.role == HighlightRole::Blue)
        );
    }

    #[test]
    fn unsupported_file_is_safe_plain_text() {
        let highlighted =
            HighlightedText::source(Path::new("data.unknown"), "safe\\x1b", true, false)
                .expect("plain");
        assert_eq!(highlighted.plain_text(), "   1  safe\\x1b");

        let without_numbers =
            HighlightedText::source(Path::new("data.unknown"), "safe", false, true).expect("plain");
        assert_eq!(without_numbers.plain_text(), "safe\n… [preview truncated]");
    }
}
