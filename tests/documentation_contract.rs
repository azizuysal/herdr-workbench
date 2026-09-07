use std::path::Path;

use herdr_workbench::config::{self, Config, IconMode, OpenPlacement};

#[test]
fn documentation_config_and_manifest_match_the_shipped_contract() {
    let example_path = Path::new("config.example.toml");
    let example = Config::load(example_path).expect("configuration example must parse");
    assert_eq!(example.width, 32);
    assert!(example.show_ignored);
    assert!(!example.show_git_directory);
    assert_eq!(example.icons, IconMode::NerdFont);
    assert!(!example.follow_symlinks);
    assert!(example.restore_visible);
    assert!(!example.show_empty_git_groups);
    assert!(example.open.command.is_empty());
    assert_eq!(example.open.placement, OpenPlacement::Tab);

    config::validate_manifest().expect("manifest contract");

    let readme = std::fs::read_to_string("README.md").unwrap();
    for heading in [
        "## What you get",
        "## Install",
        "## Everyday use",
        "## Shortcuts",
        "## Configuration",
        "## Plugin actions",
        "## Platform notes and limitations",
        "## Troubleshooting",
        "## Update or remove",
        "## Development",
        "## License",
    ] {
        assert!(
            readme.contains(heading),
            "README is missing user-facing section: {heading}"
        );
    }
    for required in [
        "Version 1.0.8",
        "Herdr 0.7.5 or newer",
        "herdr plugin install azizuysal/herdr-workbench",
        "herdr plugin list --plugin herdr-workbench",
        "herdr plugin link .",
        "herdr plugin config-dir herdr-workbench",
        "herdr plugin uninstall herdr-workbench",
        "herdr plugin unlink herdr-workbench",
        "herdr-workbench.toggle",
        "herdr-workbench.focus",
        "prefix + e",
        "show_ignored = true",
        "show_git_directory = false",
        "icons = \"nerd-font\"",
        "icons = \"plain\"",
        "restore_visible = true",
        "show_empty_git_groups = false",
        "command = [\"nvim\", \"{path}\"]",
        "placement = \"tab\"",
        "make check",
        "live file and content search",
        "read-only Source Control",
        "colored file icons",
        "Files",
        "Contents",
        "Content search follows Explorer ignored-file visibility",
        "Ignored folder icons and names are muted",
        "Outside a Git repository",
        "latest 50 commits",
        "Toggle working changes / commit history",
        "Press `Space` again to close it",
        "Toggle tree / flat layout",
        "macOS Quick Look",
        "Linux system viewer",
        "$EDITOR",
        "Finder or the Linux file manager",
        "Text previews support syntax highlighting",
        "soft-wrapped long lines",
        "Visual wrapping never changes the text copied",
        "Line numbers are off by default",
        "Cmd+A",
        "Cmd+C",
        "Right click",
        "Shift+Tab",
        "xdg-open",
        "does not launch when it would be the only pane",
        "closes automatically when it becomes the last pane",
        "No accounts, telemetry, cloud sync, or network requests",
        "MIT License",
    ] {
        assert!(
            readme.contains(required),
            "README is missing required text: {required}"
        );
    }
    assert!(
        !readme.contains("placement = \"split\""),
        "README must not document an unsupported external-open placement"
    );
    for internal_detail in [
        "right edge is reserved",
        "read once per refresh with porcelain v2",
        "complete pinned nvim-web-devicons mapping",
        "stale results do not replace",
        "com.apple.quicklook.qlmanage",
    ] {
        assert!(
            !readme.contains(internal_detail),
            "README includes unnecessary implementation detail: {internal_detail}"
        );
    }
    assert!(!readme.contains("herdr plugin update"));
    assert!(!readme.contains("--ref"));
    assert!(readme.contains("Herdr's default prefix, `Ctrl+B`"));
    assert!(!readme.contains("Ctrl+Space"));
    assert!(!readme.contains("$VISUAL"));
    for screenshot in [
        "docs/screenshots/explorer-preview.png",
        "docs/screenshots/content-search.png",
        "docs/screenshots/source-control.png",
    ] {
        assert!(
            Path::new(screenshot).is_file(),
            "README screenshot is missing: {screenshot}"
        );
        assert!(readme.contains(screenshot));
    }

    let public_description = "A polished project sidebar for Herdr with Explorer, live file and content search, read-only Source Control, rich previews, file icons, and Git decorations.";
    let cargo = std::fs::read_to_string("Cargo.toml").unwrap();
    let manifest = std::fs::read_to_string("herdr-plugin.toml").unwrap();
    for source in [cargo, manifest] {
        assert!(
            source.contains(&format!("description = \"{public_description}\"")),
            "public product descriptions must match the recommended GitHub About text"
        );
    }
}

