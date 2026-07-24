# Third-Party Notices

Herdr Workbench is licensed under MIT. The complete project license text is in `LICENSE`.

The implementation depends on permissively licensed Rust crates recorded exactly in `Cargo.lock`.
Direct dependencies and their declared licenses are:

- `base64` 0.23.0 — MIT OR Apache-2.0
- `ratatui` 0.30.2 — MIT
- `crossterm` 0.29.0 — MIT
- `hayro` 0.7.1 — MIT OR Apache-2.0
- `image` 0.25.10 — MIT OR Apache-2.0
- `libc` 0.2.189 — MIT OR Apache-2.0
- `serde` 1.0.229 — MIT OR Apache-2.0
- `serde_json` 1.0.151 — MIT OR Apache-2.0
- `shlex` 2.0.1 — MIT OR Apache-2.0
- `toml` 1.1.3 — MIT OR Apache-2.0
- `two-face` 0.5.1 — MIT OR Apache-2.0
- `notify` 8.2.0 — CC0-1.0
- `regex` 1.13.1 — MIT OR Apache-2.0
- `unicode-width` 0.2.2 — MIT OR Apache-2.0
- `tempfile` 3.27.0 (development) — MIT OR Apache-2.0
- `insta` 1.48.0 (development) — Apache-2.0

`two-face` embeds the syntax definitions curated by the `bat` project. The complete generated
acknowledgements for every embedded syntax whose license requires preservation are included in
`THIRD_PARTY_LICENSES/two-face-syntax-acknowledgements.md`. No `two-face` color theme is used; syntax
scopes are mapped to Herdr's semantic palette.

Hayro's default PDF support embeds PDFium/Foxit fallback fonts and Adobe CMap data. Their upstream
BSD 3-Clause notices are preserved in
`THIRD_PARTY_LICENSES/hayro-fallback-fonts-BSD-3-Clause.txt` and
`THIRD_PARTY_LICENSES/hayro-cmaps-Adobe-BSD-3-Clause.txt`. Hayro's embedded compact CMYK profile is
CC0-1.0.

The semantic RGB values in `src/theme.rs` are selected from these pinned upstream palettes and
mapped to Herdr's semantic roles:

- Catppuccin Palette revision `07d02aa110ef9eb7e7427afca5c73ba9cf7f8ebd` — MIT
- TokyoNight.nvim revision `cdc07ac78467a233fd62c493de29a17e0cf2b2b6` — Apache-2.0
- Dracula revision `8ada9b3a817d1d7ebe28e6e34c4a79de7e249353` — MIT
- Nord revision `1cef71605416a222e57225b544540ce0fcec18d4` — MIT
- Gruvbox revision `5d15b2765f59754d7ac263c88a0f6e3e58124951` — MIT
- Atom One Dark Syntax revision `9c96f4454362267ac45322063e193ccf9d2debb1` — MIT
- Atom One Light Syntax revision `d84579027410c576086dfca14d934c4bd74b0438` — MIT
- Solarized revision `62f656a02f93c5190a8753159e34b385588d5ff3` — MIT
- Kanagawa revision `bb85e4bfc8d89b0e62c8fa53ccdd13d12e2f77b3` — MIT
- Rosé Pine Palette revision `92af52b465ab6e47437aca223c9b8d3009a2023b` — MIT
- Vesper revision `9043f3849b776949445f0cd4990365959cca35a3` — MIT

The Braille busy-animation sequence in `src/render.rs` is the `dots` sequence from cli-spinners
revision `82c51d1e9d07e0cf95247479414d52b67d4cf019`, authored by Sindre Sorhus and licensed under MIT.
Copyright notices and the complete common MIT terms are preserved in
`THIRD_PARTY_LICENSES/theme-palettes-and-spinner-MIT.txt`. TokyoNight.nvim's complete
Apache-2.0 terms are preserved in `THIRD_PARTY_LICENSES/tokyonight-Apache-2.0.txt`.

The complete filename and extension Nerd Font mapping in `src/icons/generated.rs` is derived from
nvim-web-devicons revision `2ae6958df7ced50baac5035cec0c15799eedfbf7`, authored and maintained
by the nvim-tree project at <https://github.com/nvim-tree/nvim-web-devicons>. nvim-web-devicons is
MIT-licensed; its complete license is preserved in
`THIRD_PARTY_LICENSES/nvim-web-devicons-MIT.txt`.

Additional filename, extension, and folder associations are derived from Catppuccin Icons v1.26.0
revision `b6915da9f6889b683a110aa747de96c2820a537d`, authored and maintained by Catppuccin and
thang-nm at <https://github.com/catppuccin/vscode-icons>. The association data is mapped to
renderable Nerd Font glyphs and Herdr semantic palette roles. Catppuccin Icons is MIT-licensed; its
complete license is preserved in
`THIRD_PARTY_LICENSES/catppuccin-vscode-icons-MIT.txt`.

`tools/generate_icon_data.rs` pins and validates both source revisions. No font binary, SVG, or
glyph artwork is bundled.

No reference-plugin source, palette, font, or asset is copied.
