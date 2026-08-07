//! Complete, generated one-cell developer file icon mapping.
//!
//! nvim-web-devicons supplies every renderable Nerd Font filename and extension
//! rule. Catppuccin Icons supplies additional filename, extension, and folder
//! aliases. `tools/generate_icon_data.rs` pins both sources, validates their
//! expected shapes, and translates fixed upstream colors into Herdr semantic
//! palette roles. Plain mode always uses a one-cell ASCII glyph.

use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

mod generated;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconMode {
    NerdFont,
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Commit,
    Symlink,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileColor {
    Accent,
    Blue,
    Green,
    Yellow,
    Red,
    Teal,
    Peach,
    Text,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon {
    pub nerd: &'static str,
    pub plain: &'static str,
    pub color: FileColor,
}

impl Icon {
    pub fn glyph(self, mode: IconMode) -> &'static str {
        match mode {
            IconMode::NerdFont => self.nerd,
            IconMode::Plain => self.plain,
        }
    }

    pub fn width(self, mode: IconMode) -> usize {
        UnicodeWidthStr::width(self.glyph(mode))
    }
}

const FILE: Icon = Icon {
    nerd: "",
    plain: "f",
    color: FileColor::Text,
};
const DIRECTORY: Icon = Icon {
    nerd: "",
    plain: "d",
    color: FileColor::Blue,
};
const SYMLINK: Icon = Icon {
    nerd: "",
    plain: "@",
    color: FileColor::Teal,
};
const COMMIT: Icon = Icon {
    nerd: "",
    plain: "o",
    color: FileColor::Yellow,
};
const UNKNOWN: Icon = Icon {
    nerd: "",
    plain: "?",
    color: FileColor::Muted,
};

pub fn icon_for(name: &str, kind: EntryKind) -> Icon {
    match kind {
        EntryKind::Symlink => SYMLINK,
        EntryKind::Commit => COMMIT,
        EntryKind::Unknown => UNKNOWN,
        EntryKind::Directory => {
            let lower = name.to_ascii_lowercase();
            lookup(generated::FOLDERS, &lower).unwrap_or(DIRECTORY)
        }
        EntryKind::File => {
            let lower = name.to_ascii_lowercase();
            lookup(generated::FILE_NAMES, &lower)
                .or_else(|| extension_icon(&lower))
                .unwrap_or(FILE)
        }
    }
}

fn extension_icon(name: &str) -> Option<Icon> {
    if name.contains('.')
        && let Some(icon) = lookup(generated::EXTENSIONS, name)
    {
        return Some(icon);
    }
    let mut remainder = name;
    while let Some((_, suffix)) = remainder.split_once('.') {
        if let Some(icon) = lookup(generated::EXTENSIONS, suffix) {
            return Some(icon);
        }
        remainder = suffix;
    }
    None
}

fn lookup(entries: &[(&str, u16)], key: &str) -> Option<Icon> {
    entries
        .binary_search_by(|(candidate, _)| candidate.cmp(&key))
        .ok()
        .map(|index| generated::ICONS[usize::from(entries[index].1)])
}

pub fn file_color(icon: Icon, palette: &crate::theme::Palette) -> Color {
    match icon.color {
        FileColor::Accent => palette.accent,
        FileColor::Blue => palette.blue,
        FileColor::Green => palette.green,
        FileColor::Yellow => palette.yellow,
        FileColor::Red => palette.red,
        FileColor::Teal => palette.teal,
        FileColor::Peach => palette.peach,
        FileColor::Text => palette.text,
        FileColor::Muted => palette.text,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_generated_mode_stays_one_cell() {
        for icon in generated::ICONS
            .iter()
            .chain([&FILE, &DIRECTORY, &COMMIT, &SYMLINK, &UNKNOWN])
        {
            assert_eq!(icon.width(IconMode::Plain), 1, "{}", icon.plain);
            assert_eq!(icon.width(IconMode::NerdFont), 1, "{}", icon.nerd);
        }
    }

    #[test]
    fn generated_tables_preserve_complete_pinned_coverage() {
        assert_eq!(generated::FILE_NAMES.len(), 1_188);
        assert_eq!(generated::EXTENSIONS.len(), 949);
        assert_eq!(generated::FOLDERS.len(), 425);
        for table in [
            generated::FILE_NAMES,
            generated::EXTENSIONS,
            generated::FOLDERS,
        ] {
            assert!(
                table.windows(2).all(|entries| entries[0].0 < entries[1].0),
                "generated lookup table must be strictly sorted"
            );
            assert!(
                table
                    .iter()
                    .all(|(_, id)| usize::from(*id) < generated::ICONS.len()),
                "generated lookup table contains an invalid icon ID"
            );
        }

        let glyphs = generated::ICONS
            .iter()
            .map(|icon| icon.nerd)
            .collect::<BTreeSet<_>>();
        assert!(
            glyphs.len() >= 268,
            "all nvim-web-devicons glyphs must remain represented"
        );
    }

    #[test]
    fn precedence_is_exact_then_longest_extension_with_uniform_directories() {
        assert_ne!(
            icon_for("package.json", EntryKind::File),
            icon_for("file.json", EntryKind::File)
        );
        assert_eq!(
            icon_for("src", EntryKind::Directory),
            icon_for("ordinary", EntryKind::Directory)
        );
        assert_eq!(
            icon_for("thing.test.ts", EntryKind::File),
            icon_for("test.ts", EntryKind::File)
        );
        assert_ne!(
            icon_for("thing.test.ts", EntryKind::File),
            icon_for("thing.ts", EntryKind::File)
        );
    }

    #[test]
    fn compound_extensions_match_only_on_dot_boundaries() {
        assert_eq!(
            icon_for("thing.test.ts", EntryKind::File),
            icon_for("test.ts", EntryKind::File)
        );
        assert_eq!(
            icon_for("archive.tar.gz", EntryKind::File),
            icon_for("tar.gz", EntryKind::File)
        );
        assert_eq!(
            icon_for("contest.ts", EntryKind::File),
            icon_for("plain.ts", EntryKind::File)
        );
    }

    #[test]
    fn catppuccin_only_aliases_and_folders_are_recognized() {
        assert_ne!(icon_for("scene.aep", EntryKind::File), FILE);
        assert_ne!(icon_for("schema.proto", EntryKind::File), FILE);
        assert_ne!(icon_for("component.marko", EntryKind::File), FILE);
        assert!(lookup(generated::FOLDERS, "typings").is_some());
        assert!(lookup(generated::FOLDERS, "controllers").is_some());
        assert!(
            generated::FOLDERS
                .iter()
                .all(|(_, id)| { generated::ICONS[usize::from(*id)].color == FileColor::Blue })
        );
    }

    #[test]
    fn representative_categories_keep_distinct_glyphs() {
        let icons = [
            icon_for("main.rs", EntryKind::File).nerd,
            icon_for("main.py", EntryKind::File).nerd,
            icon_for("main.go", EntryKind::File).nerd,
            icon_for("main.ts", EntryKind::File).nerd,
            icon_for("README.md", EntryKind::File).nerd,
            icon_for("Dockerfile", EntryKind::File).nerd,
        ];
        for (index, icon) in icons.iter().enumerate() {
            assert!(
                !icons[..index].contains(icon),
                "representative icon {icon} is duplicated"
            );
        }
    }

    #[test]
    fn neutral_icons_use_high_contrast_text_in_dark_and_light_themes() {
        let makefile = icon_for("Makefile", EntryKind::File);
        assert_eq!(makefile.color, FileColor::Muted);

        let dark = crate::theme::Palette::catppuccin();
        let light = crate::theme::Palette::catppuccin_latte();
        for icon in generated::ICONS
            .iter()
            .filter(|icon| icon.color == FileColor::Muted)
            .chain([&UNKNOWN])
        {
            assert_eq!(file_color(*icon, &dark), dark.text);
            assert_eq!(file_color(*icon, &light), light.text);
        }

        assert_eq!(dark.text, Color::Rgb(205, 214, 244));
        assert_eq!(light.text, Color::Rgb(76, 79, 105));
    }
}
