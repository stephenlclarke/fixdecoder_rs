// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const AUTO_PATH: &str = "auto";
const FRAGMENT_FILE: &str = "fixdecoder-v1.ghostty";
const FRAGMENT: &str = include_str!("../contrib/ghostty/fixdecoder-v1.ghostty");
const INCLUDE_COMMENT: &str = "# fixdecoder Ghostty pager bindings (managed include)";
const INCLUDE_LINE: &str = "config-file = fixdecoder-v1.ghostty";

const LEGACY_INLINE_BINDINGS: &str = "keybind = home=csi:H
keybind = end=csi:F
keybind = super+page_up=csi:5~
keybind = chain=csi:5~
keybind = chain=csi:5~
keybind = chain=csi:5~
keybind = chain=csi:5~
keybind = super+page_down=csi:6~
keybind = chain=csi:6~
keybind = chain=csi:6~
keybind = chain=csi:6~
keybind = chain=csi:6~";

pub fn install(path_arg: &str, out: &mut dyn Write) -> Result<()> {
    let config_path = resolve_config_path(path_arg)?;
    let existing = match fs::read_to_string(&config_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };

    if existing.contains(LEGACY_INLINE_BINDINGS) {
        writeln!(
            out,
            "Ghostty pager bindings are already configured directly in {}.",
            config_path.display()
        )?;
        write_reload_instruction(out)?;
        return Ok(());
    }

    let config_dir = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;

    let fragment_path = config_dir.join(FRAGMENT_FILE);
    let fragment_changed = install_fragment(&fragment_path)?;
    let include_changed = if includes_fragment(&existing) {
        false
    } else {
        append_include(&config_path, &existing)?;
        true
    };

    if fragment_changed || include_changed {
        writeln!(out, "Installed Ghostty pager bindings:")?;
    } else {
        writeln!(out, "Ghostty pager bindings are already installed:")?;
    }
    writeln!(out, "  config: {}", config_path.display())?;
    writeln!(out, "  fragment: {}", fragment_path.display())?;
    write_reload_instruction(out)?;
    Ok(())
}

fn resolve_config_path(path_arg: &str) -> Result<PathBuf> {
    if path_arg != AUTO_PATH {
        let path = Path::new(path_arg);
        if path == Path::new("~") || path_arg.starts_with("~/") {
            return Ok(expand_home(path, &home_dir()?));
        }
        return Ok(path.to_path_buf());
    }

    let home = home_dir()?;
    let xdg_root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    Ok(default_config_path(
        &home,
        &xdg_root,
        cfg!(target_os = "macos"),
    ))
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set; pass an explicit Ghostty config path"))
}

fn expand_home(path: &Path, home: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if text == "~" {
        return home.to_path_buf();
    }
    if let Some(remainder) = text.strip_prefix("~/") {
        return home.join(remainder);
    }
    path.to_path_buf()
}

fn default_config_path(home: &Path, xdg_root: &Path, macos: bool) -> PathBuf {
    let xdg_dir = xdg_root.join("ghostty");
    let mut candidates = vec![xdg_dir.join("config.ghostty"), xdg_dir.join("config")];
    let default_path = if macos {
        home.join("Library/Application Support/com.mitchellh.ghostty/config.ghostty")
    } else {
        candidates[0].clone()
    };
    if macos {
        let macos_dir = home.join("Library/Application Support/com.mitchellh.ghostty");
        candidates.push(macos_dir.join("config.ghostty"));
        candidates.push(macos_dir.join("config"));
    }

    candidates
        .iter()
        .rev()
        .find(|path| fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0))
        .cloned()
        .or_else(|| candidates.iter().rev().find(|path| path.is_file()).cloned())
        .unwrap_or(default_path)
}

fn install_fragment(path: &Path) -> Result<bool> {
    match fs::read_to_string(path) {
        Ok(existing) if existing == FRAGMENT => Ok(false),
        Ok(_) => bail!(
            "refusing to overwrite modified Ghostty fragment {}; move it aside and retry",
            path.display()
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            fs::write(path, FRAGMENT)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Ok(true)
        }
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn includes_fragment(config: &str) -> bool {
    config.lines().any(|line| {
        let Some((key, value)) = line.trim().split_once('=') else {
            return false;
        };
        key.trim() == "config-file" && value.trim().trim_matches('"') == FRAGMENT_FILE
    })
}

fn append_include(path: &Path, existing: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;

    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file)?;
    }
    if !existing.trim().is_empty() {
        writeln!(file)?;
    }
    writeln!(file, "{INCLUDE_COMMENT}")?;
    writeln!(file, "{INCLUDE_LINE}")?;
    file.flush()
        .with_context(|| format!("failed to flush {}", path.display()))
}

