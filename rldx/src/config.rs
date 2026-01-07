use std::collections::{HashMap, HashSet};
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
    pub height: u16,
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

// =============================================================================
// Key Bindings - Context-aware with multiple bindings per action
// =============================================================================

/// All key bindings organized by context
#[derive(Debug, Clone)]
pub struct Keys {
    /// Global keys (work in most contexts)
    pub global: GlobalKeys,
    /// Keys for search input mode
    pub search_input: SearchInputKeys,
    /// Keys for search results navigation
    pub search_results: SearchResultsKeys,
    /// Keys for card/detail pane navigation
    pub navigation: NavigationKeys,
    /// Keys for modal dialogs
    pub modal: ModalKeys,
    /// Keys for inline editing
    pub editor: EditorKeys,
}

#[derive(Debug, Clone)]
pub struct GlobalKeys {
    pub quit: Vec<String>,
    pub search: Vec<String>,
    pub help: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SearchInputKeys {
    pub cancel: Vec<String>,
    pub confirm: Vec<String>,
    pub next: Vec<String>,
    pub prev: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SearchResultsKeys {
    pub cancel: Vec<String>,
    pub confirm: Vec<String>,
    pub next: Vec<String>,
    pub prev: Vec<String>,
    pub page_down: Vec<String>,
    pub page_up: Vec<String>,
    pub mark: Vec<String>,
    pub merge: Vec<String>,
    pub toggle_marked: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NavigationKeys {
    pub next: Vec<String>,
    pub prev: Vec<String>,
    pub tab_next: Vec<String>,
    pub tab_prev: Vec<String>,
    pub edit: Vec<String>,
    pub copy: Vec<String>,
    pub confirm: Vec<String>,
    pub add_alias: Vec<String>,
    pub photo_fetch: Vec<String>,
    pub lang_cycle: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ModalKeys {
    pub cancel: Vec<String>,
    pub confirm: Vec<String>,
    pub next: Vec<String>,
    pub prev: Vec<String>,
    pub edit: Vec<String>,
    pub copy: Vec<String>,
    pub set_default: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EditorKeys {
    pub cancel: Vec<String>,
    pub confirm: Vec<String>,
}

// =============================================================================
// Default implementations
// =============================================================================

impl Default for Keys {
    fn default() -> Self {
        Self {
            global: GlobalKeys::default(),
            search_input: SearchInputKeys::default(),
            search_results: SearchResultsKeys::default(),
            navigation: NavigationKeys::default(),
            modal: ModalKeys::default(),
            editor: EditorKeys::default(),
        }
    }
}

impl Default for GlobalKeys {
    fn default() -> Self {
        Self {
            quit: vec!["q".into()],
            search: vec!["/".into()],
            help: vec!["F1".into(), "?".into()],
        }
    }
}

impl Default for SearchInputKeys {
    fn default() -> Self {
        Self {
            cancel: vec!["Escape".into()],
            confirm: vec!["Enter".into()],
            next: vec!["Tab".into()],
            prev: vec!["Backtab".into()],
        }
    }
}

impl Default for SearchResultsKeys {
    fn default() -> Self {
        Self {
            cancel: vec!["Escape".into()],
            confirm: vec!["Enter".into()],
            next: vec!["j".into(), "Down".into(), "Tab".into()],
            prev: vec!["k".into(), "Up".into(), "Backtab".into()],
            page_down: vec!["PageDown".into()],
            page_up: vec!["PageUp".into()],
            mark: vec!["Space".into()],
            merge: vec!["m".into()],
            toggle_marked: vec!["M".into()],
        }
    }
}

impl Default for NavigationKeys {
    fn default() -> Self {
        Self {
            next: vec!["j".into(), "Down".into(), "Tab".into()],
            prev: vec!["k".into(), "Up".into(), "Backtab".into()],
            tab_next: vec!["l".into(), "Right".into()],
            tab_prev: vec!["h".into(), "Left".into()],
            edit: vec!["e".into()],
            copy: vec!["y".into(), "Space".into()],
            confirm: vec!["Enter".into()],
            add_alias: vec!["a".into()],
            photo_fetch: vec!["i".into()],
            lang_cycle: vec!["L".into()],
        }
    }
}

impl Default for ModalKeys {
    fn default() -> Self {
        Self {
            cancel: vec!["Escape".into(), "q".into()],
            confirm: vec!["Enter".into(), "y".into()],
            next: vec!["j".into(), "Down".into(), "Tab".into()],
            prev: vec!["k".into(), "Up".into(), "Backtab".into()],
            edit: vec!["e".into()],
            copy: vec!["y".into(), "Space".into()],
            set_default: vec!["d".into()],
        }
    }
}

impl Default for EditorKeys {
    fn default() -> Self {
        Self {
            cancel: vec!["Escape".into()],
            confirm: vec!["Enter".into()],
        }
    }
}

// =============================================================================
// Serde deserialization types (support both single string and array)
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum KeyBinding {
    Single(String),
    Multiple(Vec<String>),
}

impl KeyBinding {
    fn into_vec(self) -> Vec<String> {
        match self {
            KeyBinding::Single(s) => vec![s],
            KeyBinding::Multiple(v) => v,
        }
    }
}

impl Default for KeyBinding {
    fn default() -> Self {
        KeyBinding::Multiple(vec![])
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct KeysFile {
    global: GlobalKeysFile,
    search_input: SearchInputKeysFile,
    search_results: SearchResultsKeysFile,
    navigation: NavigationKeysFile,
    modal: ModalKeysFile,
    editor: EditorKeysFile,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct GlobalKeysFile {
    quit: KeyBinding,
    search: KeyBinding,
    help: KeyBinding,
}

impl Default for GlobalKeysFile {
    fn default() -> Self {
        let defaults = GlobalKeys::default();
        Self {
            quit: KeyBinding::Multiple(defaults.quit),
            search: KeyBinding::Multiple(defaults.search),
            help: KeyBinding::Multiple(defaults.help),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct SearchInputKeysFile {
    cancel: KeyBinding,
    confirm: KeyBinding,
    next: KeyBinding,
    prev: KeyBinding,
}

impl Default for SearchInputKeysFile {
    fn default() -> Self {
        let defaults = SearchInputKeys::default();
        Self {
            cancel: KeyBinding::Multiple(defaults.cancel),
            confirm: KeyBinding::Multiple(defaults.confirm),
            next: KeyBinding::Multiple(defaults.next),
            prev: KeyBinding::Multiple(defaults.prev),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct SearchResultsKeysFile {
    cancel: KeyBinding,
    confirm: KeyBinding,
    next: KeyBinding,
    prev: KeyBinding,
    page_down: KeyBinding,
    page_up: KeyBinding,
    mark: KeyBinding,
    merge: KeyBinding,
    toggle_marked: KeyBinding,
}

impl Default for SearchResultsKeysFile {
    fn default() -> Self {
        let defaults = SearchResultsKeys::default();
        Self {
            cancel: KeyBinding::Multiple(defaults.cancel),
            confirm: KeyBinding::Multiple(defaults.confirm),
            next: KeyBinding::Multiple(defaults.next),
            prev: KeyBinding::Multiple(defaults.prev),
            page_down: KeyBinding::Multiple(defaults.page_down),
            page_up: KeyBinding::Multiple(defaults.page_up),
            mark: KeyBinding::Multiple(defaults.mark),
            merge: KeyBinding::Multiple(defaults.merge),
            toggle_marked: KeyBinding::Multiple(defaults.toggle_marked),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct NavigationKeysFile {
    next: KeyBinding,
    prev: KeyBinding,
    tab_next: KeyBinding,
    tab_prev: KeyBinding,
    edit: KeyBinding,
    copy: KeyBinding,
    confirm: KeyBinding,
    add_alias: KeyBinding,
    photo_fetch: KeyBinding,
    lang_cycle: KeyBinding,
}

impl Default for NavigationKeysFile {
    fn default() -> Self {
        let defaults = NavigationKeys::default();
        Self {
            next: KeyBinding::Multiple(defaults.next),
            prev: KeyBinding::Multiple(defaults.prev),
            tab_next: KeyBinding::Multiple(defaults.tab_next),
            tab_prev: KeyBinding::Multiple(defaults.tab_prev),
            edit: KeyBinding::Multiple(defaults.edit),
            copy: KeyBinding::Multiple(defaults.copy),
            confirm: KeyBinding::Multiple(defaults.confirm),
            add_alias: KeyBinding::Multiple(defaults.add_alias),
            photo_fetch: KeyBinding::Multiple(defaults.photo_fetch),
            lang_cycle: KeyBinding::Multiple(defaults.lang_cycle),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ModalKeysFile {
    cancel: KeyBinding,
    confirm: KeyBinding,
    next: KeyBinding,
    prev: KeyBinding,
    edit: KeyBinding,
    copy: KeyBinding,
    set_default: KeyBinding,
}

impl Default for ModalKeysFile {
    fn default() -> Self {
        let defaults = ModalKeys::default();
        Self {
            cancel: KeyBinding::Multiple(defaults.cancel),
            confirm: KeyBinding::Multiple(defaults.confirm),
            next: KeyBinding::Multiple(defaults.next),
            prev: KeyBinding::Multiple(defaults.prev),
            edit: KeyBinding::Multiple(defaults.edit),
            copy: KeyBinding::Multiple(defaults.copy),
            set_default: KeyBinding::Multiple(defaults.set_default),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct EditorKeysFile {
    cancel: KeyBinding,
    confirm: KeyBinding,
}

impl Default for EditorKeysFile {
    fn default() -> Self {
        let defaults = EditorKeys::default();
        Self {
            cancel: KeyBinding::Multiple(defaults.cancel),
            confirm: KeyBinding::Multiple(defaults.confirm),
        }
    }
}

// =============================================================================
// Conversion from file types to runtime types
// =============================================================================

impl From<KeysFile> for Keys {
    fn from(file: KeysFile) -> Self {
        Self {
            global: file.global.into(),
            search_input: file.search_input.into(),
            search_results: file.search_results.into(),
            navigation: file.navigation.into(),
            modal: file.modal.into(),
            editor: file.editor.into(),
        }
    }
}

impl From<GlobalKeysFile> for GlobalKeys {
    fn from(file: GlobalKeysFile) -> Self {
        Self {
            quit: file.quit.into_vec(),
            search: file.search.into_vec(),
            help: file.help.into_vec(),
        }
    }
}

impl From<SearchInputKeysFile> for SearchInputKeys {
    fn from(file: SearchInputKeysFile) -> Self {
        Self {
            cancel: file.cancel.into_vec(),
            confirm: file.confirm.into_vec(),
            next: file.next.into_vec(),
            prev: file.prev.into_vec(),
        }
    }
}

impl From<SearchResultsKeysFile> for SearchResultsKeys {
    fn from(file: SearchResultsKeysFile) -> Self {
        Self {
            cancel: file.cancel.into_vec(),
            confirm: file.confirm.into_vec(),
            next: file.next.into_vec(),
            prev: file.prev.into_vec(),
            page_down: file.page_down.into_vec(),
            page_up: file.page_up.into_vec(),
            mark: file.mark.into_vec(),
            merge: file.merge.into_vec(),
            toggle_marked: file.toggle_marked.into_vec(),
        }
    }
}

impl From<NavigationKeysFile> for NavigationKeys {
    fn from(file: NavigationKeysFile) -> Self {
        Self {
            next: file.next.into_vec(),
            prev: file.prev.into_vec(),
            tab_next: file.tab_next.into_vec(),
            tab_prev: file.tab_prev.into_vec(),
            edit: file.edit.into_vec(),
            copy: file.copy.into_vec(),
            confirm: file.confirm.into_vec(),
            add_alias: file.add_alias.into_vec(),
            photo_fetch: file.photo_fetch.into_vec(),
            lang_cycle: file.lang_cycle.into_vec(),
        }
    }
}

impl From<ModalKeysFile> for ModalKeys {
    fn from(file: ModalKeysFile) -> Self {
        Self {
            cancel: file.cancel.into_vec(),
            confirm: file.confirm.into_vec(),
            next: file.next.into_vec(),
            prev: file.prev.into_vec(),
            edit: file.edit.into_vec(),
            copy: file.copy.into_vec(),
            set_default: file.set_default.into_vec(),
        }
    }
}

impl From<EditorKeysFile> for EditorKeys {
    fn from(file: EditorKeysFile) -> Self {
        Self {
            cancel: file.cancel.into_vec(),
            confirm: file.confirm.into_vec(),
        }
    }
}

// =============================================================================
// Key binding validation
// =============================================================================

/// Normalize a key binding string to a canonical form for collision detection.
/// Single characters preserve case (since 'M' means Shift+m, different from 'm').
/// Multi-character key names are case-insensitive (Enter, ENTER, enter are the same).
fn normalize_binding(binding: &str) -> String {
    let trimmed = binding.trim();
    if trimmed.len() == 1 {
        // Single character: preserve case (m != M)
        trimmed.to_string()
    } else {
        // Special key names: case-insensitive
        trimmed.to_ascii_lowercase()
    }
}

/// Check for collisions within a single context
fn check_context_collisions(
    bindings: &[(&str, &[String])],
    context_name: &str,
) -> Result<()> {
    let mut seen: HashMap<String, &str> = HashMap::new();

    for (action_name, keys) in bindings {
        for key in *keys {
            let normalized = normalize_binding(key);
            if normalized.is_empty() {
                continue;
            }
            if let Some(existing_action) = seen.get(&normalized) {
                bail!(
                    "key binding collision in [keys.{}]: '{}' is bound to both '{}' and '{}'",
                    context_name,
                    key,
                    existing_action,
                    action_name
                );
            }
            seen.insert(normalized, action_name);
        }
    }

    Ok(())
}

/// Validate all key bindings for collisions within each context
fn validate_key_bindings(keys: &Keys) -> Result<()> {
    // Global context
    check_context_collisions(
        &[
            ("quit", &keys.global.quit),
            ("search", &keys.global.search),
            ("help", &keys.global.help),
        ],
        "global",
    )?;

    // Search input context
    check_context_collisions(
        &[
            ("cancel", &keys.search_input.cancel),
            ("confirm", &keys.search_input.confirm),
        ],
        "search_input",
    )?;

    // Search results context
    check_context_collisions(
        &[
            ("cancel", &keys.search_results.cancel),
            ("confirm", &keys.search_results.confirm),
            ("next", &keys.search_results.next),
            ("prev", &keys.search_results.prev),
            ("page_down", &keys.search_results.page_down),
            ("page_up", &keys.search_results.page_up),
            ("mark", &keys.search_results.mark),
            ("merge", &keys.search_results.merge),
            ("toggle_marked", &keys.search_results.toggle_marked),
        ],
        "search_results",
    )?;

    // Navigation context
    check_context_collisions(
        &[
            ("next", &keys.navigation.next),
            ("prev", &keys.navigation.prev),
            ("tab_next", &keys.navigation.tab_next),
            ("tab_prev", &keys.navigation.tab_prev),
            ("edit", &keys.navigation.edit),
            ("copy", &keys.navigation.copy),
            ("confirm", &keys.navigation.confirm),
            ("add_alias", &keys.navigation.add_alias),
            ("photo_fetch", &keys.navigation.photo_fetch),
            ("lang_cycle", &keys.navigation.lang_cycle),
        ],
        "navigation",
    )?;

    // Modal context
    check_context_collisions(
        &[
            ("cancel", &keys.modal.cancel),
            ("confirm", &keys.modal.confirm),
            ("next", &keys.modal.next),
            ("prev", &keys.modal.prev),
            ("edit", &keys.modal.edit),
            ("copy", &keys.modal.copy),
            ("set_default", &keys.modal.set_default),
        ],
        "modal",
    )?;

    // Editor context
    check_context_collisions(
        &[
            ("cancel", &keys.editor.cancel),
            ("confirm", &keys.editor.confirm),
        ],
        "editor",
    )?;

    Ok(())
}

// =============================================================================
// Config file structure
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ConfigFile {
    vdir: Option<PathBuf>,
    #[serde(default = "default_fields_first_pane")]
    fields_first_pane: Vec<String>,
    phone_region: Option<String>,
    #[serde(default)]
    keys: KeysFile,
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
            keys: KeysFile::default(),
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

    let keys: Keys = cfg_file.keys.into();

    // Validate key bindings for collisions
    validate_key_bindings(&keys)?;

    Ok(Config {
        config_path: path,
        vdir,
        fields_first_pane: cfg_file.fields_first_pane,
        phone_region,
        keys,
        ui: cfg_file.ui.into(),
        commands: cfg_file.commands.into(),
    })
}

// =============================================================================
// Unknown key warnings
// =============================================================================

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
        warn_unknown_keys_section(keys_val);
    }

    if let Some(ui_val) = table.get("ui") {
        warn_unknown_ui_keys(ui_val);
    }

    if let Some(commands_val) = table.get("commands") {
        warn_unknown_commands_keys(commands_val);
    }
}

fn warn_unknown_keys_section(value: &toml::Value) {
    let Some(table) = value.as_table() else {
        return;
    };

    let known_contexts = HashSet::from([
        "global",
        "search_input",
        "search_results",
        "navigation",
        "modal",
        "editor",
    ]);

    for key in table.keys() {
        if !known_contexts.contains(key.as_str()) {
            eprintln!("warning: unknown keys.* context `{}`", key);
        }
    }

    if let Some(v) = table.get("global") {
        warn_unknown_in_context(v, "global", &["quit", "search", "help"]);
    }
    if let Some(v) = table.get("search_input") {
        warn_unknown_in_context(v, "search_input", &["cancel", "confirm"]);
    }
    if let Some(v) = table.get("search_results") {
        warn_unknown_in_context(
            v,
            "search_results",
            &[
                "cancel",
                "confirm",
                "next",
                "prev",
                "page_down",
                "page_up",
                "mark",
                "merge",
                "toggle_marked",
            ],
        );
    }
    if let Some(v) = table.get("navigation") {
        warn_unknown_in_context(
            v,
            "navigation",
            &[
                "next",
                "prev",
                "tab_next",
                "tab_prev",
                "edit",
                "copy",
                "confirm",
                "add_alias",
                "photo_fetch",
                "lang_cycle",
            ],
        );
    }
    if let Some(v) = table.get("modal") {
        warn_unknown_in_context(
            v,
            "modal",
            &["cancel", "confirm", "next", "prev", "edit", "copy", "set_default"],
        );
    }
    if let Some(v) = table.get("editor") {
        warn_unknown_in_context(v, "editor", &["cancel", "confirm"]);
    }
}

fn warn_unknown_in_context(value: &toml::Value, context: &str, known: &[&str]) {
    let Some(table) = value.as_table() else {
        return;
    };
    let known_set: HashSet<&str> = known.iter().copied().collect();
    for key in table.keys() {
        if !known_set.contains(key.as_str()) {
            eprintln!("warning: unknown keys.{}.* entry `{}`", context, key);
        }
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
    let known = HashSet::from(["width".to_string(), "height".to_string()]);
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

// =============================================================================
// UI config types
// =============================================================================

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
    height: u16,
}

impl Default for UiPaneImageFile {
    fn default() -> Self {
        Self {
            width: 40,
            height: 12,
        }
    }
}

impl From<UiFile> for UiConfig {
    fn from(file: UiFile) -> Self {
        let image_width = if file.pane.image.width == 0 {
            40
        } else {
            file.pane.image.width
        };
        let image_height = if file.pane.image.height == 0 {
            12
        } else {
            file.pane.image.height
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
                    height: image_height,
                },
            },
        }
    }
}

// =============================================================================
// Commands config
// =============================================================================

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
