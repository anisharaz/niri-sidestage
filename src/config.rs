//! Settings. Precedence, highest first: environment variable, config file, auto-detection (gap
//! only), built-in default.
//!
//! The config file is `$XDG_CONFIG_HOME/niri-sidestage/config.toml` (usually
//! `~/.config/niri-sidestage/config.toml`), a flat list of `key = value` lines (a subset of TOML).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Windows,
    Workspaces,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Which view the strip opens in.
    pub view: View,
    pub side: Side,
    /// Width in px of the invisible edge trigger; should equal niri's `gaps`.
    pub gap: i32,
    pub panel_width: i32,
    pub hide_delay: Duration,
    pub slide_ms: f64,
    /// The edge trigger is as tall as the panel but never shorter than this.
    pub min_trigger: i32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            view: View::Windows,
            side: Side::Left,
            gap: 5,
            panel_width: 152,
            hide_delay: Duration::from_millis(250),
            slide_ms: 170.0,
            min_trigger: 200,
        }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn get() -> &'static Config {
    CONFIG.get_or_init(load)
}

pub fn config_path() -> PathBuf {
    xdg_config_home().join("niri-sidestage").join("config.toml")
}

fn xdg_config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

/// System-wide config files, lowest priority first: the `niri-sidestage/config.toml` under each
/// directory of `$XDG_CONFIG_DIRS` (default `/etc/xdg`), with earlier directories winning.
fn system_config_files() -> Vec<PathBuf> {
    let dirs = std::env::var("XDG_CONFIG_DIRS").ok().filter(|d| !d.is_empty());
    let dirs = dirs.as_deref().unwrap_or("/etc/xdg");
    let mut files: Vec<PathBuf> = dirs
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("niri-sidestage").join("config.toml"))
        .collect();
    files.reverse();
    files
}

fn load() -> Config {
    let mut cfg = Config::default();
    let mut gap_set = false;

    // System defaults first, then the user's file on top: each key overrides the one before.
    for path in system_config_files().into_iter().chain([config_path()]) {
        apply_file(&path, &mut cfg, &mut gap_set);
    }

    if let Some(g) = std::env::var("NIRI_SIDESTAGE_GAP").ok().and_then(|v| v.parse::<i32>().ok()) {
        cfg.gap = g.max(1);
    } else if !gap_set && let Some(g) = niri_gap() {
        cfg.gap = g.max(1);
    }
    cfg
}

/// Apply the `key = value` lines of one config file on top of `cfg`. A missing file is fine.
fn apply_file(path: &Path, cfg: &mut Config, gap_set: &mut bool) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            warn(path, n, "expected `key = value`");
            continue;
        };
        let (key, val) = (key.trim(), val.trim().trim_matches('"'));
        let bad = |what: &str| warn(path, n, &format!("{key}: {what}"));
        match key {
            "side" => match val {
                "left" => cfg.side = Side::Left,
                "right" => cfg.side = Side::Right,
                _ => bad("expected \"left\" or \"right\""),
            },
            "view" => match val {
                "windows" => cfg.view = View::Windows,
                "workspaces" => cfg.view = View::Workspaces,
                _ => bad("expected \"windows\" or \"workspaces\""),
            },
            "gap" => match val {
                "auto" => {
                    cfg.gap = Config::default().gap;
                    *gap_set = false;
                }
                v => match v.parse::<i32>() {
                    Ok(g) if g >= 1 => {
                        cfg.gap = g;
                        *gap_set = true;
                    }
                    _ => bad("expected a number >= 1 or \"auto\""),
                },
            },
            "panel_width" => match val.parse::<i32>() {
                Ok(w) if (100..=600).contains(&w) => cfg.panel_width = w,
                _ => bad("expected a number from 100 to 600"),
            },
            "hide_delay_ms" => match val.parse::<u64>() {
                Ok(ms) => cfg.hide_delay = Duration::from_millis(ms),
                _ => bad("expected a number of milliseconds"),
            },
            "slide_ms" => match val.parse::<f64>() {
                Ok(ms) if ms >= 0.0 => cfg.slide_ms = ms,
                _ => bad("expected a number of milliseconds"),
            },
            "min_trigger" => match val.parse::<i32>() {
                Ok(h) if h >= 0 => cfg.min_trigger = h,
                _ => bad("expected a height in pixels"),
            },
            _ => warn(path, n, &format!("unknown key `{key}`")),
        }
    }
}

fn warn(path: &Path, line: usize, msg: &str) {
    eprintln!("niri-sidestage: {}:{}: {msg}", path.display(), line + 1);
}

