use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    process::Command,
};

const NVIM_REVISION: &str = "2ae6958df7ced50baac5035cec0c15799eedfbf7";
const CATPPUCCIN_VERSION: &str = "1.26.0";
const CATPPUCCIN_REVISION: &str = "b6915da9f6889b683a110aa747de96c2820a537d";

const NVIM_FILENAMES_URL: &str = "https://raw.githubusercontent.com/nvim-tree/nvim-web-devicons/2ae6958df7ced50baac5035cec0c15799eedfbf7/lua/nvim-web-devicons/default/icons_by_filename.lua";
const NVIM_EXTENSIONS_URL: &str = "https://raw.githubusercontent.com/nvim-tree/nvim-web-devicons/2ae6958df7ced50baac5035cec0c15799eedfbf7/lua/nvim-web-devicons/default/icons_by_file_extension.lua";
const CATPPUCCIN_FILES_URL: &str = "https://raw.githubusercontent.com/catppuccin/vscode-icons/b6915da9f6889b683a110aa747de96c2820a537d/src/defaults/fileIcons.ts";
const CATPPUCCIN_FOLDERS_URL: &str = "https://raw.githubusercontent.com/catppuccin/vscode-icons/b6915da9f6889b683a110aa747de96c2820a537d/src/defaults/folderIcons.ts";

const EXPECTED_NVIM_FILENAMES: usize = 217;
const EXPECTED_NVIM_EXTENSIONS: usize = 493;
const EXPECTED_NVIM_GLYPHS: usize = 268;
const EXPECTED_CATPPUCCIN_FILE_GROUPS: usize = 392;
const EXPECTED_CATPPUCCIN_EXTENSIONS: usize = 729;
const EXPECTED_CATPPUCCIN_FILENAMES: usize = 1_084;
const EXPECTED_CATPPUCCIN_FOLDER_GROUPS: usize = 113;
const EXPECTED_CATPPUCCIN_FOLDERS: usize = 421;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Role {
    Accent,
    Blue,
    Green,
    Muted,
    Peach,
    Red,
    Teal,
    Text,
    Yellow,
}

