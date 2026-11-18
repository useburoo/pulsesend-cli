//! Configuration locale : clé API et URL de l'API, dans ~/.config/pulsesend/config.json (0600).
//! Les variables PULSESEND_API_KEY et PULSESEND_BASE_URL, ou les options --api-key / --base-url,
//! passent avant le fichier.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_BASE_URL: &str = "https://send.noburi.space";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

pub fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("could not find the config directory")?;

    Ok(dir.join("pulsesend").join("config.json"))
}

pub fn load() -> Result<Config> {
    let path = path()?;

    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).with_context(|| format!("{} is not valid JSON", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn save(config: &Config) -> Result<PathBuf> {
    let path = path()?;
    let dir = path.parent().expect("le chemin a un dossier parent");
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::write(&path, serde_json::to_string_pretty(config)? + "\n").with_context(|| format!("writing {}", path.display()))?;
    restrict(&path)?;

    Ok(path)
}

pub fn delete() -> Result<bool> {
    let path = path()?;

    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("deleting {}", path.display())),
    }
}

/// La clé API ne doit être lisible que par son propriétaire.
#[cfg(unix)]
fn restrict(path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

#[cfg(not(unix))]
fn restrict(_path: &PathBuf) -> Result<()> {
    Ok(())
}

/// « ps_live_abcd…wxyz » : assez pour reconnaître une clé, jamais assez pour la réutiliser.
pub fn mask(key: &str) -> String {
    let prefix = if key.starts_with("ps_live_") {
        "ps_live_"
    } else if key.starts_with("ps_test_") {
        "ps_test_"
    } else {
        ""
    };
    let rest = &key[prefix.len()..];

    if rest.len() <= 8 {
        return format!("{prefix}…");
    }

    format!("{prefix}{}…{}", &rest[..4], &rest[rest.len() - 4..])
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn masks_keys() {
        assert_eq!(mask("ps_live_abcdefghijklmnop"), "ps_live_abcd…mnop");
        assert_eq!(mask("ps_test_short"), "ps_test_…");
        assert_eq!(mask("autre_cle_1234567890"), "autr…7890");
    }
}
