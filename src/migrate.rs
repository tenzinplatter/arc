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

    let moved_config = rename_if_present(&old_config_dir, &new_config_dir, "config dir")?;
    if moved_config {
        rewrite_paths_in_config(&new_config_dir.join("config.toml"))?;
    }

    let old_default_cache = home.join(".cache").join(OLD_NAME);
    let new_default_cache = home.join(".cache").join(NEW_NAME);
    let moved_default_cache =
        rename_if_present(&old_default_cache, &new_default_cache, "cache dir")?;

    let moved_configured_cache = match configured_rename(old_cache_dir_in_config.as_deref()) {
        Some((old_path, new_path)) if old_path != old_default_cache => {
            rename_if_present(&old_path, &new_path, "configured cache dir")?
        }
        _ => false,
    };

    let moved_notes = match configured_rename(old_notes_dir.as_deref()) {
        Some((old_path, new_path)) => rename_if_present(&old_path, &new_path, "notes dir")?,
        None => false,
    };

    if !(moved_config || moved_default_cache || moved_configured_cache || moved_notes) {
        println!("Nothing to migrate.");
    }

    Ok(())
}

fn configured_rename(configured: Option<&str>) -> Option<(PathBuf, PathBuf)> {
    let configured = configured.filter(|p| p.contains(OLD_NAME))?;
    Some((
        expand_tilde(configured),
        expand_tilde(&configured.replace(OLD_NAME, NEW_NAME)),
    ))
}

fn rename_if_present(old: &Path, new: &Path, label: &str) -> Result<bool> {
    if !old.exists() {
        return Ok(false);
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
    Ok(true)
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
    Ok(contents
        .lines()
        .find_map(|line| quoted_value(line.trim(), field))
        .map(str::to_string))
}

fn quoted_value<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    if line.starts_with('#') {
        return None;
    }
    line.strip_prefix(field)?
        .trim_start()
        .strip_prefix('=')?
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')
}

fn expand_tilde(p: &str) -> PathBuf {
    let expanded = shellexpand::full(p)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| p.to_string());
    PathBuf::from(expanded)
}