impl Role {
    fn rust_name(self) -> &'static str {
        match self {
            Self::Accent => "Accent",
            Self::Blue => "Blue",
            Self::Green => "Green",
            Self::Muted => "Muted",
            Self::Peach => "Peach",
            Self::Red => "Red",
            Self::Teal => "Teal",
            Self::Text => "Text",
            Self::Yellow => "Yellow",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct IconSpec {
    nerd: String,
    plain: String,
    role: Role,
}

#[derive(Clone, Debug)]
struct NvimEntry {
    key: String,
    icon: IconSpec,
    name: String,
}

#[derive(Clone, Debug, Default)]
struct CatFileGroup {
    language_ids: Vec<String>,
    extensions: Vec<String>,
    filenames: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CatField {
    LanguageIds,
    Extensions,
    Filenames,
    Folders,
}

fn main() -> Result<(), String> {
    let output = env::args()
        .nth(1)
        .unwrap_or_else(|| "src/icons/generated.rs".to_string());

    let nvim_filenames = parse_nvim(&fetch(NVIM_FILENAMES_URL)?)?;
    let nvim_extensions = parse_nvim(&fetch(NVIM_EXTENSIONS_URL)?)?;
    validate_count(
        "nvim filename entries",
        nvim_filenames.len(),
        EXPECTED_NVIM_FILENAMES,
    )?;
    validate_count(
        "nvim extension entries",
        nvim_extensions.len(),
        EXPECTED_NVIM_EXTENSIONS,
    )?;

    let cat_file_groups = parse_cat_files(&fetch(CATPPUCCIN_FILES_URL)?)?;
    let cat_folder_groups = parse_cat_folders(&fetch(CATPPUCCIN_FOLDERS_URL)?)?;
    validate_count(
        "Catppuccin file icon groups",
        cat_file_groups.len(),
        EXPECTED_CATPPUCCIN_FILE_GROUPS,
    )?;
    validate_count(
        "Catppuccin folder icon groups",
        cat_folder_groups.len(),
        EXPECTED_CATPPUCCIN_FOLDER_GROUPS,
    )?;

    let cat_extensions = cat_file_groups
        .values()
        .flat_map(|group| &group.extensions)
        .map(|value| value.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let cat_filenames = cat_file_groups
        .values()
        .flat_map(|group| &group.filenames)
        .map(|value| value.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let cat_folders = cat_folder_groups
        .values()
        .flatten()
        .map(|value| value.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    validate_count(
        "Catppuccin unique extension aliases",
        cat_extensions.len(),
        EXPECTED_CATPPUCCIN_EXTENSIONS,
    )?;
    validate_count(
        "Catppuccin unique filename aliases",
        cat_filenames.len(),
        EXPECTED_CATPPUCCIN_FILENAMES,
    )?;
    validate_count(
        "Catppuccin unique folder aliases",
        cat_folders.len(),
        EXPECTED_CATPPUCCIN_FOLDERS,
    )?;

    let mut filenames = BTreeMap::<String, IconSpec>::new();
    let mut extensions = BTreeMap::<String, IconSpec>::new();
    let mut nvim_names = BTreeMap::<String, IconSpec>::new();
    let mut nvim_glyphs = BTreeSet::new();

    for entry in &nvim_filenames {
        filenames.insert(entry.key.to_ascii_lowercase(), entry.icon.clone());
        nvim_names
            .entry(normalize(&entry.name))
            .or_insert_with(|| entry.icon.clone());
        nvim_glyphs.insert(entry.icon.nerd.clone());
    }
    for entry in &nvim_extensions {
        extensions.insert(entry.key.to_ascii_lowercase(), entry.icon.clone());
        nvim_names
            .entry(normalize(&entry.name))
            .or_insert_with(|| entry.icon.clone());
        nvim_glyphs.insert(entry.icon.nerd.clone());
    }
    validate_count(
        "nvim unique Nerd Font glyphs",
        nvim_glyphs.len(),
        EXPECTED_NVIM_GLYPHS,
    )?;

    let mut generic_cat_groups = Vec::new();
    for (group_name, group) in &cat_file_groups {
        let (icon, generic) =
            choose_cat_icon(group_name, group, &filenames, &extensions, &nvim_names);
        if generic {
            generic_cat_groups.push(group_name.clone());
        }
        for filename in &group.filenames {
            filenames
                .entry(filename.to_ascii_lowercase())
                .or_insert_with(|| icon.clone());
        }
        for extension in &group.extensions {
            extensions
                .entry(extension.to_ascii_lowercase())
                .or_insert_with(|| icon.clone());
        }
    }

    let mut folders = BTreeMap::<String, IconSpec>::new();
    for aliases in cat_folder_groups.values() {
        let icon = folder_icon();
        for alias in aliases {
            folders
                .entry(alias.to_ascii_lowercase())
                .or_insert_with(|| icon.clone());
        }
    }
    for alias in [
        ".git",
        "generated",
        "include",
        "internal",
        "target",
        "vendor",
    ] {
        folders.entry(alias.to_string()).or_insert_with(folder_icon);
    }

    let generated = render_generated(&filenames, &extensions, &folders)?;
    fs::create_dir_all(
        std::path::Path::new(&output)
            .parent()
            .ok_or_else(|| format!("output path has no parent: {output}"))?,
    )
    .map_err(|error| format!("cannot create output directory for {output}: {error}"))?;
    fs::write(&output, generated).map_err(|error| format!("cannot write {output}: {error}"))?;

    eprintln!(
        "generated {output}: {} filenames, {} extensions, {} folders, {} Catppuccin groups used the generic file fallback",
        filenames.len(),
        extensions.len(),
        folders.len(),
        generic_cat_groups.len()
    );
    if !generic_cat_groups.is_empty() {
        eprintln!(
            "generic Catppuccin groups: {}",
            generic_cat_groups.join(", ")
        );
    }
    Ok(())
}

fn fetch(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", url])
        .output()
        .map_err(|error| format!("cannot run curl for {url}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("non-UTF-8 source at {url}: {error}"))
}

fn validate_count(label: &str, actual: usize, expected: usize) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} changed: expected {expected}, received {actual}; inspect upstream before regenerating"
        ))
    }
}

fn parse_nvim(source: &str) -> Result<Vec<NvimEntry>, String> {
    let mut entries = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("[\"") {
            continue;
        }
        let key = extract(trimmed, "[\"", "\"]")
            .ok_or_else(|| format!("cannot parse nvim key: {trimmed}"))?;
        let nerd = extract(trimmed, "icon = \"", "\"")
            .ok_or_else(|| format!("cannot parse nvim glyph: {trimmed}"))?;
        let color = extract(trimmed, "color = \"", "\"")
            .ok_or_else(|| format!("cannot parse nvim color: {trimmed}"))?;
        let name = extract(trimmed, "name = \"", "\"")
            .ok_or_else(|| format!("cannot parse nvim name: {trimmed}"))?;
        entries.push(NvimEntry {
            key: key.to_string(),
            icon: IconSpec {
                nerd: nerd.to_string(),
                plain: plain_for(name),
                role: role_for_hex(color)?,
            },
            name: name.to_string(),
        });
    }
    Ok(entries)
}

fn extract<'a>(source: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = source.find(prefix)? + prefix.len();
    let remaining = &source[start..];
    let end = remaining.find(suffix)?;
    Some(&remaining[..end])
}

fn parse_cat_files(source: &str) -> Result<BTreeMap<String, CatFileGroup>, String> {
    let mut groups = BTreeMap::<String, CatFileGroup>::new();
    let mut group = None::<String>;
    let mut field = None::<CatField>;

    for line in source.lines() {
        if let Some(name) = group_declaration(line) {
            groups.entry(name.clone()).or_default();
            group = Some(name);
            field = None;
            continue;
        }
        let Some(group_name) = group.as_ref() else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed == "}," {
            group = None;
            field = None;
            continue;
        }
        if trimmed.starts_with("languageIds:") {
            field = Some(CatField::LanguageIds);
        } else if trimmed.starts_with("fileExtensions:") {
            field = Some(CatField::Extensions);
        } else if trimmed.starts_with("fileNames:") {
            field = Some(CatField::Filenames);
        }
        let Some(active_field) = field else {
            continue;
        };
        let values = quoted_values(line)?;
        let target = groups
            .get_mut(group_name)
            .ok_or_else(|| format!("missing Catppuccin group {group_name}"))?;
        match active_field {
            CatField::LanguageIds => target.language_ids.extend(values),
            CatField::Extensions => target.extensions.extend(values),
            CatField::Filenames => target.filenames.extend(values),
            CatField::Folders => unreachable!(),
        }
        if line.contains(']') {
            field = None;
        }
    }
    Ok(groups)
}

fn parse_cat_folders(source: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    let mut group = None::<String>;
    let mut field = None::<CatField>;

    for line in source.lines() {
        if let Some(name) = group_declaration(line) {
            groups.entry(name.clone()).or_default();
            group = Some(name);
            field = None;
            continue;
        }
        let Some(group_name) = group.as_ref() else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed == "}," {
            group = None;
            field = None;
            continue;
        }
        if trimmed.starts_with("folderNames:") {
            field = Some(CatField::Folders);
        }
        if field != Some(CatField::Folders) {
            continue;
        }
        groups
            .get_mut(group_name)
            .ok_or_else(|| format!("missing Catppuccin folder group {group_name}"))?
            .extend(quoted_values(line)?);
        if line.contains(']') {
            field = None;
        }
    }
    Ok(groups)
}

fn group_declaration(line: &str) -> Option<String> {
    let remaining = line.strip_prefix("  '")?;
    let (name, suffix) = remaining.split_once("': {")?;
    suffix.trim().is_empty().then(|| name.to_string())
}

fn quoted_values(line: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in line.chars() {
        if !quoted {
            if character == '\'' {
                quoted = true;
                current.clear();
            }
            continue;
        }
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '\'' {
            values.push(current.clone());
            quoted = false;
        } else {
            current.push(character);
        }
    }
    if quoted || escaped {
        Err(format!("unterminated quoted Catppuccin value: {line}"))
    } else {
        Ok(values)
    }
}

fn choose_cat_icon(
    group_name: &str,
    group: &CatFileGroup,
    filenames: &BTreeMap<String, IconSpec>,
    extensions: &BTreeMap<String, IconSpec>,
    nvim_names: &BTreeMap<String, IconSpec>,
) -> (IconSpec, bool) {
    let mut candidates = BTreeMap::<IconSpec, usize>::new();
    if let Some(icon) = nvim_names.get(&normalize(group_name)) {
        add_candidate(&mut candidates, icon, 100);
    }
    for language in &group.language_ids {
        if let Some(icon) = nvim_names.get(&normalize(language)) {
            add_candidate(&mut candidates, icon, 25);
        }
    }
    for filename in &group.filenames {
        let lower = filename.to_ascii_lowercase();
        if let Some(icon) = filenames.get(&lower) {
            add_candidate(&mut candidates, icon, 10);
        } else if let Some(icon) = extension_lookup(&lower, extensions) {
            add_candidate(&mut candidates, icon, 2);
        }
    }
    for extension in &group.extensions {
        let lower = extension.to_ascii_lowercase();
        if let Some(icon) = extensions.get(&lower) {
            add_candidate(&mut candidates, icon, 10);
        } else if let Some((_, suffix)) = lower.rsplit_once('.')
            && let Some(icon) = extensions.get(suffix)
        {
            add_candidate(&mut candidates, icon, 2);
        }
    }
    if let Some((icon, _)) =
        candidates
            .into_iter()
            .max_by(|(left_icon, left_count), (right_icon, right_count)| {
                left_count
                    .cmp(right_count)
                    .then_with(|| right_icon.cmp(left_icon))
            })
    {
        return (icon, false);
    }
    let icon = fallback_icon(group_name, filenames, extensions);
    let generic = icon == generic_file();
    (icon, generic)
}

fn add_candidate(candidates: &mut BTreeMap<IconSpec, usize>, icon: &IconSpec, weight: usize) {
    *candidates.entry(icon.clone()).or_default() += weight;
}

fn extension_lookup<'a>(
    name: &str,
    extensions: &'a BTreeMap<String, IconSpec>,
) -> Option<&'a IconSpec> {
    let mut remainder = name;
    while let Some((_, suffix)) = remainder.split_once('.') {
        if let Some(icon) = extensions.get(suffix) {
            return Some(icon);
        }
        remainder = suffix;
    }
    None
}