fn write_reload_instruction(out: &mut dyn Write) -> Result<()> {
    writeln!(
        out,
        "Reload Ghostty with Cmd+Shift+, on macOS or Ctrl+Shift+, on Linux."
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_creates_versioned_fragment_and_include() {
        let dir = tempdir().expect("temporary directory");
        let config = dir.path().join("ghostty/config.ghostty");
        let mut output = Vec::new();

        install(config.to_str().expect("config path"), &mut output).expect("install bindings");

        assert_eq!(
            fs::read_to_string(&config).expect("read config"),
            format!("{INCLUDE_COMMENT}\n{INCLUDE_LINE}\n")
        );
        assert_eq!(
            fs::read_to_string(config.parent().unwrap().join(FRAGMENT_FILE))
                .expect("read fragment"),
            FRAGMENT
        );
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Installed Ghostty pager bindings")
        );
    }

    #[test]
    fn install_is_idempotent_and_preserves_existing_config() {
        let dir = tempdir().expect("temporary directory");
        let config = dir.path().join("config");
        fs::write(&config, "theme = Catppuccin Mocha\n").expect("write config");

        install(config.to_str().unwrap(), &mut Vec::new()).expect("first install");
        let installed = fs::read_to_string(&config).expect("read installed config");
        let mut output = Vec::new();
        install(config.to_str().unwrap(), &mut output).expect("second install");

        assert_eq!(fs::read_to_string(&config).unwrap(), installed);
        assert!(installed.starts_with("theme = Catppuccin Mocha\n\n"));
        assert_eq!(installed.matches(INCLUDE_LINE).count(), 1);
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("already installed")
        );
    }

    #[test]
    fn install_recognises_existing_inline_bindings_without_rewriting_config() {
        let dir = tempdir().expect("temporary directory");
        let config = dir.path().join("config");
        let original = format!("theme = dark\n{LEGACY_INLINE_BINDINGS}\nfont-size = 14\n");
        fs::write(&config, &original).expect("write config");
        let mut output = Vec::new();

        install(config.to_str().unwrap(), &mut output).expect("detect inline bindings");

        assert_eq!(fs::read_to_string(&config).unwrap(), original);
        assert!(!config.parent().unwrap().join(FRAGMENT_FILE).exists());
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("already configured directly")
        );
    }

    #[test]
    fn install_refuses_to_overwrite_a_conflicting_fragment() {
        let dir = tempdir().expect("temporary directory");
        let config = dir.path().join("config");
        fs::write(dir.path().join(FRAGMENT_FILE), "user content\n").expect("write conflict");

        let error = install(config.to_str().unwrap(), &mut Vec::new()).unwrap_err();

        assert!(error.to_string().contains("refusing to overwrite"));
        assert!(!config.exists());
    }

    #[test]
    fn default_path_uses_highest_precedence_existing_config() {
        let dir = tempdir().expect("temporary directory");
        let home = dir.path().join("home");
        let xdg = dir.path().join("xdg");
        let xdg_config = xdg.join("ghostty/config");
        let macos_config = home.join("Library/Application Support/com.mitchellh.ghostty/config");
        fs::create_dir_all(xdg_config.parent().unwrap()).expect("create XDG directory");
        fs::create_dir_all(macos_config.parent().unwrap()).expect("create macOS directory");
        fs::write(&xdg_config, "theme = light\n").expect("write XDG config");
        fs::write(&macos_config, "theme = dark\n").expect("write macOS config");

        assert_eq!(default_config_path(&home, &xdg, true), macos_config);
        assert_eq!(default_config_path(&home, &xdg, false), xdg_config);
    }

    #[test]
    fn default_path_ignores_empty_higher_precedence_config() {
        let dir = tempdir().expect("temporary directory");
        let home = dir.path().join("home");
        let xdg = dir.path().join("xdg");
        let xdg_config = xdg.join("ghostty/config");
        let macos_config =
            home.join("Library/Application Support/com.mitchellh.ghostty/config.ghostty");
        fs::create_dir_all(xdg_config.parent().unwrap()).expect("create XDG directory");
        fs::create_dir_all(macos_config.parent().unwrap()).expect("create macOS directory");
        fs::write(&xdg_config, "theme = dark\n").expect("write XDG config");
        fs::write(&macos_config, "").expect("write empty macOS config");

        assert_eq!(default_config_path(&home, &xdg, true), xdg_config);
    }

    #[test]
    fn default_path_uses_platform_preferred_location_when_none_exists() {
        let dir = tempdir().expect("temporary directory");
        let home = dir.path().join("home");
        let xdg = dir.path().join("xdg");

        assert_eq!(
            default_config_path(&home, &xdg, true),
            home.join("Library/Application Support/com.mitchellh.ghostty/config.ghostty")
        );
        assert_eq!(
            default_config_path(&home, &xdg, false),
            xdg.join("ghostty/config.ghostty")
        );
    }

    #[test]
    fn explicit_tilde_path_expands_against_home() {
        let home = Path::new("/users/tester");
        assert_eq!(
            expand_home(Path::new("~/.config/ghostty/config"), home),
            home.join(".config/ghostty/config")
        );
    }
}
