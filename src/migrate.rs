use std::{env, fs, path::{Path, PathBuf}};

use anyhow::{Context, Result};

const OLD_NAME: &str = "shortcut-notes";
const NEW_NAME: &str = "arc";

pub fn migrate() -> Result<()> {
    let home = env::home_dir().context("Couldn't find home dir")?;

    let old_config_dir = home.join(".config").join(OLD_NAME);
    let new_config_dir = home.join(".config").join(NEW_NAME);

    // Read before the rename so customised cache_dir/notes_dir can still be found after it.
    let old_config_file = old_config_dir.join("config.toml");
    let old_notes_dir = read_confy_quoted_field(&old_config_file, "notes_dir")?;
    let old_cache_dir_in_config = read_confy_quoted_field(&old_config_file, "cache_dir")?;

    let mut moved_anything = false;

    if rename_if_present(&old_config_dir, &new_config_dir, "config dir")?.is_some() {
        moved_anything = true;
        rewrite_paths_in_config(&new_config_dir.join("config.toml"))?;
    }

    let old_default_cache = home.join(".cache").join(OLD_NAME);
    let new_default_cache = home.join(".cache").join(NEW_NAME);
    if rename_if_present(&old_default_cache, &new_default_cache, "cache dir")?.is_some() {
        moved_anything = true;
    }

    if let Some(configured) = old_cache_dir_in_config.as_deref().filter(|p| p.contains(OLD_NAME)) {
        let old_path = expand_tilde(configured);
        let new_path = expand_tilde(&configured.replace(OLD_NAME, NEW_NAME));
        if old_path != old_default_cache
            && rename_if_present(&old_path, &new_path, "configured cache dir")?.is_some()
        {
            moved_anything = true;
        }
    }

    if let Some(configured) = old_notes_dir.as_deref().filter(|p| p.contains(OLD_NAME)) {
        let old_path = expand_tilde(configured);
        let new_path = expand_tilde(&configured.replace(OLD_NAME, NEW_NAME));
        if rename_if_present(&old_path, &new_path, "notes dir")?.is_some() {
            moved_anything = true;
        }
    }

    if !moved_anything {
        println!("Nothing to migrate.");
    }

    Ok(())
}

fn rename_if_present(old: &Path, new: &Path, label: &str) -> Result<Option<()>> {
    if !old.exists() {
        return Ok(None);
    }
    if new.exists() {
        anyhow::bail!(
            "Cannot migrate {label}: target {} already exists. Move it aside and retry.",
            new.display()
        );
    }
    fs::rename(old, new)
        .with_context(|| format!("Failed to rename {} -> {}", old.display(), new.display()))?;
    println!("Moved {} -> {}", old.display(), new.display());
    Ok(Some(()))
}

fn rewrite_paths_in_config(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    if !contents.contains(OLD_NAME) {
        return Ok(());
    }
    let updated = contents.replace(OLD_NAME, NEW_NAME);
    fs::write(path, &updated)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    println!("Rewrote paths in {}", path.display());
    Ok(())
}

fn read_confy_quoted_field(config_path: &Path, field: &str) -> Result<Option<String>> {
    if !config_path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix(field) else { continue };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim();
        if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            return Ok(Some(inner.to_string()));
        }
    }
    Ok(None)
}

fn expand_tilde(p: &str) -> PathBuf {
    let expanded = shellexpand::full(p)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| p.to_string());
    PathBuf::from(expanded)
}