fn fallback_icon(
    group_name: &str,
    filenames: &BTreeMap<String, IconSpec>,
    extensions: &BTreeMap<String, IconSpec>,
) -> IconSpec {
    let explicit_probe = match group_name {
        "adobe-ae" | "adobe-id" | "adobe-xd" | "drawio" | "figma" | "sketch" => Some("png"),
        "amber" | "antlr" | "hare" | "jule" | "rsml" | "spwn" => Some("rs"),
        "api-blueprint" | "mermaid" | "plantuml" | "slidesk" => Some("md"),
        "apple" | "swiftformat" | "xcode" => Some("swift"),
        "autohotkey" | "envrc" | "vhs" => Some("sh"),
        "blink" | "jinja" | "latte" | "marko" | "mjml" | "nunjucks" | "phtml" | "vento" => {
            Some("html")
        }
        "browserslist" | "caddy" | "cursor" | "cursor-ignore" | "kdl" | "nx-ignore" | "search"
        | "semgrep-ignore" | "sentry" | "stackblitz" | "stylelint-ignore" | "vercel-ignore"
        | "vs-codium" | "vscode-ignore" | "zap" => Some("conf"),
        "cabal" => Some("hs"),
        "certificate" => Some("lock"),
        "codeowners" => Some(".gitignore"),
        "css-map" => Some("css"),
        "dhall" => Some("toml"),
        "django" | "twine" => Some("py"),
        "fastlane" => Some("rb"),
        "flutter" => Some("dart"),
        "java-class" => Some("java"),
        "javascript-map" | "vital" => Some("js"),
        "juce" => Some("cpp"),
        "lisp" => Some("clj"),
        "lua-rocks" | "roblox" | "stylua-ignore" => Some("lua"),
        "midi" => Some("mp3"),
        "nextflow" => Some("groovy"),
        "ninja" => Some("makefile"),
        "nuxt-ignore" => Some("vue"),
        "proto" | "prototools" => Some("graphql"),
        "rdata" => Some("r"),
        "reason" => Some("ml"),
        "salesforce" => Some("xml"),
        "squirrel" => Some("lua"),
        "stata" => Some("sql"),
        "tauri-ignore" => Some("rs"),
        "url" => Some("html"),
        "vapi" => Some("vala"),
        "workflow" => Some("yaml"),
        _ => None,
    };
    if let Some(icon) =
        explicit_probe.and_then(|probe| filenames.get(probe).or_else(|| extensions.get(probe)))
    {
        return icon.clone();
    }

    let group = normalize(group_name);
    let probes: &[&str] = if contains_any(&group, &["test", "spec", "benchmark", "coverage"]) {
        &["test.ts"]
    } else if contains_any(&group, &["image", "photo", "adobe", "3d", "draw"]) {
        &["png"]
    } else if contains_any(&group, &["audio", "music", "sound"]) {
        &["mp3"]
    } else if contains_any(&group, &["video", "movie"]) {
        &["mp4"]
    } else if contains_any(&group, &["archive", "compressed", "zip"]) {
        &["zip"]
    } else if contains_any(&group, &["database", "sql", "data"]) {
        &["sql"]
    } else if contains_any(
        &group,
        &["document", "markdown", "readme", "license", "text"],
    ) {
        &["md"]
    } else if contains_any(&group, &["config", "setting", "lint", "format"]) {
        &["conf"]
    } else if contains_any(&group, &["docker", "container"]) {
        &["dockerfile"]
    } else if contains_any(&group, &["git", "github", "gitlab"]) {
        &[".gitignore"]
    } else if contains_any(&group, &["package", "npm", "node"]) {
        &["package.json"]
    } else if contains_any(&group, &["lock", "checksum"]) {
        &["lock"]
    } else if contains_any(&group, &["font", "typeface"]) {
        &["ttf"]
    } else if contains_any(&group, &["shell", "terminal", "command"]) {
        &["sh"]
    } else {
        &[]
    };
    for probe in probes {
        if let Some(icon) = filenames.get(*probe).or_else(|| extensions.get(*probe)) {
            return icon.clone();
        }
    }
    generic_file()
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn plain_for(name: &str) -> String {
    name.chars()
        .find(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase().to_string())
        .unwrap_or_else(|| "f".to_string())
}

fn role_for_hex(value: &str) -> Result<Role, String> {
    let raw = value
        .strip_prefix('#')
        .ok_or_else(|| format!("icon color is not hexadecimal: {value}"))?;
    if raw.len() != 6 {
        return Err(format!("icon color is not six digits: {value}"));
    }
    let component = |start| {
        u8::from_str_radix(&raw[start..start + 2], 16)
            .map_err(|error| format!("invalid icon color {value}: {error}"))
    };
    let red = f64::from(component(0)?) / 255.0;
    let green = f64::from(component(2)?) / 255.0;
    let blue = f64::from(component(4)?) / 255.0;
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let delta = maximum - minimum;
    let saturation = if maximum == 0.0 { 0.0 } else { delta / maximum };
    if saturation < 0.25 {
        return Ok(if maximum >= 0.65 {
            Role::Text
        } else {
            Role::Muted
        });
    }
    let hue = if delta == 0.0 {
        0.0
    } else if maximum == red {
        60.0 * ((green - blue) / delta).rem_euclid(6.0)
    } else if maximum == green {
        60.0 * (((blue - red) / delta) + 2.0)
    } else {
        60.0 * (((red - green) / delta) + 4.0)
    };
    Ok(match hue {
        hue if !(15.0..345.0).contains(&hue) => Role::Red,
        hue if hue < 45.0 => Role::Peach,
        hue if hue < 75.0 => Role::Yellow,
        hue if hue < 160.0 => Role::Green,
        hue if hue < 200.0 => Role::Teal,
        hue if hue < 260.0 => Role::Blue,
        _ => Role::Accent,
    })
}

fn generic_file() -> IconSpec {
    IconSpec {
        nerd: "".to_string(),
        plain: "f".to_string(),
        role: Role::Text,
    }
}

fn folder_icon() -> IconSpec {
    IconSpec {
        nerd: "".to_string(),
        plain: "d".to_string(),
        role: Role::Blue,
    }
}

fn render_generated(
    filenames: &BTreeMap<String, IconSpec>,
    extensions: &BTreeMap<String, IconSpec>,
    folders: &BTreeMap<String, IconSpec>,
) -> Result<String, String> {
    let mut specs = filenames
        .values()
        .chain(extensions.values())
        .chain(folders.values())
        .cloned()
        .collect::<BTreeSet<_>>();
    specs.insert(generic_file());
    let specs = specs.into_iter().collect::<Vec<_>>();
    if specs.len() > usize::from(u16::MAX) {
        return Err(format!("too many icon specifications: {}", specs.len()));
    }
    let ids = specs
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, icon)| {
            let id = u16::try_from(index)
                .map_err(|error| format!("icon specification index overflow: {error}"))?;
            Ok((icon, id))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;

    let mut output = String::new();
    output.push_str("// @generated by tools/generate_icon_data.rs; do not edit by hand.\n");
    output.push_str(&format!(
        "// nvim-web-devicons revision {NVIM_REVISION}; Catppuccin Icons v{CATPPUCCIN_VERSION} revision {CATPPUCCIN_REVISION}.\n"
    ));
    output.push_str(&format!(
        "// {} filename aliases, {} extension aliases, {} folder aliases, {} icon specifications.\n\n",
        filenames.len(),
        extensions.len(),
        folders.len(),
        specs.len()
    ));
    output.push_str("use super::{FileColor, Icon};\n\n");
    output.push_str("pub(super) const ICONS: &[Icon] = &[\n");
    for icon in &specs {
        output.push_str(&format!(
            "    Icon {{\n        nerd: {:?},\n        plain: {:?},\n        color: FileColor::{},\n    }},\n",
            icon.nerd,
            icon.plain,
            icon.role.rust_name()
        ));
    }
    output.push_str("];\n\n");
    render_table(&mut output, "FILE_NAMES", filenames, &ids)?;
    render_table(&mut output, "EXTENSIONS", extensions, &ids)?;
    render_table(&mut output, "FOLDERS", folders, &ids)?;
    while output.ends_with("\n\n") {
        output.pop();
    }
    Ok(output)
}

fn render_table(
    output: &mut String,
    name: &str,
    entries: &BTreeMap<String, IconSpec>,
    ids: &BTreeMap<IconSpec, u16>,
) -> Result<(), String> {
    output.push_str(&format!("pub(super) const {name}: &[(&str, u16)] = &[\n"));
    for (key, icon) in entries {
        let id = ids
            .get(icon)
            .ok_or_else(|| format!("missing generated icon ID for {key}"))?;
        output.push_str(&format!("    ({key:?}, {id}),\n"));
    }
    output.push_str("];\n\n");
    Ok(())
}
