use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use directories::BaseDirs;
use serde::de::Deserializer;
use serde::Deserialize;

const CONFIG_FILE_NAME: &str = "config.toml";
const APP_NAME: &str = "rldx";

#[derive(Debug, Clone)]
pub struct Config {
    pub config_path: PathBuf,
    pub vdir: PathBuf,
    pub fields_first_pane: Vec<String>,
    pub phone_region: Option<String>,
    pub keys: Keys,
    pub ui: UiConfig,
    pub commands: Commands,
}

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub colors: UiColors,
    pub icons: UiIcons,
    pub pane: UiPane,
}

#[derive(Debug, Clone)]
pub struct UiColors {
    pub border: RgbColor,
    pub selection_bg: RgbColor,
    pub selection_fg: RgbColor,
    pub separator: RgbColor,
    pub status_fg: RgbColor,
    pub status_bg: RgbColor,
}

#[derive(Debug, Clone)]
pub struct UiIcons {
    pub address_book: String,
    pub contact: String,
    pub organization: String,
}

#[derive(Debug, Clone)]
pub struct UiPane {
    pub image: UiPaneImage,
}

#[derive(Debug, Clone)]
pub struct UiPaneImage {
    pub width: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone)]
pub struct Commands {
    pub copy: Option<CommandExec>,
}

#[derive(Debug, Clone)]
pub struct CommandExec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub toggle_search: String,
    pub confirm: String,
    pub quit: String,
    pub next: String,
    pub prev: String,
    pub edit: String,
    pub photo_fetch: String,
    pub lang_next: String,
    pub tab_next: String,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            toggle_search: "/".to_string(),
            confirm: "Enter".to_string(),
            quit: "q".to_string(),
            next: "j".to_string(),
            prev: "k".to_string(),
            edit: "e".to_string(),
            photo_fetch: "i".to_string(),
            lang_next: "L".to_string(),
            tab_next: "Tab".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ConfigFile {
    vdir: Option<PathBuf>,
    #[serde(default = "default_fields_first_pane")]
    fields_first_pane: Vec<String>,
    phone_region: Option<String>,
    #[serde(default)]
    keys: Keys,
    #[serde(default)]
    ui: UiFile,
    #[serde(default)]
    commands: CommandsFile,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            vdir: None,
            fields_first_pane: default_fields_first_pane(),
            phone_region: None,
            keys: Keys::default(),
            ui: UiFile::default(),
            commands: CommandsFile::default(),
        }
    }
}

fn default_fields_first_pane() -> Vec<String> {
    vec![
        "fname".to_string(),
        "mname".to_string(),
        "lname".to_string(),
        "alias".to_string(),
        "phone".to_string(),
        "email".to_string(),
    ]
}

fn config_root() -> Result<PathBuf> {
    let base = BaseDirs::new().context("unable to determine base directories")?;
    let dir = base.config_dir().join(APP_NAME);
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_root()?.join(CONFIG_FILE_NAME))
}

pub fn ensure_config_dir() -> Result<()> {
    let dir = config_root()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create config dir: {}", dir.display()))?;
    }
    Ok(())
}

pub fn load() -> Result<Config> {
    ensure_config_dir()?;
    let path = config_path()?;
    if !path.exists() {
        bail!(
            "configuration file not found at {}. Please create it as per docs.",
            path.display()
        );
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read configuration file at {}", path.display()))?;

    let value: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {} as TOML", path.display()))?;

    warn_unknown_keys(&value);

    let cfg_file: ConfigFile = value
        .try_into()
        .with_context(|| format!("failed to deserialize config from {}", path.display()))?;

    let vdir = cfg_file
        .vdir
        .ok_or_else(|| anyhow!("`vdir` must be specified in configuration"))?;

    if !vdir.exists() {
        bail!("configured vdir does not exist: {}", vdir.display());
    }

    let phone_region = cfg_file
        .phone_region
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase());

    Ok(Config {
        config_path: path,
        vdir,
        fields_first_pane: cfg_file.fields_first_pane,
        phone_region,
        keys: cfg_file.keys,
        ui: cfg_file.ui.into(),
        commands: cfg_file.commands.into(),
    })
}

fn warn_unknown_keys(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };

    let known = HashSet::from([
        "vdir".to_string(),
        "fields_first_pane".to_string(),
        "phone_region".to_string(),
        "keys".to_string(),
        "ui".to_string(),
        "commands".to_string(),
    ]);

    for key in table.keys() {
        if !known.contains(key) {
            eprintln!("warning: unknown configuration key `{}`", key);
        }
    }

    if let Some(keys_val) = table.get("keys") {
        if let Some(keys_table) = keys_val.as_table() {
            let key_known = HashSet::from([
                "toggle_search".to_string(),
                "confirm".to_string(),
                "quit".to_string(),
                "next".to_string(),
                "prev".to_string(),
                "edit".to_string(),
                "photo_fetch".to_string(),
                "lang_next".to_string(),
                "tab_next".to_string(),
            ]);
            for key in keys_table.keys() {
                if !key_known.contains(key) {
                    eprintln!("warning: unknown keys.* entry `{}`", key);
                }
            }
        }
    }

    if let Some(ui_val) = table.get("ui") {
        warn_unknown_ui_keys(ui_val);
    }

    if let Some(commands_val) = table.get("commands") {
        warn_unknown_commands_keys(commands_val);
    }
}