/// The `gaps` value from niri's config, following `include` lines. This is the width of the
/// outer gap the edge trigger lives in.
pub fn niri_gap() -> Option<i32> {
    let main = std::env::var_os("NIRI_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| xdg_config_home().join("niri").join("config.kdl"));
    find_gap(&main, 0)
}

fn find_gap(file: &Path, depth: u8) -> Option<i32> {
    if depth > 5 {
        return None;
    }
    let text = std::fs::read_to_string(file).ok()?;
    let dir = file.parent().unwrap_or(Path::new("."));
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or("").trim();
        // `/-` comments out the node that follows it.
        if line.is_empty() || line.starts_with("/-") {
            continue;
        }
        let mut tok = line.split_whitespace();
        match tok.next() {
            Some("gaps") => {
                if let Some(v) = tok.next().and_then(|v| v.parse::<f64>().ok()) {
                    return Some(v.round() as i32);
                }
            }
            Some("include") => {
                if let Some(rel) = line.split('"').nth(1) {
                    let p = Path::new(rel);
                    let p = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
                    if let Some(g) = find_gap(&p, depth + 1) {
                        return Some(g);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("niri-sidestage-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn gap_in_main_file() {
        let d = tmp("main");
        fs::write(d.join("config.kdl"), "layout {\n    gaps 12\n}\n").unwrap();
        assert_eq!(find_gap(&d.join("config.kdl"), 0), Some(12));
    }

    #[test]
    fn gap_via_include_and_comments() {
        let d = tmp("inc");
        fs::create_dir_all(d.join("config.d")).unwrap();
        fs::write(
            d.join("config.kdl"),
            "// gaps 99\ninclude \"config.d/layout.kdl\"\n",
        )
        .unwrap();
        fs::write(
            d.join("config.d/layout.kdl"),
            "layout {\n    /-gaps 40\n    gaps 7 // trailing\n    gaps-between-tabs 2\n}\n",
        )
        .unwrap();
        assert_eq!(find_gap(&d.join("config.kdl"), 0), Some(7));
    }

    #[test]
    fn no_gap_and_missing_file() {
        let d = tmp("none");
        fs::write(d.join("config.kdl"), "layout { }\n").unwrap();
        assert_eq!(find_gap(&d.join("config.kdl"), 0), None);
        assert_eq!(find_gap(&d.join("missing.kdl"), 0), None);
    }

    #[test]
    fn include_cycle_terminates() {
        let d = tmp("cycle");
        fs::write(d.join("a.kdl"), "include \"a.kdl\"\n").unwrap();
        assert_eq!(find_gap(&d.join("a.kdl"), 0), None);
    }

    #[test]
    fn user_file_overrides_system_file_key_by_key() {
        let d = tmp("layer");
        let (sys, user) = (d.join("sys.toml"), d.join("user.toml"));
        fs::write(&sys, "side = \"right\"\npanel_width = 200\ngap = 9\n").unwrap();
        fs::write(&user, "# only some keys\npanel_width = 180\ngap = auto\n").unwrap();
        let (mut cfg, mut gap_set) = (Config::default(), false);
        apply_file(&sys, &mut cfg, &mut gap_set);
        assert_eq!((cfg.side, cfg.panel_width, cfg.gap, gap_set), (Side::Right, 200, 9, true));
        apply_file(&user, &mut cfg, &mut gap_set);
        // side kept from the system file, width overridden, "auto" cleared the explicit gap
        assert_eq!((cfg.side, cfg.panel_width, cfg.gap, gap_set), (Side::Right, 180, 5, false));
    }

    #[test]
    fn view_option() {
        let d = tmp("view");
        let f = d.join("c.toml");
        fs::write(&f, "view = \"workspaces\"\n").unwrap();
        let (mut cfg, mut gap_set) = (Config::default(), false);
        apply_file(&f, &mut cfg, &mut gap_set);
        assert_eq!(cfg.view, View::Workspaces);
        fs::write(&f, "view = \"grid\"\n").unwrap();
        apply_file(&f, &mut cfg, &mut gap_set);
        assert_eq!(cfg.view, View::Workspaces);
    }

    #[test]
    fn bad_values_are_ignored() {
        let d = tmp("bad");
        let f = d.join("c.toml");
        fs::write(&f, "panel_width = 5\nside = \"up\"\nnonsense\nslide_ms = fast\n").unwrap();
        let (mut cfg, mut gap_set) = (Config::default(), false);
        apply_file(&f, &mut cfg, &mut gap_set);
        assert_eq!((cfg.panel_width, cfg.side, cfg.slide_ms), (152, Side::Left, 170.0));
    }

    #[test]
    fn shipped_default_config_matches_builtin_defaults() {
        let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/config.toml");
        let (mut cfg, mut gap_set) = (Config::default(), false);
        apply_file(&f, &mut cfg, &mut gap_set);
        let d = Config::default();
        assert_eq!(
            (cfg.view, cfg.side, cfg.gap, cfg.panel_width, cfg.hide_delay, cfg.slide_ms, cfg.min_trigger, gap_set),
            (d.view, d.side, d.gap, d.panel_width, d.hide_delay, d.slide_ms, d.min_trigger, false)
        );
    }
}
