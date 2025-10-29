use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use directories::BaseDirs;
use serde::Deserialize;

const CONFIG_FILE_NAME: &str = "config.toml";
const APP_NAME: &str = "rldx";

#[derive(Debug, Clone)]
pub struct Config {
    pub config_path: PathBuf,
    pub vdir: PathBuf,
    pub fields_first_pane: Vec<String>,
    pub keys: Keys,
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
    #[serde(default)]
    keys: Keys,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            vdir: None,
            fields_first_pane: default_fields_first_pane(),
            keys: Keys::default(),
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
        fs::create_dir_all(&dir).with_context(|| format!("failed to create config dir: {}", dir.display()))?;
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

    Ok(Config {
        config_path: path,
        vdir,
        fields_first_pane: cfg_file.fields_first_pane,
        keys: cfg_file.keys,
    })
}

fn warn_unknown_keys(value: &toml::Value) {
    let Some(table) = value.as_table() else { return; };

    let known = HashSet::from([
        "vdir".to_string(),
        "fields_first_pane".to_string(),
        "keys".to_string(),
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
}

pub fn config_exists() -> Result<bool> {
    Ok(config_path()?.exists())
}