fn warn_unknown_ui_keys(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };

    let known = HashSet::from([
        "colors".to_string(),
        "icons".to_string(),
        "pane".to_string(),
    ]);

    for key in table.keys() {
        if !known.contains(key) {
            eprintln!("warning: unknown ui.* entry `{}`", key);
        }
    }

    if let Some(colors_val) = table.get("colors") {
        warn_unknown_ui_colors(colors_val);
    }
    if let Some(icons_val) = table.get("icons") {
        warn_unknown_ui_icons(icons_val);
    }
    if let Some(pane_val) = table.get("pane") {
        warn_unknown_ui_pane(pane_val);
    }
}

fn warn_unknown_ui_colors(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };
    let known = HashSet::from([
        "border".to_string(),
        "selection_bg".to_string(),
        "selection_fg".to_string(),
        "separator".to_string(),
        "status_fg".to_string(),
        "status_bg".to_string(),
    ]);
    for key in table.keys() {
        if !known.contains(key) {
            eprintln!("warning: unknown ui.colors entry `{}`", key);
        }
    }
}

fn warn_unknown_ui_icons(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };
    let known = HashSet::from([
        "address_book".to_string(),
        "contact".to_string(),
        "organization".to_string(),
    ]);
    for key in table.keys() {
        if !known.contains(key) {
            eprintln!("warning: unknown ui.icons entry `{}`", key);
        }
    }
}

fn warn_unknown_ui_pane(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };
    let known = HashSet::from(["image".to_string()]);
    for key in table.keys() {
        if !known.contains(key) {
            eprintln!("warning: unknown ui.pane entry `{}`", key);
        }
    }

    if let Some(image_val) = table.get("image") {
        warn_unknown_ui_pane_image(image_val);
    }
}

fn warn_unknown_ui_pane_image(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };
    let known = HashSet::from(["width".to_string()]);
    for key in table.keys() {
        if !known.contains(key) {
            eprintln!("warning: unknown ui.pane.image entry `{}`", key);
        }
    }
}

fn warn_unknown_commands_keys(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };
    let known = HashSet::from(["copy".to_string()]);
    for key in table.keys() {
        if !known.contains(key) {
            eprintln!("warning: unknown commands entry `{}`", key);
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct UiFile {
    colors: UiColorsFile,
    icons: UiIconsFile,
    pane: UiPaneFile,
}

impl Default for UiFile {
    fn default() -> Self {
        Self {
            colors: UiColorsFile::default(),
            icons: UiIconsFile::default(),
            pane: UiPaneFile::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct UiColorsFile {
    border: RgbColor,
    selection_bg: RgbColor,
    selection_fg: RgbColor,
    separator: RgbColor,
    status_fg: RgbColor,
    status_bg: RgbColor,
}

impl Default for UiColorsFile {
    fn default() -> Self {
        Self {
            border: RgbColor::new(255, 165, 0),
            selection_bg: RgbColor::new(255, 165, 0),
            selection_fg: RgbColor::new(0, 0, 0),
            separator: RgbColor::new(255, 165, 0),
            status_fg: RgbColor::new(255, 165, 0),
            status_bg: RgbColor::new(0, 0, 0),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct UiIconsFile {
    address_book: String,
    contact: String,
    organization: String,
}

impl Default for UiIconsFile {
    fn default() -> Self {
        Self {
            address_book: "@".to_string(),
            contact: "👤 ".to_string(),
            organization: "🏢 ".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct UiPaneFile {
    image: UiPaneImageFile,
}

impl Default for UiPaneFile {
    fn default() -> Self {
        Self {
            image: UiPaneImageFile::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct UiPaneImageFile {
    width: u16,
}

impl Default for UiPaneImageFile {
    fn default() -> Self {
        Self { width: 40 }
    }
}

impl From<UiFile> for UiConfig {
    fn from(file: UiFile) -> Self {
        let image_width = if file.pane.image.width == 0 {
            40
        } else {
            file.pane.image.width
        };
        Self {
            colors: UiColors {
                border: file.colors.border,
                selection_bg: file.colors.selection_bg,
                selection_fg: file.colors.selection_fg,
                separator: file.colors.separator,
                status_fg: file.colors.status_fg,
                status_bg: file.colors.status_bg,
            },
            icons: UiIcons {
                address_book: file.icons.address_book,
                contact: file.icons.contact,
                organization: file.icons.organization,
            },
            pane: UiPane {
                image: UiPaneImage {
                    width: image_width,
                },
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct CommandsFile {
    copy: Option<CommandDef>,
}

impl Default for CommandsFile {
    fn default() -> Self {
        Self { copy: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CommandDef {
    Simple(String),
    List(Vec<String>),
}

impl From<CommandsFile> for Commands {
    fn from(file: CommandsFile) -> Self {
        Self {
            copy: file.copy.and_then(CommandExec::from_def),
        }
    }
}

impl CommandExec {
    fn from_def(def: CommandDef) -> Option<Self> {
        match def {
            CommandDef::Simple(cmd) => {
                let trimmed = cmd.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(Self {
                        program: trimmed.to_string(),
                        args: Vec::new(),
                    })
                }
            }
            CommandDef::List(mut parts) => {
                if parts.is_empty() {
                    return None;
                }
                let program = parts.remove(0);
                Some(Self {
                    program,
                    args: parts,
                })
            }
        }
    }
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl<'de> serde::Deserialize<'de> for RgbColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Array([u8; 3]),
            Map { r: u8, g: u8, b: u8 },
        }

        let helper = Helper::deserialize(deserializer)?;
        let (r, g, b) = match helper {
            Helper::Array(values) => (values[0], values[1], values[2]),
            Helper::Map { r, g, b } => (r, g, b),
        };
        Ok(RgbColor { r, g, b })
    }
}