#[test]
fn verification_and_attribution_files_are_complete_and_non_writing() {
    let makefile = std::fs::read_to_string("Makefile").unwrap();
    for required in [
        "cargo fmt --check",
        "cargo clippy --all-targets --all-features -- -D warnings",
        "cargo test --all-features",
        "cargo build --release --locked",
        "validate-manifest",
        "--test visual_contract_matrix",
        "--test documentation_contract",
        "rustfmt --edition 2024 --check tools/generate_icon_data.rs",
    ] {
        assert!(
            makefile.contains(required),
            "Makefile is missing required gate: {required}"
        );
    }
    assert!(!makefile.contains("INSTA_UPDATE"));
    assert!(!makefile.contains("cargo fmt\n"));

    let gitignore = std::fs::read_to_string(".gitignore").unwrap();
    for required in [
        "/.agents/",
        "/.claude/",
        "/.codex/",
        "/.cursor/",
        "/.windsurf/",
        "/AGENTS.md",
        "/CLAUDE.md",
    ] {
        assert!(
            gitignore.lines().any(|line| line == required),
            ".gitignore is missing agent-only path: {required}"
        );
    }

    let notices = std::fs::read_to_string("THIRD_PARTY_NOTICES.md").unwrap();
    assert!(notices.contains("libc` 0.2.189"));
    assert!(notices.contains("licensed under MIT"));
    assert!(notices.contains("TokyoNight.nvim revision"));
    assert!(notices.contains("cli-spinners"));
    assert!(notices.contains("nvim-web-devicons revision"));
    assert!(notices.contains("Catppuccin Icons v1.26.0"));
    assert!(notices.contains("two-face` 0.5.1"));
    assert!(notices.contains("hayro` 0.7.1"));
    assert!(!notices.contains("AGPL-3.0-or-later"));
    assert!(!Path::new("THIRD_PARTY_LICENSES/Herdr-AGPL-3.0-or-later.txt").exists());
    let cargo = std::fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("license = \"MIT\""));
    let mit = std::fs::read_to_string("LICENSE").unwrap();
    assert!(mit.starts_with("MIT License"));
    assert!(mit.contains("Copyright (c) 2026 Aziz Uysal"));
    assert!(!Path::new("LICENSE-APACHE").exists());
    let apache = std::fs::read_to_string("THIRD_PARTY_LICENSES/tokyonight-Apache-2.0.txt").unwrap();
    assert!(apache.contains("Apache License"));
    assert!(apache.contains("Version 2.0, January 2004"));
    let palette_license =
        std::fs::read_to_string("THIRD_PARTY_LICENSES/theme-palettes-and-spinner-MIT.txt").unwrap();
    assert!(palette_license.contains("Copyright (c) 2021 Catppuccin"));
    assert!(palette_license.contains("Copyright (c) Sindre Sorhus"));
    assert!(palette_license.contains("Permission is hereby granted"));
    for path in [
        "THIRD_PARTY_LICENSES/nvim-web-devicons-MIT.txt",
        "THIRD_PARTY_LICENSES/catppuccin-vscode-icons-MIT.txt",
        "THIRD_PARTY_LICENSES/hayro-fallback-fonts-BSD-3-Clause.txt",
        "THIRD_PARTY_LICENSES/hayro-cmaps-Adobe-BSD-3-Clause.txt",
    ] {
        let license = std::fs::read_to_string(path).unwrap();
        assert!(
            license.contains("Redistribution and use")
                || license.starts_with("MIT License")
                    && license.contains("Permission is hereby granted")
        );
    }
    let syntax_notices =
        std::fs::read_to_string("THIRD_PARTY_LICENSES/two-face-syntax-acknowledgements.md")
            .unwrap();
    assert!(syntax_notices.contains("# Syntaxes"));
    assert!(syntax_notices.contains("syntaxes/01_Packages/Rust/LICENSE.txt"));

    let generated = std::fs::read_to_string("src/icons/generated.rs").unwrap();
    assert!(generated.starts_with("// @generated by tools/generate_icon_data.rs"));
    assert!(generated.contains("1188 filename aliases"));
    assert!(generated.contains("949 extension aliases"));
    assert!(generated.contains("425 folder aliases"));

    let ci = std::fs::read_to_string(".github/workflows/ci.yml").unwrap();
    assert!(ci.contains("ubuntu-latest"));
    assert!(ci.contains("macos-latest"));
    assert!(ci.contains("make check"));
}
