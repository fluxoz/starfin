mod media;

use media::hwaccel::HwAccel;
use media::transcode::Quality;
use tracing::{error, info, warn};

use actix_web::{
    App, Error, HttpRequest, HttpResponse, HttpServer, Responder,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    http::header, middleware::{self, Logger, Next}, web,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use rust_embed::RustEmbed;
use mime_guess::MimeGuess;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use parking_lot::{Mutex, RwLock};
use std::time::{Duration, Instant, UNIX_EPOCH};
use uuid::Uuid;
use walkdir::WalkDir;
use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::{SaltString, rand_core::OsRng}};

// ── Theme system ─────────────────────────────────────────────────────────────

/// TOML-based theme configuration.  A theme defines CSS custom properties for
/// both light and dark modes, giving end-users full control over the UI palette.
///
/// ### Preset themes
/// Set the `THEME` environment variable to one of: `jetson` (default), `nord`,
/// `catppuccin`, or `dracula`.
///
/// ### Preset designs
/// Set the `DESIGN` environment variable to one of: `editorial` (default),
/// `neubrutalist`, or `aero`.  Designs control the UX style language
/// (typography, geometry, effects) and are composable with any color theme.
///
/// ### Custom themes
/// Point `THEME_FILE` at a TOML file to use a fully custom palette.  See the
/// bundled `themes/example.toml` for the file format.

#[derive(Clone, Debug, Deserialize)]
struct ThemeMeta {
    name: String,
    /// Optional design preset name (e.g. "neubrutalist", "aero").
    /// Overridden by the `DESIGN` environment variable.
    #[serde(default)]
    design: Option<String>,
}

/// One mode (light or dark) of a theme — each field maps directly to a CSS
/// custom property.  All fields are optional so a user-supplied TOML only needs
/// to override the values they care about; anything omitted inherits the
/// built-in Jetson defaults.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ThemeMode {
    bg: Option<String>,
    panel: Option<String>,
    panel_2: Option<String>,
    text: Option<String>,
    muted: Option<String>,
    border: Option<String>,
    accent: Option<String>,
    accent_2: Option<String>,
    danger: Option<String>,
    radius: Option<String>,
    shadow: Option<String>,
    sidebar_bg: Option<String>,
    topbar_bg: Option<String>,
    topbar_border: Option<String>,
    card_bg: Option<String>,
    card_border: Option<String>,
    card_top_bg: Option<String>,
    card_top_color: Option<String>,
    input_bg: Option<String>,
    input_border: Option<String>,
    notice_bg: Option<String>,
    empty_bg: Option<String>,
}

/// Design tokens control the structural UX appearance: typography, geometry,
/// and visual effects.  Each field maps to a CSS custom property.  All fields
/// are optional — omitted values keep the built-in editorial defaults from
/// `main.css`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct DesignTokens {
    font_body: Option<String>,
    font_heading: Option<String>,
    border_width: Option<String>,
    heading_transform: Option<String>,
    heading_spacing: Option<String>,
    heading_weight: Option<String>,
}

/// A design defines the UX style language — typography, geometry, and visual
/// effects.  Designs are composable with any color theme.
struct DesignConfig {
    name: String,
    /// CSS custom property declarations for the design tokens.
    tokens: DesignTokens,
    /// Additional CSS rules specific to this design (structural overrides
    /// that cannot be expressed as simple custom properties).
    extra_css: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ThemeConfig {
    meta: ThemeMeta,
    /// Custom design token overrides from the TOML `[design]` section.
    #[serde(default)]
    design: DesignTokens,
    #[serde(default)]
    light: ThemeMode,
    #[serde(default)]
    dark: ThemeMode,
}

impl ThemeMode {
    /// Emit CSS custom property declarations for every field that is `Some`.
    /// Values are sanitized to prevent CSS injection.
    fn to_css_declarations(&self) -> String {
        let mut out = String::new();
        let fields: &[(&str, &Option<String>)] = &[
            ("--bg",            &self.bg),
            ("--panel",         &self.panel),
            ("--panel-2",       &self.panel_2),
            ("--text",          &self.text),
            ("--muted",         &self.muted),
            ("--border",        &self.border),
            ("--accent",        &self.accent),
            ("--accent-2",      &self.accent_2),
            ("--danger",        &self.danger),
            ("--radius",        &self.radius),
            ("--shadow",        &self.shadow),
            ("--sidebar-bg",    &self.sidebar_bg),
            ("--topbar-bg",     &self.topbar_bg),
            ("--topbar-border", &self.topbar_border),
            ("--card-bg",       &self.card_bg),
            ("--card-border",   &self.card_border),
            ("--card-top-bg",   &self.card_top_bg),
            ("--card-top-color",&self.card_top_color),
            ("--input-bg",      &self.input_bg),
            ("--input-border",  &self.input_border),
            ("--notice-bg",     &self.notice_bg),
            ("--empty-bg",      &self.empty_bg),
        ];
        for (prop, value) in fields {
            if let Some(v) = value {
                let sanitized = sanitize_css_value(v);
                out.push_str(&format!("  {}: {};\n", prop, sanitized));
            }
        }
        out
    }
}

/// Sanitize a CSS custom property value to prevent injection.
///
/// Strips characters that could break out of a CSS declaration (`{`, `}`, `;`,
/// `<`, `>`) and removes `url(` / `expression(` function calls.  This is a
/// defense-in-depth measure — theme files are operator-controlled, not
/// user-supplied, but we sanitize anyway.
fn sanitize_css_value(value: &str) -> String {
    let stripped: String = value
        .chars()
        .filter(|c| !matches!(c, '{' | '}' | ';' | '<' | '>' | '\\'))
        .collect();
    // Reject url() and expression() to prevent resource loading / script execution.
    let lower = stripped.to_lowercase();
    if lower.contains("url(") || lower.contains("expression(") || lower.contains("javascript:") {
        return String::new();
    }
    stripped
}

impl ThemeConfig {
    /// Generate a complete CSS stylesheet that overrides the default (Jetson)
    /// custom properties.  Returns an empty string for the built-in Jetson
    /// theme so the default `main.css` values apply unchanged.
    fn to_css(&self) -> String {
        let light = self.light.to_css_declarations();
        let dark = self.dark.to_css_declarations();
        if light.is_empty() && dark.is_empty() {
            return String::new();
        }
        let mut css = format!("/* Theme: {} */\n", self.meta.name);
        if !light.is_empty() {
            css.push_str(":root{\n");
            css.push_str(&light);
            css.push_str("}\n");
        }
        if !dark.is_empty() {
            css.push_str(".app.dark-mode{\n");
            css.push_str(&dark);
            css.push_str("}\n");
        }
        css
    }
}

impl DesignTokens {
    /// Emit CSS custom property declarations for every design token that is `Some`.
    fn to_css_declarations(&self) -> String {
        let mut out = String::new();
        let fields: &[(&str, &Option<String>)] = &[
            ("--font-body",         &self.font_body),
            ("--font-heading",      &self.font_heading),
            ("--border-width",      &self.border_width),
            ("--heading-transform", &self.heading_transform),
            ("--heading-spacing",   &self.heading_spacing),
            ("--heading-weight",    &self.heading_weight),
        ];
        for (prop, value) in fields {
            if let Some(v) = value {
                let sanitized = sanitize_css_value(v);
                out.push_str(&format!("  {}: {};\n", prop, sanitized));
            }
        }
        out
    }

    /// Merge another set of tokens on top, overriding only the fields that are
    /// `Some` in `overrides`.
    fn merge(&mut self, overrides: &DesignTokens) {
        if overrides.font_body.is_some()         { self.font_body = overrides.font_body.clone(); }
        if overrides.font_heading.is_some()      { self.font_heading = overrides.font_heading.clone(); }
        if overrides.border_width.is_some()      { self.border_width = overrides.border_width.clone(); }
        if overrides.heading_transform.is_some() { self.heading_transform = overrides.heading_transform.clone(); }
        if overrides.heading_spacing.is_some()   { self.heading_spacing = overrides.heading_spacing.clone(); }
        if overrides.heading_weight.is_some()    { self.heading_weight = overrides.heading_weight.clone(); }
    }
}

impl DesignConfig {
    /// Generate a CSS stylesheet for this design.  Includes custom property
    /// declarations in `:root` and any additional structural CSS rules.
    fn to_css(&self) -> String {
        let decls = self.tokens.to_css_declarations();
        if decls.is_empty() && self.extra_css.is_empty() {
            return String::new();
        }
        let mut css = format!("/* Design: {} */\n", self.name);
        if !decls.is_empty() {
            css.push_str(":root{\n");
            css.push_str(&decls);
            css.push_str("}\n");
        }
        if !self.extra_css.is_empty() {
            css.push_str(&self.extra_css);
            css.push('\n');
        }
        css
    }
}

// ── Design presets ────────────────────────────────────────────────────────────

/// Built-in "Editorial" design — the default.  Monospace typography, uppercase
/// headings, thick borders, and a technical/industrial feel.  Returns empty CSS
/// so `main.css` defaults apply unchanged.
fn design_editorial() -> DesignConfig {
    DesignConfig {
        name: "Editorial".into(),
        tokens: DesignTokens::default(),
        extra_css: String::new(),
    }
}

/// Built-in "Neubrutalist" design — bold system-ui sans-serif typography, zero
/// border-radius, extra-thick borders, and hard offset drop shadows.
fn design_neubrutalist() -> DesignConfig {
    DesignConfig {
        name: "Neubrutalist".into(),
        tokens: DesignTokens {
            font_body:         Some("system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif".into()),
            font_heading:      Some("system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif".into()),
            border_width:      Some("3px".into()),
            heading_transform: Some("uppercase".into()),
            heading_spacing:   Some("1px".into()),
            heading_weight:    Some("900".into()),
        },
        extra_css: concat!(
            "/* Neubrutalist structural overrides */\n",
            ":root{ --radius: 0px; --shadow: 5px 5px 0 rgba(0,0,0,.25); }\n",
            ".card{ box-shadow: 5px 5px 0 var(--card-border); }\n",
            ".btn,.chip[aria-pressed=\"true\"]{ box-shadow: 3px 3px 0 rgba(0,0,0,.3); }\n",
            ".btn:hover{ transform: translate(1px,1px); box-shadow: 2px 2px 0 rgba(0,0,0,.3); }\n",
            ".input,.select{ box-shadow: 3px 3px 0 var(--input-border); }\n",
            ".badge{ box-shadow: 2px 2px 0 rgba(0,0,0,.2); }\n",
            ".scroll-top-btn{ box-shadow: 4px 4px 0 rgba(0,0,0,.3); }\n",
            ".section-header{ border-bottom-width: 4px; }\n",
            ".app.dark-mode{ --shadow: 5px 5px 0 rgba(0,0,0,.5); }\n",
            ".app.dark-mode .card{ box-shadow: 5px 5px 0 var(--card-border); }\n",
            ".app.dark-mode .btn,.app.dark-mode .chip[aria-pressed=\"true\"]{ box-shadow: 3px 3px 0 rgba(255,255,255,.1); }\n",
            ".app.dark-mode .btn:hover{ box-shadow: 2px 2px 0 rgba(255,255,255,.1); }\n",
            ".app.dark-mode .input,.app.dark-mode .select{ box-shadow: 3px 3px 0 var(--input-border); }\n",
        ).into(),
    }
}

/// Built-in "Aero" design — glass morphism inspired by Y2K / early-2000s
/// aesthetics (think Nokia, Aqua, Windows Vista).  Rounded corners, translucent
/// panels with backdrop-filter blur, and soft shadows.
fn design_aero() -> DesignConfig {
    DesignConfig {
        name: "Aero".into(),
        tokens: DesignTokens {
            font_body:         Some("system-ui, -apple-system, 'Segoe UI', Tahoma, sans-serif".into()),
            font_heading:      Some("system-ui, -apple-system, 'Segoe UI', Tahoma, sans-serif".into()),
            border_width:      Some("1px".into()),
            heading_transform: Some("none".into()),
            heading_spacing:   Some("0.5px".into()),
            heading_weight:    Some("600".into()),
        },
        extra_css: concat!(
            "/* Aero / Glass Y2K structural overrides */\n",
            ":root{ --radius: 16px; --shadow: 0 8px 32px rgba(0,0,0,.10); }\n",
            ".card{ backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px); }\n",
            ".topbar{ backdrop-filter: blur(12px); -webkit-backdrop-filter: blur(12px); }\n",
            ".filters{ backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px); border-width: 1px; }\n",
            ".notice{ backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px); }\n",
            ".section-header{ border-bottom-width: 1px; }\n",
            ".scan-btn{ border-width: 1px; }\n",
            ".random-btn{ border-width: 1px; }\n",
            ".scroll-top-btn{ border-width: 1px; border-radius: 50%; }\n",
            ".app.dark-mode{ --shadow: 0 8px 32px rgba(0,0,0,.30); }\n",
        ).into(),
    }
}

/// Resolve the active design from environment variables and theme config.
///
/// * `DESIGN` — preset name: `editorial` (default), `neubrutalist`, `aero`.
///   Takes precedence over the TOML `meta.design` field.
/// * Custom tokens from the TOML `[design]` section are merged on top of the
///   selected preset.
fn resolve_design(theme: &ThemeConfig) -> DesignConfig {
    let name = std::env::var("DESIGN")
        .ok()
        .or_else(|| theme.meta.design.clone())
        .unwrap_or_default()
        .to_lowercase();

    let mut design = match name.as_str() {
        "neubrutalist" | "brutalist" => { info!(design = "Neubrutalist", "using design preset"); design_neubrutalist() }
        "aero" | "glass" | "y2k"    => { info!(design = "Aero", "using design preset"); design_aero() }
        "editorial"                  => { info!(design = "Editorial", "using design preset"); design_editorial() }
        _                            => { info!(design = "Editorial", "using default design"); design_editorial() }
    };

    // Merge any custom token overrides from the TOML [design] section.
    design.tokens.merge(&theme.design);

    design
}

/// Built-in "Jetson" theme — the default.  Returns an empty CSS string because
/// `main.css` already defines these values.
fn theme_jetson() -> ThemeConfig {
    ThemeConfig {
        meta: ThemeMeta { name: "Jetson".into(), design: None },
        design: DesignTokens::default(),
        light: ThemeMode::default(),
        dark: ThemeMode::default(),
    }
}

/// Built-in "Nord" theme — Arctic, cool-blue toned palette inspired by the
/// popular Nord color scheme.
fn theme_nord() -> ThemeConfig {
    ThemeConfig {
        meta: ThemeMeta { name: "Nord".into(), design: None },
        design: DesignTokens::default(),
        light: ThemeMode {
            bg:            Some("#eceff4".into()),
            panel:         Some("rgba(46,52,64,.04)".into()),
            panel_2:       Some("rgba(46,52,64,.07)".into()),
            text:          Some("#2e3440".into()),
            muted:         Some("#4c566a".into()),
            border:        Some("rgba(46,52,64,.12)".into()),
            accent:        Some("#5e81ac".into()),
            accent_2:      Some("#3b4252".into()),
            danger:        Some("#bf616a".into()),
            radius:        Some("6px".into()),
            shadow:        Some("0 2px 8px rgba(46,52,64,.10)".into()),
            sidebar_bg:    Some("#5e81ac".into()),
            topbar_bg:     Some("#d8dee9".into()),
            topbar_border: Some("2px solid rgba(46,52,64,.10)".into()),
            card_bg:       Some("rgba(255,255,255,.70)".into()),
            card_border:   Some("rgba(46,52,64,.15)".into()),
            card_top_bg:   Some("#3b4252".into()),
            card_top_color:Some("#eceff4".into()),
            input_bg:      Some("rgba(255,255,255,.60)".into()),
            input_border:  Some("rgba(46,52,64,.18)".into()),
            notice_bg:     Some("rgba(255,255,255,.65)".into()),
            empty_bg:      Some("rgba(255,255,255,.45)".into()),
        },
        dark: ThemeMode {
            bg:            Some("#2e3440".into()),
            panel:         Some("rgba(255,255,255,.04)".into()),
            panel_2:       Some("rgba(255,255,255,.07)".into()),
            text:          Some("#d8dee9".into()),
            muted:         Some("#81a1c1".into()),
            border:        Some("rgba(255,255,255,.10)".into()),
            accent:        Some("#88c0d0".into()),
            accent_2:      Some("#434c5e".into()),
            danger:        Some("#bf616a".into()),
            shadow:        Some("0 2px 8px rgba(0,0,0,.40)".into()),
            sidebar_bg:    Some("#5e81ac".into()),
            topbar_bg:     Some("#3b4252".into()),
            topbar_border: Some("1px solid rgba(255,255,255,.08)".into()),
            card_bg:       Some("rgba(255,255,255,.06)".into()),
            card_border:   Some("rgba(255,255,255,.10)".into()),
            card_top_bg:   Some("#434c5e".into()),
            card_top_color:Some("#d8dee9".into()),
            input_bg:      Some("rgba(255,255,255,.06)".into()),
            input_border:  Some("rgba(255,255,255,.12)".into()),
            notice_bg:     Some("rgba(255,255,255,.06)".into()),
            empty_bg:      Some("rgba(255,255,255,.04)".into()),
            radius:        None,
        },
    }
}

/// Built-in "Catppuccin" theme — soothing pastel palette from the popular
/// Catppuccin project (Latte for light, Mocha for dark).
fn theme_catppuccin() -> ThemeConfig {
    ThemeConfig {
        meta: ThemeMeta { name: "Catppuccin".into(), design: None },
        design: DesignTokens::default(),
        light: ThemeMode {
            bg:            Some("#eff1f5".into()),
            panel:         Some("rgba(76,79,105,.04)".into()),
            panel_2:       Some("rgba(76,79,105,.07)".into()),
            text:          Some("#4c4f69".into()),
            muted:         Some("#6c6f85".into()),
            border:        Some("rgba(76,79,105,.12)".into()),
            accent:        Some("#8839ef".into()),
            accent_2:      Some("#5c5f77".into()),
            danger:        Some("#d20f39".into()),
            radius:        Some("8px".into()),
            shadow:        Some("0 2px 8px rgba(76,79,105,.10)".into()),
            sidebar_bg:    Some("#8839ef".into()),
            topbar_bg:     Some("#e6e9ef".into()),
            topbar_border: Some("2px solid rgba(76,79,105,.08)".into()),
            card_bg:       Some("rgba(255,255,255,.65)".into()),
            card_border:   Some("rgba(76,79,105,.12)".into()),
            card_top_bg:   Some("#5c5f77".into()),
            card_top_color:Some("#eff1f5".into()),
            input_bg:      Some("rgba(255,255,255,.55)".into()),
            input_border:  Some("rgba(76,79,105,.15)".into()),
            notice_bg:     Some("rgba(255,255,255,.60)".into()),
            empty_bg:      Some("rgba(255,255,255,.40)".into()),
        },
        dark: ThemeMode {
            bg:            Some("#1e1e2e".into()),
            panel:         Some("rgba(205,214,244,.04)".into()),
            panel_2:       Some("rgba(205,214,244,.07)".into()),
            text:          Some("#cdd6f4".into()),
            muted:         Some("#a6adc8".into()),
            border:        Some("rgba(205,214,244,.10)".into()),
            accent:        Some("#cba6f7".into()),
            accent_2:      Some("#45475a".into()),
            danger:        Some("#f38ba8".into()),
            shadow:        Some("0 2px 8px rgba(0,0,0,.40)".into()),
            sidebar_bg:    Some("#8839ef".into()),
            topbar_bg:     Some("#313244".into()),
            topbar_border: Some("1px solid rgba(205,214,244,.08)".into()),
            card_bg:       Some("rgba(205,214,244,.06)".into()),
            card_border:   Some("rgba(205,214,244,.10)".into()),
            card_top_bg:   Some("#45475a".into()),
            card_top_color:Some("#cdd6f4".into()),
            input_bg:      Some("rgba(205,214,244,.06)".into()),
            input_border:  Some("rgba(205,214,244,.12)".into()),
            notice_bg:     Some("rgba(205,214,244,.06)".into()),
            empty_bg:      Some("rgba(205,214,244,.04)".into()),
            radius:        None,
        },
    }
}

/// Built-in "Dracula" theme — the popular purple/pink/green dark-first palette.
fn theme_dracula() -> ThemeConfig {
    ThemeConfig {
        meta: ThemeMeta { name: "Dracula".into(), design: None },
        design: DesignTokens::default(),
        light: ThemeMode {
            bg:            Some("#f8f8f2".into()),
            panel:         Some("rgba(40,42,54,.04)".into()),
            panel_2:       Some("rgba(40,42,54,.07)".into()),
            text:          Some("#282a36".into()),
            muted:         Some("#6272a4".into()),
            border:        Some("rgba(40,42,54,.12)".into()),
            accent:        Some("#bd93f9".into()),
            accent_2:      Some("#44475a".into()),
            danger:        Some("#ff5555".into()),
            radius:        Some("6px".into()),
            shadow:        Some("0 2px 8px rgba(40,42,54,.12)".into()),
            sidebar_bg:    Some("#bd93f9".into()),
            topbar_bg:     Some("#e8e8e2".into()),
            topbar_border: Some("2px solid rgba(40,42,54,.10)".into()),
            card_bg:       Some("rgba(255,255,255,.65)".into()),
            card_border:   Some("rgba(40,42,54,.15)".into()),
            card_top_bg:   Some("#44475a".into()),
            card_top_color:Some("#f8f8f2".into()),
            input_bg:      Some("rgba(255,255,255,.55)".into()),
            input_border:  Some("rgba(40,42,54,.18)".into()),
            notice_bg:     Some("rgba(255,255,255,.60)".into()),
            empty_bg:      Some("rgba(255,255,255,.40)".into()),
        },
        dark: ThemeMode {
            bg:            Some("#282a36".into()),
            panel:         Some("rgba(248,248,242,.04)".into()),
            panel_2:       Some("rgba(248,248,242,.07)".into()),
            text:          Some("#f8f8f2".into()),
            muted:         Some("#6272a4".into()),
            border:        Some("rgba(248,248,242,.10)".into()),
            accent:        Some("#bd93f9".into()),
            accent_2:      Some("#44475a".into()),
            danger:        Some("#ff5555".into()),
            shadow:        Some("0 2px 8px rgba(0,0,0,.50)".into()),
            sidebar_bg:    Some("#bd93f9".into()),
            topbar_bg:     Some("#44475a".into()),
            topbar_border: Some("1px solid rgba(248,248,242,.08)".into()),
            card_bg:       Some("rgba(248,248,242,.06)".into()),
            card_border:   Some("rgba(248,248,242,.10)".into()),
            card_top_bg:   Some("#44475a".into()),
            card_top_color:Some("#f8f8f2".into()),
            input_bg:      Some("rgba(248,248,242,.06)".into()),
            input_border:  Some("rgba(248,248,242,.12)".into()),
            notice_bg:     Some("rgba(248,248,242,.06)".into()),
            empty_bg:      Some("rgba(248,248,242,.04)".into()),
            radius:        None,
        },
    }
}

/// Resolve the active theme from environment variables.
///
/// * `THEME` — preset name: `jetson`, `nord`, `catppuccin`, `dracula`.
/// * `THEME_FILE` — path to a user-supplied TOML file (takes precedence over
///   `THEME` if both are set).  May also include a `[design]` section and
///   `meta.design` field to select or customise the UX design.
fn resolve_theme() -> ThemeConfig {
    /// Maximum theme file size (100 KiB) — prevents DoS via oversized files.
    const MAX_THEME_FILE_SIZE: u64 = 100 * 1024;

    // A custom TOML file takes highest precedence.
    if let Ok(path) = std::env::var("THEME_FILE") {
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_THEME_FILE_SIZE => {
                warn!(
                    path = %path,
                    size = meta.len(),
                    limit = MAX_THEME_FILE_SIZE,
                    "THEME_FILE exceeds size limit; falling back to preset"
                );
            }
            _ => {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match toml::from_str::<ThemeConfig>(&contents) {
                        Ok(cfg) => {
                            info!(path = %path, name = %cfg.meta.name, "loaded custom theme");
                            return cfg;
                        }
                        Err(e) => {
                            warn!(path = %path, error = %e, "failed to parse THEME_FILE; falling back to preset");
                        }
                    },
                    Err(e) => {
                        warn!(path = %path, error = %e, "failed to read THEME_FILE; falling back to preset");
                    }
                }
            }
        }
    }

    let name = std::env::var("THEME").unwrap_or_default().to_lowercase();
    match name.as_str() {
        "nord"       => { info!(theme = "Nord", "using preset theme"); theme_nord() }
        "catppuccin" => { info!(theme = "Catppuccin", "using preset theme"); theme_catppuccin() }
        "dracula"    => { info!(theme = "Dracula", "using preset theme"); theme_dracula() }
        _            => { info!(theme = "Jetson", "using default theme"); theme_jetson() }
    }
}

/// `GET /api/theme.css` — returns the active theme as a CSS stylesheet.
async fn get_theme_css(state: web::Data<AppState>) -> HttpResponse {
    HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, "text/css; charset=utf-8"))
        .insert_header((header::CACHE_CONTROL, "no-cache"))
        .body(state.theme_css.clone())
}

// ── Stream quality ────────────────────────────────────────────────────────────

/// Query-string struct used by the playlist and segment endpoints so that
/// `?quality=medium` is deserialized into a `Quality` value automatically.
#[derive(Default, Deserialize)]
struct QualityQuery {
    #[serde(default)]
    quality: Quality,
}

/// `GET /api/quality-options` – list the available quality levels.
async fn get_quality_options() -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!([
        { "value": "original", "label": Quality::Original.label() },
        { "value": "2160p",    "label": Quality::Q2160.label() },
        { "value": "1080p",    "label": Quality::Q1080.label() },
        { "value": "720p",     "label": Quality::Q720.label() },
        { "value": "480p",     "label": Quality::Q480.label() },
        { "value": "360p",     "label": Quality::Q360.label() },
    ]))
}

/// `GET /api/videos/{id}/quality-info` – return quality options with estimated
/// bitrate and resolution specific to this video.
async fn get_video_quality_info(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs, _title) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let (stream_info, remuxable) = tokio::task::spawn_blocking(move || {
        let si = media::probe::probe_stream_info(&abs);
        let rm = media::probe::is_source_remuxable(&abs);
        (si, rm)
    })
    .await
    .unwrap_or_else(|_| (Default::default(), false));

    // Resolution-based transcoded qualities, ordered highest to lowest.
    // Only include qualities whose target height is ≤ the source height so
    // we never advertise upscaling options.
    let res_qualities: &[(Quality, u32)] = &[
        (Quality::Q2160, 2160),
        (Quality::Q1080, 1080),
        (Quality::Q720,  720),
        (Quality::Q480,  480),
        (Quality::Q360,  360),
    ];
    let source_height = stream_info.height.max(1);

    // Original is always first.
    let mut qualities = vec![Quality::Original];
    for &(q, target_h) in res_qualities {
        if source_height >= target_h {
            qualities.push(q);
        }
    }

    let options: Vec<serde_json::Value> = qualities
        .iter()
        .map(|&q| {
            let bw = estimate_bandwidth(&stream_info, q);
            let (w, h) = estimate_resolution(&stream_info, q);
            let mbps = bw as f64 / 1_000_000.0;
            let label = if mbps >= 1.0 {
                format!("{}p · {:.1} Mbps", h, mbps)
            } else {
                format!("{}p · {} Kbps", h, (bw / 1000) as u32)
            };
            let mut entry = serde_json::json!({
                "value": q.as_str(),
                "label": label,
                "width": w,
                "height": h,
                "bitrate": bw,
            });
            if q == Quality::Original {
                entry["remuxable"] = serde_json::Value::Bool(remuxable);
            }
            entry
        })
        .collect();

    HttpResponse::Ok().json(options)
}

/// `GET /api/videos/{id}/stream` — serve the original video file directly for
/// "Original (Direct Copy)" playback.
///
/// Unlike DASH segment serving, this endpoint streams the entire remuxed file
/// with HTTP range request support so the browser can seek by byte position.
/// The output is a standard (non-fragmented) MP4 with `movflags=faststart`.
///
/// The remuxed file is cached at `{cache_dir}/{id}/direct.mp4` to avoid
/// repeated FFmpeg processing.  Video is always stream-copied; audio is
/// stream-copied when browser-compatible (stereo AAC/MP3) or transcoded to
/// stereo AAC otherwise.
async fn stream_video(
    id: web::Path<String>,
    req: HttpRequest,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    // Ensure the video-level cache directory exists.
    let video_cache_dir = state.cache_dir.join(id.as_str());
    if let Err(e) = tokio::fs::create_dir_all(&video_cache_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    let direct_path = video_cache_dir.join("direct.mp4");

    // Generate the remuxed file if it doesn't exist yet.
    if !direct_path.exists() {
        let abs_str = match abs_path.to_str() {
            Some(s) => s.to_owned(),
            None => return HttpResponse::BadRequest().body("video path is not valid UTF-8"),
        };
        let direct_path_clone = direct_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            media::transcode::create_direct_remux(&abs_str, &direct_path_clone)
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return HttpResponse::ServiceUnavailable()
                    .body(format!("remux failed: {e}"));
            }
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .body(format!("remux task panicked: {e}"));
            }
        }
    }

    // Get the file size.
    let file_size = match tokio::fs::metadata(&direct_path).await {
        Ok(m) => m.len(),
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("file metadata error: {e}"));
        }
    };

    if file_size == 0 {
        return HttpResponse::InternalServerError().body("remux produced empty file");
    }

    // Parse optional Range header.
    let range_header = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());

    let (start, end) = if let Some(ref range) = range_header {
        // Parse "bytes=start-[end]"
        let bytes_part = range.strip_prefix("bytes=").unwrap_or("");
        let mut parts = bytes_part.splitn(2, '-');
        let start_str = parts.next().unwrap_or("0");
        let end_str = parts.next().unwrap_or("");
        let start: u64 = start_str.parse().unwrap_or(0);
        let end: u64 = if end_str.is_empty() {
            file_size.saturating_sub(1)
        } else {
            end_str.parse().unwrap_or(file_size.saturating_sub(1))
        };
        if start > end || start >= file_size {
            return HttpResponse::RangeNotSatisfiable()
                .insert_header(("Content-Range", format!("bytes */{file_size}")))
                .finish();
        }
        (start, end.min(file_size.saturating_sub(1)))
    } else {
        (0u64, file_size.saturating_sub(1))
    };

    let content_length = end - start + 1;
    let is_partial = range_header.is_some() && (start != 0 || end != file_size.saturating_sub(1));

    // Open the file and seek to the requested start position once.
    let mut file = match tokio::fs::File::open(&direct_path).await {
        Ok(f) => f,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("open file: {e}"));
        }
    };
    if let Err(e) = file.seek(std::io::SeekFrom::Start(start)).await {
        return HttpResponse::InternalServerError()
            .body(format!("seek error: {e}"));
    }

    // Build a streaming body that reads [start, end] from the already-open file.
    let stream = futures::stream::unfold(
        (file, content_length),
        |(mut file, remaining)| async move {
            if remaining == 0 {
                return None;
            }
            let chunk_size: u64 = 65536;
            let to_read = remaining.min(chunk_size) as usize;
            let mut buf = vec![0u8; to_read];
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    let read = n as u64;
                    Some((
                        Ok::<bytes::Bytes, actix_web::Error>(bytes::Bytes::from(buf)),
                        (file, remaining - read),
                    ))
                }
                Err(e) => Some((Err(actix_web::Error::from(e)), (file, 0))),
            }
        },
    );

    let status = if is_partial {
        actix_web::http::StatusCode::PARTIAL_CONTENT
    } else {
        actix_web::http::StatusCode::OK
    };

    HttpResponse::build(status)
        .content_type("video/mp4")
        .insert_header(("Accept-Ranges", "bytes"))
        .insert_header(("Content-Length", content_length.to_string()))
        .insert_header((
            "Content-Range",
            format!("bytes {start}-{end}/{file_size}"),
        ))
        .streaming(stream)
}

/// Estimate the DASH `bandwidth` attribute (bits/sec) for a given quality.
///
/// For `Original` the probed container bitrate is used directly.  For
/// transcoded qualities we scale by the pixel-count ratio (area) and an
/// empirical CRF factor so that the MPD advertises a realistic value and
/// the frontend can display Mbps in the quality selector.
fn estimate_bandwidth(info: &media::probe::StreamInfo, quality: Quality) -> u64 {
    // Fallback: if probing returned 0 use a conservative 5 Mbps default.
    let source_bps = if info.bitrate > 0 { info.bitrate } else { 5_000_000 };

    match quality {
        Quality::Original => source_bps,
        Quality::Q2160 => {
            // Re-encode at native 4K: ~90% of original bitrate.
            ((source_bps as f64) * 0.9) as u64
        }
        Quality::Q1080 => {
            let max_w = 1920u32;
            let sw = info.width.max(1);
            let area_ratio = if sw > max_w {
                let r = max_w as f64 / sw as f64;
                r * r
            } else {
                1.0
            };
            ((source_bps as f64) * area_ratio * 0.55).max(2_000_000.0) as u64
        }
        Quality::Q720 => {
            let max_w = 1280u32;
            let sw = info.width.max(1);
            let area_ratio = if sw > max_w {
                let r = max_w as f64 / sw as f64;
                r * r
            } else {
                1.0
            };
            ((source_bps as f64) * area_ratio * 0.35).max(1_000_000.0) as u64
        }
        Quality::Q480 => {
            let max_w = 854u32;
            let sw = info.width.max(1);
            let area_ratio = if sw > max_w {
                let r = max_w as f64 / sw as f64;
                r * r
            } else {
                1.0
            };
            ((source_bps as f64) * area_ratio * 0.20).max(500_000.0) as u64
        }
        Quality::Q360 => {
            let max_w = 640u32;
            let sw = info.width.max(1);
            let area_ratio = if sw > max_w {
                let r = max_w as f64 / sw as f64;
                r * r
            } else {
                1.0
            };
            ((source_bps as f64) * area_ratio * 0.12).max(300_000.0) as u64
        }
    }
}

/// Estimate the output resolution for a given quality.
///
/// Mirrors the logic in `transcode.rs` — scale down to fit within max width
/// while keeping aspect ratio and rounding height to even.
fn estimate_resolution(info: &media::probe::StreamInfo, quality: Quality) -> (u32, u32) {
    let (sw, sh) = (info.width.max(1), info.height.max(1));
    match quality {
        Quality::Original | Quality::Q2160 => {
            let max_w = 3840u32;
            if sw <= max_w { (sw, sh) } else {
                let r = max_w as f64 / sw as f64;
                let h = ((sh as f64 * r) as u32) & !1;
                (max_w, h)
            }
        }
        Quality::Q1080 => {
            let max_w = 1920u32;
            if sw <= max_w { (sw, sh) } else {
                let r = max_w as f64 / sw as f64;
                let h = ((sh as f64 * r) as u32) & !1;
                (max_w, h)
            }
        }
        Quality::Q720 => {
            let max_w = 1280u32;
            if sw <= max_w { (sw, sh) } else {
                let r = max_w as f64 / sw as f64;
                let h = ((sh as f64 * r) as u32) & !1;
                (max_w, h)
            }
        }
        Quality::Q480 => {
            let max_w = 854u32;
            if sw <= max_w { (sw, sh) } else {
                let r = max_w as f64 / sw as f64;
                let h = ((sh as f64 * r) as u32) & !1;
                (max_w, h)
            }
        }
        Quality::Q360 => {
            let max_w = 640u32;
            if sw <= max_w { (sw, sh) } else {
                let r = max_w as f64 / sw as f64;
                let h = ((sh as f64 * r) as u32) & !1;
                (max_w, h)
            }
        }
    }
}

// ── Startup healthchecks ──────────────────────────────────────────────────────

/// Run detailed healthchecks at startup and log results so they are visible in
/// journalctl.  Checks cover: process identity, directory read/write access,
/// ffmpeg availability, and available render devices.
async fn run_startup_healthchecks(library_path: &Path, cache_dir: &Path) {
    info!("STARFIN — STARTUP HEALTHCHECKS");

    // ── 1. Process identity ──────────────────────────────────────────────
    info!("── Process identity");
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    // Resolve username from /etc/passwd via reentrant getpwuid_r.
    let username = {
        let mut buf = vec![0u8; 1024];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                uid,
                pwd.as_mut_ptr(),
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 && !result.is_null() {
            let pwd = unsafe { pwd.assume_init() };
            unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }
                .to_string_lossy()
                .into_owned()
        } else {
            format!("(uid {})", uid)
        }
    };

    // Resolve group name from /etc/group via reentrant getgrgid_r.
    let groupname = {
        let mut buf = vec![0u8; 1024];
        let mut grp = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut result: *mut libc::group = std::ptr::null_mut();
        let rc = unsafe {
            libc::getgrgid_r(
                gid,
                grp.as_mut_ptr(),
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 && !result.is_null() {
            let grp = unsafe { grp.assume_init() };
            unsafe { std::ffi::CStr::from_ptr(grp.gr_name) }
                .to_string_lossy()
                .into_owned()
        } else {
            format!("(gid {})", gid)
        }
    };

    info!(user = %username, uid, group = %groupname, gid, pid = std::process::id(), "process identity");

    // ── 2. Directory access checks ───────────────────────────────────────
    info!("── Directory access");
    check_directory_access("VIDEO_LIBRARY_PATH", library_path);
    check_directory_access("CACHE_DIR", cache_dir);

    // ── 3. ffmpeg libraries (linked in-process via ffmpeg-next) ─────────
    info!("── ffmpeg (in-process via ffmpeg-next)");
    media::ensure_init();
    info!(libavcodec = %media::libavcodec_version_string(), libavformat = %media::libavformat_version_string(), libavfilter = %media::libavfilter_version_string(), "ffmpeg libraries loaded");

    // ── 4. Render devices ────────────────────────────────────────────────
    info!("── Render devices (/dev/dri)");
    let dri_path = Path::new("/dev/dri");
    if dri_path.exists() {
        match std::fs::read_dir(dri_path) {
            Ok(entries) => {
                let mut found_any = false;
                let mut devices: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .collect();
                devices.sort_by_key(|e| e.file_name());
                for entry in &devices {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("render") || name_str.starts_with("card") {
                        let accessible = std::fs::File::open(entry.path()).is_ok();
                        if accessible {
                            info!(device = %name_str, "render device accessible");
                        } else {
                            warn!(device = %name_str, "render device not accessible");
                        }
                        found_any = true;
                    }
                }
                if !found_any {
                    info!("no render/card devices found in /dev/dri");
                }
            }
            Err(e) => warn!(error = %e, "cannot read /dev/dri"),
        }

        // Also check by-path symlinks for stable device identification
        let by_path = dri_path.join("by-path");
        if by_path.exists() {
            info!("── Stable paths (/dev/dri/by-path)");
            if let Ok(entries) = std::fs::read_dir(&by_path) {
                let mut links: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .collect();
                links.sort_by_key(|e| e.file_name());
                for entry in &links {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.contains("render") {
                        let target = std::fs::read_link(entry.path())
                            .map(|t| t.display().to_string())
                            .unwrap_or_else(|_| "?".into());
                        info!(link = %name_str, target = %target, "stable render device path");
                    }
                }
            }
        }
    } else {
        info!("no /dev/dri directory — no GPU devices detected");
    }

    info!("── Hardware acceleration probe");
}

/// Check that a directory exists and is readable and writable by the current
/// process.  Logs a clear pass/fail line for each check.
fn check_directory_access(label: &str, path: &Path) {
    // Display the canonical (resolved) path when possible; fall back to the
    // raw configured path if canonicalization fails (e.g. broken symlink).
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    info!(label, path = %canonical.display(), "checking directory access");

    // Existence
    if !path.exists() {
        warn!(label, path = %canonical.display(), "directory does not exist");
        return;
    }

    // Metadata (readability)
    match std::fs::metadata(path) {
        Ok(meta) => {
            if !meta.is_dir() {
                warn!(label, path = %canonical.display(), "path exists but is not a directory");
                return;
            }
        }
        Err(e) => {
            warn!(label, path = %canonical.display(), error = %e, "cannot read metadata");
            return;
        }
    }

    // Read check (can we list contents?)
    if let Err(e) = std::fs::read_dir(path) {
        warn!(label, path = %canonical.display(), error = %e, "directory not readable");
        return;
    }

    // Write check (try creating and removing a temp file)
    let probe = path.join(format!(".starfin_healthcheck_probe_{}", std::process::id()));
    match std::fs::write(&probe, b"healthcheck") {
        Ok(_) => {
            info!(label, path = %canonical.display(), "directory is readable and writable");
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) => {
            warn!(label, path = %canonical.display(), error = %e, "directory not writable");
        }
    }
}

// ── Embedded frontend assets ─────────────────────────────────────────────────

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

// ── Models ───────────────────────────────────────────────────────────────────

/// Matches the `Element` struct used by the frontend.
#[derive(Clone, Serialize, Deserialize)]
struct VideoItem {
    id: String,
    title: String,
    description: String,
    genre: String,
    tags: Vec<String>,
    rating: f64,
    year: u16,
    duration_secs: u32,
    director: String,
    /// Unix timestamp (seconds) of the file's last modification time.
    date_added: u64,
    /// Whether the user has favorited this media file.
    #[serde(default)]
    favorite: bool,
    /// User-defined list of actors / people appearing in the media.
    #[serde(default)]
    actors: Vec<String>,
    /// User-defined genre / category labels.
    #[serde(default)]
    categories: Vec<String>,
}

// ── Cache eviction constants ─────────────────────────────────────────────────

/// How long a video's segments may sit in cache without a new request before
/// they are automatically removed.
const CACHE_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60); // 10 minutes

/// How often the background sweep task wakes up to evict idle caches.
const CACHE_SWEEP_INTERVAL: Duration = Duration::from_secs(60); // 1 minute

/// How long after the last segment request before playback is considered
/// inactive and background workers are allowed to resume.
const PLAYBACK_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

// ── Cache strategy ───────────────────────────────────────────────────────────

/// Controls the caching behaviour of the segment pre-cache worker and the
/// cache-eviction logic.  Selected via the `CACHE_STRATEGY` environment
/// variable (default: `balanced`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheStrategy {
    /// No pre-caching.  Segments are transcoded strictly on demand and are
    /// aggressively evicted after use.  Best for fast disk arrays.
    OnDemand,
    /// Pre-cache the first [`PRECACHE_SEGMENTS`] segments for instant
    /// playback start, and keep every [`SPARSE_CACHE_STRIDE`]-th segment as
    /// a seek anchor.  All other on-demand segments are evicted.  This is
    /// the default.
    Balanced,
    /// Pre-transcode and cache every segment at every quality level that is
    /// applicable to the video's native resolution.  Eviction is disabled.
    /// Best for slow disk arrays where seek performance is critical.
    Aggressive,
}

impl CacheStrategy {
    fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "on-demand" | "ondemand" => CacheStrategy::OnDemand,
            "aggressive" => CacheStrategy::Aggressive,
            _ => CacheStrategy::Balanced,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            CacheStrategy::OnDemand  => "on-demand",
            CacheStrategy::Balanced  => "balanced",
            CacheStrategy::Aggressive => "aggressive",
        }
    }
}

// ── App state ────────────────────────────────────────────────────────────────

/// Tracks the progress of the thumbnail generation background job.
struct ThumbProgress {
    current: u32,
    total: u32,
    active: bool,
    /// The set of video IDs currently being processed (may be > 1 when running
    /// in parallel).  Empty when idle.
    current_ids: HashSet<String>,
}

/// Tracks the progress of the sprite generation background job.
struct SpriteProgress {
    current: u32,
    total: u32,
    active: bool,
    /// The set of video IDs currently being processed (may be > 1 when running
    /// in parallel).  Empty when idle.
    current_ids: HashSet<String>,
}

/// Tracks the progress of the segment pre-caching background job.
struct PrecacheProgress {
    current: u32,
    total: u32,
    active: bool,
    /// The video ID currently being pre-cached, or `None` when idle.
    current_id: Option<String>,
}

impl PrecacheProgress {
    /// Mark the current video as finished and advance the counter.
    fn advance(&mut self) {
        self.current_id = None;
        self.current += 1;
        if self.current >= self.total {
            self.active = false;
        }
    }
}

/// Maps a stable video ID to its `(absolute_path, relative_path)`.
type VideoPathIndex = HashMap<String, (PathBuf, String)>;

struct AppState {
    library_path: PathBuf,
    cache_dir: PathBuf,
    video_cache: Arc<RwLock<Vec<VideoItem>>>,
    /// In-memory lookup table: video ID → (absolute path, relative path).
    /// Built at startup and refreshed on every scan so that `find_video` is O(1).
    video_path_index: Arc<RwLock<VideoPathIndex>>,
    /// Tracks the last time a segment was served for each video ID.
    /// Used by the background idle-eviction sweep.
    last_segment_access: RwLock<HashMap<String, Instant>>,
    /// Progress counters for the background thumbnail generation worker.
    thumb_progress: Arc<RwLock<ThumbProgress>>,
    /// Notified to (re-)start the thumbnail generation batch.
    thumb_trigger: Arc<tokio::sync::Notify>,
    /// Progress counters for the background sprite generation worker.
    sprite_progress: Arc<RwLock<SpriteProgress>>,
    /// Notified to (re-)start the sprite generation batch.
    sprite_trigger: Arc<tokio::sync::Notify>,
    /// Progress counters for the background segment pre-caching worker.
    precache_progress: Arc<RwLock<PrecacheProgress>>,
    /// Notified to (re-)start the segment pre-caching batch.
    precache_trigger: Arc<tokio::sync::Notify>,
    /// Detected hardware acceleration backend.  Starts as `Software` and is
    /// updated in the background once GPU probe completes, so the server can
    /// begin accepting requests immediately.
    hwaccel: Arc<RwLock<HwAccel>>,
    /// Semaphore limiting the number of concurrent on-demand segment transcode
    /// operations.  Used exclusively by the `get_segment` handler for real-time
    /// playback requests.  The pre-cache background worker runs sequentially and
    /// does not compete for these permits.
    /// The limit is set at startup from `TRANSCODE_CONCURRENCY` (default:
    /// available CPU parallelism).
    transcode_semaphore: Arc<tokio::sync::Semaphore>,
    /// Broadcasts playback state (`true` = playing, `false` = idle) to all
    /// background workers so they can pause while a video is being streamed.
    playback_tx: Arc<tokio::sync::watch::Sender<bool>>,
    /// Whether password protection is enabled.
    password_protection: bool,
    /// Path to the `.hash` file inside the cache directory.
    password_hash_path: PathBuf,
    /// In-memory set of valid session tokens.
    auth_tokens: Arc<RwLock<HashSet<String>>>,
    /// In-flight segment transcode deduplication map.
    ///
    /// Maps `(video_id, seg_index, quality)` to a watch-channel sender whose
    /// current value transitions from `None` (pending) to
    /// `Some(Ok(()))` / `Some(Err(_))` once the single authoritative
    /// transcode job finishes.  All concurrent requests for the same segment
    /// subscribe to the same channel and await the result, so only one ffmpeg
    /// job is ever spawned per segment at a time.
    segment_inflight: Arc<Mutex<HashMap<(String, usize, Quality), Arc<tokio::sync::watch::Sender<Option<Result<(), String>>>>>>>,
    /// In-flight demuxed video segment transcode deduplication.
    /// Maps `(video_id, seg_index, quality)` for the `/api/videos/{id}/video/{q}/seg_N.m4s` route.
    video_segment_inflight: Arc<Mutex<HashMap<(String, usize, Quality), Arc<tokio::sync::watch::Sender<Option<Result<(), String>>>>>>>,
    /// In-flight demuxed audio segment transcode deduplication.
    /// Maps `(video_id, seg_index)` for the `/api/videos/{id}/audio/seg_N.m4s` route.
    audio_segment_inflight: Arc<Mutex<HashMap<(String, usize), Arc<tokio::sync::watch::Sender<Option<Result<(), String>>>>>>>,
    /// Pre-rendered CSS for the active theme (served at `/api/theme.css`).
    theme_css: String,
    /// Last known playback positions per video ID (populated by player_ws).
    /// Used for resume-on-reload support.
    playback_positions: Arc<RwLock<HashMap<String, f64>>>,
    /// Monotonically increasing counter bumped every time `video_cache` is
    /// updated (startup scan, periodic scan, manual scan, metadata edit).
    /// Streamed to the frontend via the progress WebSocket so it can re-fetch
    /// the video list immediately instead of polling.
    library_version: Arc<AtomicU64>,
    /// Active caching strategy (from `CACHE_STRATEGY` env var).
    cache_strategy: CacheStrategy,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Stable, deterministic video ID derived from the relative path.
fn video_id(rel_path: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, rel_path.as_bytes()).to_string()
}

/// Returns `true` for file extensions we treat as video.
fn is_video(path: &Path) -> bool {
    const EXTS: &[&str] = &[
        "mp4", "mkv", "avi", "mov", "webm", "m4v", "flv", "wmv", "ts", "m2ts",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Returns the file's modification time as a Unix timestamp (seconds).
/// Falls back to `0` if metadata is unavailable.
fn file_date_added(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Metadata probing (via ffmpeg-next in-process) ────────────────────────────

/// Alias for the metadata struct from the media module.
type FfprobeMeta = media::probe::ProbeMeta;

/// Probe a video file for its duration and metadata using the in-process
/// ffmpeg-next library (replaces the old `ffprobe` subprocess call).
///
/// Because ffmpeg-next calls are synchronous (they block while reading the
/// file header), we run them on a blocking Tokio thread so the async runtime
/// is not starved.
async fn probe_video(path: &Path) -> (f64, FfprobeMeta) {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || media::probe::probe_video(&path))
        .await
        .unwrap_or((0.0, FfprobeMeta::default()))
}

// ── Library scanning ─────────────────────────────────────────────────────────

/// Maximum number of `probe_video` calls that run concurrently during a library
/// scan.  Keeping this bounded avoids thrashing the disk and the blocking
/// thread-pool when a library has thousands of files.
const SCAN_CONCURRENCY: usize = 8;

/// Path of the persisted video-index file inside `cache_dir`.
fn video_index_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(".video_index.json")
}

/// Remove any orphaned `*.tmp` files left behind by a previous unclean
/// shutdown (e.g. SIGKILL while a transcode or sprite write was in progress).
/// These files are always incomplete and can never be reused.
///
/// The cache tree is at most two levels deep:
///   {cache_dir}/{video_id}_thumbs/sprite.tmp.jpg     (name contains ".tmp.")
///   {cache_dir}/{video_id}/{quality}/.seg_XXXXX.m4s.tmp  (extension == "tmp")
/// so a two-level walk is sufficient.
fn cleanup_orphaned_tmp_files(cache_dir: &Path) {
    fn is_tmp(path: &Path) -> bool {
        path.extension().map_or(false, |e| e == "tmp")
            || path
                .file_name()
                .and_then(|n| n.to_str())
                .map_or(false, |n| n.contains(".tmp."))
    }

    let top = match std::fs::read_dir(cache_dir) {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in top.flatten() {
        let path = entry.path();
        if path.is_file() {
            if is_tmp(&path) {
                let _ = std::fs::remove_file(&path);
            }
        } else if path.is_dir() {
            // One level down: {video_id}_thumbs/ and {video_id}/
            let mid = match std::fs::read_dir(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            for mid_entry in mid.flatten() {
                let mid_path = mid_entry.path();
                if mid_path.is_file() {
                    if is_tmp(&mid_path) {
                        let _ = std::fs::remove_file(&mid_path);
                    }
                } else if mid_path.is_dir() {
                    // Two levels down: {video_id}/{quality}/
                    let deep = match std::fs::read_dir(&mid_path) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                    for deep_entry in deep.flatten() {
                        let deep_path = deep_entry.path();
                        if deep_path.is_file() && is_tmp(&deep_path) {
                            let _ = std::fs::remove_file(&deep_path);
                        }
                    }
                }
            }
        }
    }
}

/// Persist the current in-memory video list to disk so it can be restored
/// on the next startup without waiting for a full re-scan.
fn save_video_cache(items: &[VideoItem], cache_dir: &Path) {
    let path = video_index_path(cache_dir);
    match serde_json::to_string(items) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                warn!(path = %path.display(), error = %e, "could not save video index");
            }
        }
        Err(e) => warn!(error = %e, "could not serialize video index"),
    }
}

/// Load a previously-persisted video list from disk.
/// Returns an empty `Vec` if the file does not exist or cannot be parsed.
fn load_video_cache(cache_dir: &Path) -> Vec<VideoItem> {
    let path = video_index_path(cache_dir);
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
            warn!(path = %path.display(), error = %e, "could not parse video index");
            Vec::new()
        }),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "could not read video index");
            Vec::new()
        }
    }
}

/// Walk the library once and build a lookup table of ID → (absolute path, relative path).
/// This is a fast, probe-free pass used at startup and after each scan.
fn build_video_index(library_path: &Path) -> VideoPathIndex {
    WalkDir::new(library_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_video(e.path()))
        .map(|e| {
            let abs = e.path().to_path_buf();
            let rel = abs
                .strip_prefix(library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            let id = video_id(&rel);
            (id, (abs, rel))
        })
        .collect()
}

async fn scan_library(library_path: &Path) -> (Vec<VideoItem>, VideoPathIndex) {
    let entries: Vec<_> = WalkDir::new(library_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_video(e.path()))
        .collect();

    let semaphore = Arc::new(tokio::sync::Semaphore::new(SCAN_CONCURRENCY));
    // Each task returns the VideoItem plus its (abs, rel) paths for the index.
    let mut tasks: tokio::task::JoinSet<(VideoItem, PathBuf, String)> =
        tokio::task::JoinSet::new();

    for entry in entries {
        let abs = entry.path().to_path_buf();
        let rel = abs
            .strip_prefix(library_path)
            .unwrap_or(&abs)
            .to_string_lossy()
            .to_string();

        // Humanise filename as a fallback title
        let fallback_title = abs
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .replace(['.', '_', '-'], " ");

        let id = video_id(&rel);
        let sem = Arc::clone(&semaphore);

        tasks.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let (duration_secs, meta) = probe_video(&abs).await;
            let item = VideoItem {
                id,
                title: meta.title.unwrap_or(fallback_title),
                description: String::new(),
                genre: meta.genre.unwrap_or_default(),
                tags: vec![],
                rating: 0.0,
                year: meta.year.unwrap_or(0),
                duration_secs: duration_secs as u32,
                director: meta.director.unwrap_or_default(),
                date_added: file_date_added(&abs),
                favorite: false,
                actors: vec![],
                categories: vec![],
            };
            (item, abs, rel)
        });
    }

    let mut items = Vec::new();
    let mut index = HashMap::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((item, abs, rel)) => {
                index.insert(item.id.clone(), (abs, rel));
                items.push(item);
            }
            Err(e) => error!(error = %e, "probe task panicked during library scan"),
        }
    }
    (items, index)
}

/// Carry over user-edited metadata fields from a previous cache snapshot into
/// freshly-scanned items.  This ensures that favorites, tags, actors, and
/// categories survive library rescans.
fn merge_user_metadata(items: &mut [VideoItem], previous: &[VideoItem]) {
    let prev_map: HashMap<&str, &VideoItem> = previous.iter().map(|v| (v.id.as_str(), v)).collect();
    for item in items.iter_mut() {
        if let Some(prev) = prev_map.get(item.id.as_str()) {
            item.favorite = prev.favorite;
            item.rating = prev.rating;
            item.tags.clone_from(&prev.tags);
            item.actors.clone_from(&prev.actors);
            item.categories.clone_from(&prev.categories);
        }
    }
}

/// Look up a video by its stable ID using the in-memory path index.
/// Returns `(absolute_path, relative_path)` when found.
fn find_video(state: &AppState, id: &str) -> Option<(PathBuf, String)> {
    state
        .video_path_index
        .read()
        .get(id)
        .cloned()
}

// ── API handlers ─────────────────────────────────────────────────────────────

/// `GET /api/videos` — list all videos with metadata (served from cache).
async fn list_videos(state: web::Data<AppState>) -> impl Responder {
    let items = state.video_cache.read().clone();
    HttpResponse::Ok().json(serde_json::json!({ "items": items }))
}

/// Request body for `PATCH /api/videos/{id}/metadata`.
#[derive(Deserialize)]
struct UpdateMetadataRequest {
    #[serde(default)]
    favorite: Option<bool>,
    #[serde(default)]
    rating: Option<f64>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    actors: Option<Vec<String>>,
    #[serde(default)]
    categories: Option<Vec<String>>,
}

/// `PATCH /api/videos/{id}/metadata` — update user-defined metadata for a video.
async fn update_metadata(
    id: web::Path<String>,
    body: web::Json<UpdateMetadataRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let video_id = id.into_inner();
    let mut cache = state.video_cache.write();
    let item = match cache.iter_mut().find(|v| v.id == video_id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().json(serde_json::json!({"error": "video not found"})),
    };

    if let Some(fav) = body.favorite {
        item.favorite = fav;
    }
    if let Some(r) = body.rating {
        item.rating = r.clamp(0.0, 5.0);
    }
    if let Some(ref tags) = body.tags {
        item.tags = tags.iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
    }
    if let Some(ref actors) = body.actors {
        item.actors = actors.iter()
            .map(|a| a.trim().to_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
    }
    if let Some(ref categories) = body.categories {
        item.categories = categories.iter()
            .map(|c| c.trim().to_lowercase())
            .filter(|c| !c.is_empty())
            .collect();
    }

    let updated = item.clone();
    let items_snapshot = cache.clone();
    drop(cache);

    state.library_version.fetch_add(1, Ordering::Relaxed);

    // Persist to disk in the background so the response isn't delayed.
    let cache_dir = state.cache_dir.clone();
    actix_web::rt::spawn(async move {
        save_video_cache(&items_snapshot, &cache_dir);
    });

    HttpResponse::Ok().json(updated)
}

/// `GET /api/scan/ws` — WebSocket endpoint that starts an immediate library scan and
/// streams live progress as JSON text frames: `{"current":N,"total":M}`.
/// The connection closes once the scan completes and the cache has been updated.
async fn scan_ws(
    req: HttpRequest,
    body: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, mut session, _msg_stream) = actix_ws::handle(&req, body)?;

    let library_path = state.library_path.clone();
    let cache_dir = state.cache_dir.clone();
    let video_cache = Arc::clone(&state.video_cache);
    let video_path_index = Arc::clone(&state.video_path_index);
    let thumb_trigger = Arc::clone(&state.thumb_trigger);
    let sprite_trigger = Arc::clone(&state.sprite_trigger);
    let precache_trigger = Arc::clone(&state.precache_trigger);
    let scan_lib_ver = Arc::clone(&state.library_version);

    actix_web::rt::spawn(async move {
        // Snapshot the existing cache so we can preserve user-edited metadata.
        let previous = video_cache.read().clone();

        // Enumerate all video files up-front so we can report a total.
        let entries: Vec<_> = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .collect();

        let total = entries.len() as u32;

        // Send the initial frame so the client knows the total immediately.
        let init_msg = serde_json::json!({"current": 0u32, "total": total}).to_string();
        if session.text(init_msg).await.is_err() {
            return; // Client already disconnected.
        }

        let mut items = Vec::new();

        let semaphore = Arc::new(tokio::sync::Semaphore::new(SCAN_CONCURRENCY));
        // Each task returns the VideoItem plus its (abs, rel) paths for the index.
        let mut tasks: tokio::task::JoinSet<(VideoItem, PathBuf, String)> =
            tokio::task::JoinSet::new();

        for entry in entries {
            let abs = entry.path().to_path_buf();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();

            let fallback_title = abs
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .replace(['.', '_', '-'], " ");

            let id = video_id(&rel);
            let sem = Arc::clone(&semaphore);

            tasks.spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                let (duration_secs, meta) = probe_video(&abs).await;
                let item = VideoItem {
                    id,
                    title: meta.title.unwrap_or(fallback_title),
                    description: String::new(),
                    genre: meta.genre.unwrap_or_default(),
                    tags: vec![],
                    rating: 0.0,
                    year: meta.year.unwrap_or(0),
                    duration_secs: duration_secs as u32,
                    director: meta.director.unwrap_or_default(),
                    date_added: file_date_added(&abs),
                    favorite: false,
                    actors: vec![],
                    categories: vec![],
                };
                (item, abs, rel)
            });
        }

        let mut index = HashMap::new();
        let mut current = 0u32;
        while let Some(result) = tasks.join_next().await {
            let (item, abs, rel) = match result {
                Ok(t) => t,
                Err(e) => {
                    error!(error = %e, "probe task panicked during scan_ws");
                    continue;
                }
            };
            current += 1;
            index.insert(item.id.clone(), (abs, rel));
            // Include the newly-scanned item so the frontend can stream
            // cards into the grid as each file is discovered.
            let msg = serde_json::json!({
                "current": current,
                "total": total,
                "item": &item,
            })
            .to_string();
            items.push(item);
            if session.text(msg).await.is_err() {
                return; // Client disconnected mid-scan.
            }
        }

        // Commit the updated library to the shared cache and persist to disk.
        merge_user_metadata(&mut items, &previous);
        save_video_cache(&items, &cache_dir);
        *video_cache.write() = items;
        *video_path_index.write() = index;
        scan_lib_ver.fetch_add(1, Ordering::Relaxed);

        // Re-trigger thumbnail generation for any newly discovered videos.
        thumb_trigger.notify_one();

        // Re-trigger sprite generation for any newly discovered videos.
        sprite_trigger.notify_one();

        // Re-trigger segment pre-caching for any newly discovered videos.
        precache_trigger.notify_one();

        // Close the WebSocket — the client uses this signal to know the scan is done.
        let _ = session.close(None).await;
    });

    Ok(response)
}

/// `GET /api/videos/{id}/thumbnail` — serve the cached JPEG thumbnail.
///
/// Thumbnails are generated entirely in the background by `run_thumb_worker`
/// (seeks to 20% of the video duration and grabs a single keyframe).
/// If the thumbnail has not yet been generated this returns 404 so
/// callers can handle the not-ready state gracefully.
async fn get_thumbnail(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let thumb_path = state.cache_dir.join(format!("{}.jpg", *id));
    match tokio::fs::read(&thumb_path).await {
        Ok(data) => HttpResponse::Ok()
            .content_type("image/jpeg")
            .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
            .body(data),
        Err(_) => HttpResponse::NotFound().body("thumbnail not ready"),
    }
}

// ── Thumbnail background job ──────────────────────────────────────────────────

/// Fast thumbnail: seek to exactly 20% of the video duration and decode the
/// nearest available frame as JPEG.  Mirrors the approach used by KDE's
/// `ffmpegthumbs`.
async fn generate_thumbnail(
    id: &str,
    video_path: &Path,
    cache_dir: &Path,
) -> bool {
    let thumb_path = cache_dir.join(format!("{}.jpg", id));
    if thumb_path.exists() {
        return true;
    }

    let (duration_secs, _) = probe_video(video_path).await;
    if duration_secs <= 0.0 {
        return false;
    }

    // Seek to 20% of the runtime, clamped to at least 1 second.
    let seek_secs = (duration_secs * 0.20).max(1.0);

    let video_path = video_path.to_path_buf();
    let thumb_path_clone = thumb_path.clone();

    tokio::task::spawn_blocking(move || {
        media::thumbnail::extract_frame_as_jpeg(&video_path, seek_secs, &thumb_path_clone)
    })
    .await
    .unwrap_or(false)
}

/// Returns the number of tasks that sprite/thumbnail background workers will
/// run concurrently.
///
/// Defaults to **1** so that background work never saturates CPU or disk I/O.
/// With concurrency=1 each worker has at most one in-flight `spawn_blocking`
/// task at a time, so when playback starts the worker pauses after finishing
/// just that one task — keeping the overlap with on-demand transcoding to a
/// minimum.
///
/// Override with the `WORKER_CONCURRENCY` environment variable.
fn worker_concurrency() -> usize {
    std::env::var("WORKER_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok().filter(|&n| n > 0))
        .unwrap_or(1)
}

/// Background worker that generates thumbnails for every video in the library.
///
/// For each video whose `.jpg` is absent, seeks to 20% of the runtime and
/// extracts a single keyframe as JPEG — the same approach used by KDE's
/// `ffmpegthumbs`.  This is fast (one short ffmpeg invocation per file) and
/// gives the UI something to show immediately.
///
/// The worker is triggered by a notification on `trigger` (sent at startup
/// and after every library re-scan).  Progress counters are written to
/// `progress` so `GET /api/thumbnails/progress` can drive the frontend bar.
async fn run_thumb_worker(
    library_path: PathBuf,
    cache_dir: PathBuf,
    progress: Arc<RwLock<ThumbProgress>>,
    trigger: Arc<tokio::sync::Notify>,
    mut playback_rx: tokio::sync::watch::Receiver<bool>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let concurrency = worker_concurrency();
    loop {
        // Fast exit if shutdown was already signaled before we block.
        if *shutdown_rx.borrow() {
            return;
        }
        tokio::select! {
            _ = trigger.notified() => {}
            _ = shutdown_rx.changed() => { return; }
        }
        if *shutdown_rx.borrow() {
            return;
        }

        let (done, entries): (Vec<_>, Vec<_>) = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .partition(|e| {
                let abs = e.path();
                let rel = abs
                    .strip_prefix(&library_path)
                    .unwrap_or(abs)
                    .to_string_lossy();
                let id = video_id(&rel);
                cache_dir.join(format!("{}.jpg", id)).exists()
            });

        {
            let mut p = progress.write();
            p.current = done.len() as u32;
            p.total = (done.len() + entries.len()) as u32;
            p.active = !entries.is_empty();
        }

        let mut join_set: tokio::task::JoinSet<(String, bool)> = tokio::task::JoinSet::new();
        let mut iter = entries.into_iter().peekable();
        loop {
            // Suspend while a video is being streamed: wait here until
            // playback goes idle, then fill the next batch of tasks.
            while *playback_rx.borrow() {
                let _ = playback_rx.changed().await;
            }
            if *shutdown_rx.borrow() {
                return;
            }
            // Fill empty slots up to the concurrency limit.
            while join_set.len() < concurrency && iter.peek().is_some() {
                if *shutdown_rx.borrow() {
                    return;
                }
                let entry = iter.next().unwrap();
                let abs = entry.path().to_path_buf();
                let rel = abs
                    .strip_prefix(&library_path)
                    .unwrap_or(&abs)
                    .to_string_lossy()
                    .to_string();
                let id = video_id(&rel);
                {
                    let mut p = progress.write();
                    p.current_ids.insert(id.clone());
                }
                let cache_dir = cache_dir.clone();
                join_set.spawn(async move {
                    let ok = generate_thumbnail(&id, &abs, &cache_dir).await;
                    (id, ok)
                });
            }
            if join_set.is_empty() {
                break;
            }
            // Collect the next completed task.  React immediately to
            // playback or shutdown rather than waiting for the in-flight
            // spawn_blocking to finish.
            let next = tokio::select! {
                r = join_set.join_next() => r,
                _ = playback_rx.changed() => { continue; }
                _ = shutdown_rx.changed() => { return; }
            };
            if let Some(Ok((id, _ok))) = next {
                let mut p = progress.write();
                p.current_ids.remove(&id);
                p.current += 1;
                if p.current >= p.total {
                    p.active = false;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────

/// `GET /api/thumbnails/progress` — current thumbnail generation progress.
///
/// Returns `{"current":N,"total":M,"active":bool}`.
/// The frontend polls this every few seconds to drive the progress bar on the
/// homepage.
#[derive(Clone, Serialize)]
struct ThumbProgressResponse {
    current: u32,
    total: u32,
    active: bool,
}

async fn get_thumb_progress(state: web::Data<AppState>) -> impl Responder {
    let p = state.thumb_progress.read();
    HttpResponse::Ok().json(ThumbProgressResponse {
        current: p.current,
        total: p.total,
        active: p.active,
    })
}

/// `GET /api/progress/ws` — persistent WebSocket that streams live progress
/// updates from the thumbnail, sprite, and pre-cache background workers at
/// 500 ms intervals.
///
/// Each frame is a JSON text message:
/// ```json
/// {
///   "thumb":    { "current": N, "total": M, "active": bool, "current_ids": ["uuid", ...] },
///   "sprite":   { "current": N, "total": M, "active": bool, "current_ids": ["uuid", ...] },
///   "precache": { "current": N, "total": M, "active": bool, "current_id": "uuid"|null }
/// }
/// ```
async fn progress_ws(
    req: HttpRequest,
    body: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, mut session, _msg_stream) = actix_ws::handle(&req, body)?;

    let thumb_progress = Arc::clone(&state.thumb_progress);
    let sprite_progress = Arc::clone(&state.sprite_progress);
    let precache_progress = Arc::clone(&state.precache_progress);
    let library_version = Arc::clone(&state.library_version);

    actix_web::rt::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(500));
        loop {
            ticker.tick().await;

            let (tc, tt, ta, tids) = {
                let p = thumb_progress.read();
                let ids: Vec<String> = p.current_ids.iter().cloned().collect();
                (p.current, p.total, p.active, ids)
            };
            let (sc, st, sa, sids) = {
                let p = sprite_progress.read();
                let ids: Vec<String> = p.current_ids.iter().cloned().collect();
                (p.current, p.total, p.active, ids)
            };
            let (pc, pt, pa, pid) = {
                let p = precache_progress.read();
                (p.current, p.total, p.active, p.current_id.clone())
            };
            let lv = library_version.load(Ordering::Relaxed);

            let msg = serde_json::json!({
                "thumb":    { "current": tc, "total": tt, "active": ta, "current_ids": tids },
                "sprite":   { "current": sc, "total": st, "active": sa, "current_ids": sids },
                "precache": { "current": pc, "total": pt, "active": pa, "current_id": pid },
                "library_version": lv
            })
            .to_string();

            if session.text(msg).await.is_err() {
                break; // Client disconnected.
            }
        }
    });

    Ok(response)
}

// ══════════════════════════════════════════════════════════════════════════════
// Player WebSocket — tight integration between video_player.rs and main.rs
// ══════════════════════════════════════════════════════════════════════════════
//
// This endpoint enables bidirectional communication between the frontend
// video player (powered by dashjs-rs) and the server.
//
// **Client → Server messages:**
//   • `{ "type": "playback_state", "video_id": "...", "time": 1.23, "paused": false }`
//     Reports playback position for resume-on-reload and multi-device sync.
//   • `{ "type": "buffer_health", "video_id": "...", "buffer_level": 12.3, "throughput_kbps": 5000 }`
//     Reports dashjs-rs buffer/throughput metrics for server-side monitoring.
//
// **Server → Client messages (ServerCommand):**
//   • `{ "type": "play" }` / `{ "type": "pause" }`
//   • `{ "type": "seek", "time": 30.0 }`
//   • `{ "type": "set_quality", "quality": "high" }`
//   • `{ "type": "set_volume", "volume": 0.8 }`
//   • `{ "type": "update_source", "video_id": "..." }`
//
// The frontend `apply_server_command()` function in `video_player.rs`
// dispatches these commands to the dashjs-rs MediaPlayer and browser video
// element.
//
// **Architecture:**
//   frontend/video_player.rs → dashjs-rs MediaPlayer → browser MSE
//                             ↕ (WebSocket)
//   src/main.rs (player_ws)  → AppState (playback positions, metrics)

/// Playback state reported by the frontend player.
#[derive(Debug, Clone, Deserialize)]
struct PlaybackReport {
    video_id: String,
    time: f64,
    #[allow(dead_code)]
    paused: bool,
}

/// Buffer health reported by the frontend player (dashjs-rs metrics).
#[derive(Debug, Clone, Deserialize)]
struct BufferHealthReport {
    #[allow(dead_code)]
    video_id: String,
    #[allow(dead_code)]
    buffer_level: f64,
    #[allow(dead_code)]
    throughput_kbps: f64,
}

/// `GET /api/player/ws` — WebSocket for player ↔ server integration.
///
/// Accepts playback state and buffer health reports from the frontend.
/// Can send ServerCommand messages to control playback remotely.
/// Stores the last known playback position per video so the frontend
/// can resume from where the user left off.
async fn player_ws(
    req: HttpRequest,
    body: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, mut session, mut msg_stream) = actix_ws::handle(&req, body)?;

    let playback_positions = Arc::clone(&state.playback_positions);

    actix_web::rt::spawn(async move {
        while let Some(Ok(msg)) = msg_stream.recv().await {
            match msg {
                actix_ws::Message::Text(text) => {
                    // Parse incoming JSON messages from the player
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        match value.get("type").and_then(|t| t.as_str()) {
                            Some("playback_state") => {
                                if let Ok(report) = serde_json::from_value::<PlaybackReport>(value) {
                                    // Store the playback position for resume support
                                    playback_positions.write().insert(report.video_id.clone(), report.time);
                                }
                            }
                            Some("buffer_health") => {
                                // Buffer health from dashjs-rs metrics — logged for monitoring
                                if let Ok(_report) = serde_json::from_value::<BufferHealthReport>(value) {
                                    // Future: aggregate metrics, trigger quality hints
                                }
                            }
                            _ => {
                                // Unknown message type — ignore
                            }
                        }
                    }
                }
                actix_ws::Message::Close(_) => break,
                actix_ws::Message::Ping(data) => {
                    if session.pong(&data).await.is_err() { break; }
                }
                _ => {}
            }
        }
    });

    Ok(response)
}

/// `GET /api/player/position/{id}` — Get the last known playback position.
///
/// Returns `{ "time": <seconds> }` for resume-on-reload support.
/// The position is updated by the player WebSocket.
async fn get_playback_position(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> HttpResponse {
    let video_id = path.into_inner();
    let time = state.playback_positions.read()
        .get(&video_id)
        .copied()
        .unwrap_or(0.0);
    HttpResponse::Ok().json(serde_json::json!({ "time": time }))
}

/// Segment duration in seconds for on-demand DASH segment generation.
/// Apple recommends 6 seconds; common range is 2–10 seconds.
/// Jellyfin/Plex default to 6 second segments.
const SEGMENT_DURATION: f64 = media::transcode::SEGMENT_DURATION;

/// Number of segments at the start of each video to pre-cache so that
/// playback can begin immediately without waiting for on-demand transcoding.
/// At 6 seconds per segment, 20 segments ≈ 2 minutes of video.
const PRECACHE_SEGMENTS: usize = 20;

/// Stride for sparse seek-anchor caching beyond the initial dense pre-cache window.
/// Every Nth segment index (where `idx % SPARSE_CACHE_STRIDE == 0`) will be
/// pre-transcoded as a seek anchor across the full video duration.
/// At 6 seconds per segment and a stride of 3, anchors are placed every 18 seconds.
///
/// NOTE: This value must stay in sync with `SPARSE_CACHE_STRIDE_F` in
/// `frontend/src/components/video_player.rs`.
const SPARSE_CACHE_STRIDE: usize = 3;

/// Transcode a single fMP4 segment for a video using ffmpeg-next
/// (in-process for software encoding, subprocess for GPU-accelerated High
/// quality).
///
/// Segments are stored in a quality-specific subdirectory so that multiple
/// quality levels can be cached independently.
///
/// Writes to a temporary file first, then atomically renames to the final
/// location to prevent readers from seeing partially-written segments.
async fn transcode_segment(
    abs_path: &str,
    seg_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: Quality,
) -> Result<(), String> {
    media::transcode::transcode_segment(abs_path, seg_dir, seg_index, hwaccel, quality).await
}

/// Remove cached segments beyond the pre-cache range from a quality-specific
/// segment directory.  The segments retained depend on the active [`CacheStrategy`]:
///
/// - `OnDemand` — remove **all** segments (nothing is retained).
/// - `Balanced` — keep segments with index < [`PRECACHE_SEGMENTS`] and sparse
///   seek anchors (`idx % SPARSE_CACHE_STRIDE == 0`).
/// - `Aggressive` — keep **all** segments (no eviction).
async fn remove_non_precached_segments(cache_dir: &Path, strategy: CacheStrategy) -> std::io::Result<()> {
    // Aggressive mode: eviction is disabled — keep everything.
    if strategy == CacheStrategy::Aggressive {
        return Ok(());
    }

    let mut entries = match tokio::fs::read_dir(cache_dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Parse segment index from "seg_XXXXX.m4s"
        if let Some(idx) = name_str
            .strip_prefix("seg_")
            .and_then(|s| s.strip_suffix(".m4s"))
            .and_then(|s| s.parse::<usize>().ok())
        {
            let should_keep = match strategy {
                // OnDemand: evict everything.
                CacheStrategy::OnDemand => false,
                // Balanced: keep dense pre-cache window and sparse seek anchors.
                CacheStrategy::Balanced => idx < PRECACHE_SEGMENTS || idx % SPARSE_CACHE_STRIDE == 0,
                // Aggressive already returned early above.
                CacheStrategy::Aggressive => true,
            };
            if !should_keep {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
        // Also clean up any temp files left by the transcoding helper.
        else if name_str.starts_with(".seg_") && name_str.ends_with(".tmp") {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }

    Ok(())
}

/// Remove non-pre-cached segments from **all** quality subdirectories of a
/// video's cache folder (`{cache_dir}/{video_id}/video/{quality}/` and `{cache_dir}/{video_id}/audio/`).
async fn remove_non_precached_segments_all_qualities(video_cache_dir: &Path, strategy: CacheStrategy) {
    // Scan the demuxed video subdirectory for quality-specific dirs.
    let video_dir = video_cache_dir.join("video");
    if let Ok(mut dir) = tokio::fs::read_dir(&video_dir).await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                let q_dir = entry.path();
                if let Err(e) = remove_non_precached_segments(&q_dir, strategy).await {
                    error!(dir = %q_dir.display(), error = %e, "cache eviction error");
                }
            }
        }
    }
    // Also clean the audio directory (quality-independent).
    let audio_dir = video_cache_dir.join("audio");
    if audio_dir.exists() {
        if let Err(e) = remove_non_precached_segments(&audio_dir, strategy).await {
            error!(dir = %audio_dir.display(), error = %e, "cache eviction error");
        }
    }
}

/// Compute the greatest common divisor of two u32 values.
fn gcd_u32(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd_u32(b, a % b) }
}

/// `GET /api/videos/{id}/manifest.mpd`
///
/// Generates a DASH-IF IOP v5 compliant MPD manifest for VOD playback.
///
/// Uses separate audio and video AdaptationSets (demuxed) with:
/// - Video AdaptationSet: timescale=90000, all quality Representations
/// - Audio AdaptationSet: timescale=audio_sample_rate, one Representation
///
/// The `?quality=` query parameter is accepted for backward compatibility
/// but ignored — all qualities are included as Representations in the MPD.
async fn get_manifest(
    id: web::Path<String>,
    _query: web::Query<QualityQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let (duration_secs, _metadata) = probe_video(&abs_path).await;
    if duration_secs <= 0.0 {
        return HttpResponse::ServiceUnavailable()
            .body("Could not determine video duration.");
    }

    let abs_for_codec = abs_path.clone();
    let codec_info = tokio::task::spawn_blocking(move || {
        media::probe::probe_codecs(&abs_for_codec)
    })
    .await
    .unwrap_or_default();

    let abs_for_stream = abs_path.clone();
    let stream_info = tokio::task::spawn_blocking(move || {
        media::probe::probe_stream_info(&abs_for_stream)
    })
    .await
    .unwrap_or_default();

    // Determine which quality tiers to include based on source resolution.
    // Only include transcoded resolutions ≤ the source height (no upscaling).
    // "Original (Direct Copy)" is served via /api/videos/{id}/stream instead
    // of DASH segments, so it is intentionally excluded from the MPD.
    // This ensures ABR (autoSwitchBitrate) only switches between cached,
    // transcoded representations and never accidentally selects the original
    // source stream (which may use HEVC or other non-universal codecs).
    let source_height = stream_info.height.max(1);
    let res_qualities: &[(Quality, u32)] = &[
        (Quality::Q2160, 2160),
        (Quality::Q1080, 1080),
        (Quality::Q720,  720),
        (Quality::Q480,  480),
        (Quality::Q360,  360),
    ];
    let mut video_qualities: Vec<Quality> = Vec::new();
    for &(q, target_h) in res_qualities {
        if source_height >= target_h {
            video_qualities.push(q);
        }
    }

    // Ensure the video and audio cache directories exist for all applicable tiers.
    for &quality in &video_qualities {
        let video_seg_dir = state.cache_dir.join(id.as_str()).join("video").join(quality.as_str());
        if let Err(e) = tokio::fs::create_dir_all(&video_seg_dir).await {
            return HttpResponse::InternalServerError()
                .body(format!("cache dir error: {e}"));
        }
    }
    let audio_seg_dir = state.cache_dir.join(id.as_str()).join("audio");
    if let Err(e) = tokio::fs::create_dir_all(&audio_seg_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    let duration = duration_secs;
    let audio_sample_rate = if stream_info.audio_sample_rate > 0 {
        stream_info.audio_sample_rate
    } else {
        48000
    };
    let has_audio = codec_info.audio_codec.is_some();

    // Use nominal segment boundaries — standard DASH practice.
    // Each segment is exactly SEGMENT_DURATION seconds (6s), except possibly
    // the last one.  The segment generator seeks to the nearest keyframe and
    // patches the tfdt to match.  dash.js gap-jumping handles any residual
    // timing differences.
    let num_segments = (duration / SEGMENT_DURATION).ceil().max(1.0) as usize;

    // Format ISO 8601 duration.
    let hours = (duration as u64) / 3600;
    let minutes = ((duration as u64) % 3600) / 60;
    let frac_seconds = duration - (hours * 3600 + minutes * 60) as f64;
    let pt_duration = format!("PT{hours}H{minutes}M{frac_seconds:.3}S");

    // Estimate max segment duration for MPD attribute.
    let max_seg_dur = SEGMENT_DURATION.ceil() as u64;

    // Build video Representations.
    let (w_orig, h_orig) = (stream_info.width.max(1), stream_info.height.max(1));

    // Frame rate from stream info.
    let fps_str = "30"; // simplified; could probe if needed

    // Build SegmentTimeline for video (timescale=90000) using nominal durations.
    let mut video_durations: Vec<u64> = Vec::with_capacity(num_segments);
    for i in 0..num_segments {
        let seg_start = i as f64 * SEGMENT_DURATION;
        let seg_end = ((i + 1) as f64 * SEGMENT_DURATION).min(duration);
        let dur_ticks = ((seg_end - seg_start) * 90000.0).round().max(1.0) as u64;
        video_durations.push(dur_ticks);
    }

    // Build SegmentTimeline for audio (timescale=audio_sample_rate) using nominal durations.
    let mut audio_durations: Vec<u64> = Vec::with_capacity(num_segments);
    for i in 0..num_segments {
        let seg_start = i as f64 * SEGMENT_DURATION;
        let seg_end = ((i + 1) as f64 * SEGMENT_DURATION).min(duration);
        let dur_ticks = ((seg_end - seg_start) * audio_sample_rate as f64).round().max(1.0) as u64;
        audio_durations.push(dur_ticks);
    }

    fn build_segment_timeline(durations: &[u64]) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < durations.len() {
            let d = durations[i];
            let mut count = 1usize;
            while i + count < durations.len() && durations[i + count] == d {
                count += 1;
            }
            if count > 1 {
                out.push_str(&format!("            <S d=\"{d}\" r=\"{}\"/>\n", count - 1));
            } else {
                out.push_str(&format!("            <S d=\"{d}\"/>\n"));
            }
            i += count;
        }
        out
    }

    let video_timeline = build_segment_timeline(&video_durations);
    let audio_timeline = build_segment_timeline(&audio_durations);
    let audio_codec = codec_info.audio_codec.as_deref().unwrap_or("mp4a.40.2");

    // GCD of width/height for par attribute.
    let g = gcd_u32(w_orig, h_orig);
    let (par_w, par_h) = (w_orig / g, h_orig / g);

    let mut mpd = String::new();
    mpd.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    mpd.push_str(&format!(
        "<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\"\n\
         \x20    profiles=\"urn:mpeg:dash:profile:isoff-live:2011\"\n\
         \x20    type=\"static\"\n\
         \x20    mediaPresentationDuration=\"{pt_duration}\"\n\
         \x20    minBufferTime=\"PT4S\"\n\
         \x20    maxSegmentDuration=\"PT{max_seg_dur}S\">\n"
    ));
    mpd.push_str(&format!("  <Period id=\"1\" start=\"PT0S\" duration=\"{pt_duration}\">\n"));

    // ── Video AdaptationSet ──
    mpd.push_str(&format!(
        "    <AdaptationSet id=\"1\" contentType=\"video\" mimeType=\"video/mp4\"\n\
         \x20    segmentAlignment=\"true\" subsegmentAlignment=\"true\"\n\
         \x20    subsegmentStartsWithSAP=\"1\" startWithSAP=\"1\"\n\
         \x20    par=\"{par_w}:{par_h}\">\n"
    ));
    mpd.push_str(&format!(
        "      <SegmentTemplate timescale=\"90000\"\n\
         \x20        initialization=\"/api/videos/{id}/video/$RepresentationID$/init.mp4\"\n\
         \x20        media=\"/api/videos/{id}/video/$RepresentationID$/seg_$Number%05d$.m4s\"\n\
         \x20        startNumber=\"1\">\n",
        id = *id
    ));
    mpd.push_str("        <SegmentTimeline>\n");
    mpd.push_str(&video_timeline);
    mpd.push_str("        </SegmentTimeline>\n");
    mpd.push_str("      </SegmentTemplate>\n");

    // Transcoded resolution representations — all use H.264 avc1.640029.
    for &quality in video_qualities.iter() {
        let bw = estimate_bandwidth(&stream_info, quality);
        let (w, h) = estimate_resolution(&stream_info, quality);
        mpd.push_str(&format!(
            "      <Representation id=\"{qid}\" bandwidth=\"{bw}\" \
             width=\"{w}\" height=\"{h}\" codecs=\"avc1.640029\" frameRate=\"{fps_str}\"/>\n",
            qid = quality.as_str()
        ));
    }
    mpd.push_str("    </AdaptationSet>\n");

    // ── Audio AdaptationSet (omit if no audio stream) ──
    if has_audio {
        mpd.push_str(&format!(
            "    <AdaptationSet id=\"2\" contentType=\"audio\" mimeType=\"audio/mp4\"\n\
             \x20    segmentAlignment=\"true\" subsegmentAlignment=\"true\"\n\
             \x20    lang=\"und\">\n"
        ));
        mpd.push_str(&format!(
            "      <SegmentTemplate timescale=\"{audio_sample_rate}\"\n\
             \x20        initialization=\"/api/videos/{id}/audio/init.mp4\"\n\
             \x20        media=\"/api/videos/{id}/audio/seg_$Number%05d$.m4s\"\n\
             \x20        startNumber=\"1\">\n",
            audio_sample_rate = audio_sample_rate,
            id = *id
        ));
        mpd.push_str("        <SegmentTimeline>\n");
        mpd.push_str(&audio_timeline);
        mpd.push_str("        </SegmentTimeline>\n");
        mpd.push_str("      </SegmentTemplate>\n");
        mpd.push_str(&format!(
            "      <Representation id=\"audio\" bandwidth=\"256000\" \
             codecs=\"{audio_codec}\" audioSamplingRate=\"{audio_sample_rate}\"/>\n"
        ));
        mpd.push_str("    </AdaptationSet>\n");
    }

    mpd.push_str("  </Period>\n");
    mpd.push_str("</MPD>\n");

    HttpResponse::Ok()
        .content_type("application/dash+xml")
        .insert_header((header::CACHE_CONTROL, "no-cache"))
        .body(mpd)
}

/// `GET /api/videos/{id}/init.mp4` — serve the fMP4 init segment.
///
/// The init segment contains ftyp + moov atoms with codec configuration
/// (SPS/PPS for H.264, channel layout for AAC) that the browser's MSE
/// SourceBuffer needs before any media segments can be appended.
///
/// Init segments are cached per-quality in `{cache_dir}/{id}/{quality}/init.mp4`.
async fn get_init_segment(
    id: web::Path<String>,
    query: web::Query<QualityQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let quality = query.quality;

    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let seg_dir = state.cache_dir.join(id.as_str()).join(quality.as_str());
    let init_path = seg_dir.join("init.mp4");
    let seg0_path = seg_dir.join("seg_00000.m4s");

    // ── Always derive the init segment from segment 0 ────────────────────
    //
    // The init segment (ftyp+moov) MUST come from the same FFmpeg run that
    // produced the media segments.  Firefox's H264ChangeMonitor compares
    // the avcC (SPS/PPS) in the init's moov against the media segment
    // data; any mismatch causes NS_ERROR_DOM_MEDIA_FATAL_ERR.
    //
    // Instead of blindly serving a cached init.mp4 (which may be stale —
    // e.g. from a previous server version that used a separate FFmpeg run),
    // we always re-derive the init from segment 0.  Extraction is fast
    // (~microseconds, just parsing MP4 box headers).
    //
    // If segment 0 exists in cache → extract ftyp+moov from it directly.
    // If not → generate segment 0, then extract.
    // Either way, the result is written to init.mp4 for future requests.
    if seg0_path.exists() {
        // Fast path: derive init from the cached segment 0 file.
        let seg0_path_clone = seg0_path.clone();
        let init_data = match tokio::task::spawn_blocking(move || {
            let data = std::fs::read(&seg0_path_clone)
                .map_err(|e| format!("read segment 0: {e}"))?;
            media::transcode::extract_ftyp_moov_pub(&data)
        }).await {
            Ok(Ok(data)) => data,
            Ok(Err(e)) => {
                // Segment 0 exists but is corrupt — delete it and fall through
                // to regeneration below.
                warn!(error = %e, "cached segment 0 corrupt, regenerating");
                let _ = tokio::fs::remove_file(&seg0_path).await;
                let _ = tokio::fs::remove_file(&init_path).await;
                // Fall through to generation path below
                return get_init_segment_generate(
                    abs_path, quality, &state, seg_dir, init_path,
                ).await;
            }
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .body(format!("init extraction task panicked: {e}"));
            }
        };

        // Update the cached init.mp4 to match current segment 0.
        let _ = tokio::fs::write(&init_path, &init_data).await;

        return HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(init_data);
    }

    // No segment 0 in cache — generate it.
    get_init_segment_generate(abs_path, quality, &state, seg_dir, init_path).await
}

/// Generate init segment from scratch (creates segment 0 if needed).
async fn get_init_segment_generate(
    abs_path: std::path::PathBuf,
    quality: media::transcode::Quality,
    state: &web::Data<AppState>,
    seg_dir: std::path::PathBuf,
    init_path: std::path::PathBuf,
) -> HttpResponse {
    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };
    let abs_str = match resolved_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
    };

    if let Err(e) = tokio::fs::create_dir_all(&seg_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    let hwaccel = state.hwaccel.read().clone();
    let seg_dir_clone = seg_dir.clone();
    let init_data = match tokio::task::spawn_blocking(move || {
        media::transcode::create_init_segment(&abs_str, quality, &hwaccel, &seg_dir_clone)
    }).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            error!(error = %e, "init segment generation failed");
            return HttpResponse::ServiceUnavailable()
                .body(format!("init segment generation failed: {e}"));
        }
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("init segment task panicked: {e}"));
        }
    };

    // Cache the init segment for future requests.
    let _ = tokio::fs::write(&init_path, &init_data).await;

    HttpResponse::Ok()
        .content_type("video/mp4")
        .insert_header((
            header::CACHE_CONTROL,
            "public, max-age=31536000, immutable",
        ))
        .body(init_data)
}

/// `GET /api/videos/{id}/segments/{filename}` — serve an fMP4 media segment on-demand.
///
/// Accepts an optional `?quality=high|medium|low` query parameter (default:
/// `original`).  Segments are stored in and served from a quality-specific
/// subdirectory so that different quality caches never interfere with each
/// other.
///
/// Segments are transcoded on-demand if they don't exist in the cache.
/// Uses fMP4 (CMAF) format for DASH-compatible streaming with MSE.
async fn get_segment(
    params: web::Path<(String, String)>,
    query: web::Query<QualityQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, filename) = params.into_inner();
    let quality = query.quality;

    // Reject path traversal and unexpected extensions.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return HttpResponse::BadRequest().body("invalid filename");
    }
    if !filename.ends_with(".m4s") {
        return HttpResponse::BadRequest().body("invalid segment type");
    }

    // Segments are stored in a quality-specific subdirectory.
    let seg_dir = state.cache_dir.join(&id).join(quality.as_str());
    let seg_path = seg_dir.join(&filename);

    // Record that this video was actively streamed right now so the
    // idle-eviction sweep resets its 10-minute countdown.
    {
        let mut map = state
            .last_segment_access
            .write();
        map.insert(id.clone(), Instant::now());
    }
    // Signal to background workers that playback is in progress.
    state.playback_tx.send_if_modified(|v| {
        if *v { false } else { *v = true; true }
    });

    // If segment exists, serve it immediately from cache.
    // Strip ftyp+moov init boxes so the SourceBuffer only receives moof+mdat
    // fragments — the init segment is served separately via /init.mp4.
    // Without this, the browser re-initialises the decoder at every segment
    // boundary, causing visible stutter (matching dash.js SourceBufferSink
    // which never sends duplicate init segments during normal playback).
    if let Ok(data) = tokio::fs::read(&seg_path).await {
        let stripped = media::transcode::strip_init_boxes(&data);
        return HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(stripped);
    }

    // Parse segment index from filename (e.g., "seg_00042.m4s" -> 42)
    let seg_index: usize = match filename
        .strip_prefix("seg_")
        .and_then(|s| s.strip_suffix(".m4s"))
        .and_then(|s| s.parse().ok())
    {
        Some(idx) => idx,
        None => return HttpResponse::BadRequest().body("invalid segment filename format"),
    };

    // Find the source video
    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };
    let abs_str = match resolved_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
    };

    // Create cache directory if needed
    if let Err(e) = tokio::fs::create_dir_all(&seg_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    // ── Segment-request deduplication ────────────────────────────────────────
    // Only one transcode job per (video_id, seg_index, quality) should run at a
    // time.  Concurrent requests for the same segment subscribe to a shared
    // tokio::sync::watch channel that the "owner" of the job writes to once the
    // transcode finishes (or fails).
    let inflight_key = (id.clone(), seg_index, quality);

    // Acquire the inflight map just long enough to check/insert – never across
    // an await point.
    let maybe_rx = {
        let mut map = state
            .segment_inflight
            .lock();
        if let Some(tx) = map.get(&inflight_key) {
            // Another request is already transcoding this segment – subscribe.
            Some(tx.subscribe())
        } else {
            // We are the first – register a new channel in the map.
            let (tx, _initial_rx) =
                tokio::sync::watch::channel::<Option<Result<(), String>>>(None);
            map.insert(inflight_key.clone(), Arc::new(tx));
            None
        }
    };

    let transcode_result: Result<(), String> = if let Some(mut rx) = maybe_rx {
        // Wait for the owner's transcode to produce a result.
        loop {
            // Sender dropped means the owner's task was cancelled/panicked.
            if rx.changed().await.is_err() {
                break Err(format!("segment {seg_index} transcoding cancelled"));
            }
            if let Some(result) = rx.borrow().clone() {
                break result;
            }
        }
    } else {
        // We own the transcode job — acquire a permit to bound concurrent
        // transcode operations before starting.  The permit is released
        // automatically when `_permit` is dropped.
        let _permit = match state.transcode_semaphore.acquire().await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "transcode semaphore closed during request — server may be shutting down");
                // Broadcast failure to any subscribers waiting on this key.
                let tx = state
                    .segment_inflight
                    .lock()
                    .remove(&inflight_key);
                if let Some(tx) = tx {
                    let _ = tx.send(Some(Err(format!("semaphore closed"))));
                }
                return HttpResponse::InternalServerError().body("transcode unavailable");
            }
        };

        // We own the transcode job.
        // Use nominal boundaries — segment generator seeks to nearest keyframe.
        let result = media::transcode::transcode_segment_with_boundaries(
            &abs_str, &seg_dir, seg_index, &state.hwaccel.read().clone(), quality, None,
        ).await;

        // Remove from the inflight map first (no new subscribers can join
        // after this point) then broadcast the result to existing waiters.
        let tx = state
            .segment_inflight
            .lock()
            .remove(&inflight_key);
        if let Some(tx) = tx {
            let _ = tx.send(Some(result.clone()));
        }

        result
    };

    // Serve the segment from the transcode result (either from our own job or
    // from waiting on a concurrent request that owned the transcode).
    match transcode_result {
        Ok(()) => {
            match tokio::fs::read(&seg_path).await {
                Ok(data) => {
                    let stripped = media::transcode::strip_init_boxes(&data);
                    HttpResponse::Ok()
                        .content_type("video/mp4")
                        .insert_header((
                            header::CACHE_CONTROL,
                            "public, max-age=31536000, immutable",
                        ))
                        .body(stripped)
                }
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("failed to read generated segment: {e}")),
            }
        }
        Err(msg) => {
            error!(error = %msg, segment = seg_index, "segment transcoding failed");
            HttpResponse::ServiceUnavailable()
                .body(format!("segment {seg_index} transcoding failed"))
        }
    }
}

// ── Demuxed (DASH-IF IOP v5) video/audio routes ──────────────────────────────

/// `GET /api/videos/{id}/video/{quality}/init.mp4` — video-only init segment.
///
/// Returns the ftyp+moov boxes for the video track at the requested quality.
/// The init is extracted from a freshly-generated (or cached) segment 0.
async fn get_video_init(
    params: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, quality_str) = params.into_inner();

    let quality = Quality::from_str(&quality_str.to_lowercase());

    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let seg_dir = state.cache_dir.join(&id).join("video").join(quality.as_str());
    if let Err(e) = tokio::fs::create_dir_all(&seg_dir).await {
        return HttpResponse::InternalServerError().body(format!("cache dir: {e}"));
    }

    let seg0_path = seg_dir.join("seg_00000.m4s");
    if seg0_path.exists() {
        let seg_dir_clone = seg_dir.clone();
        let init_data = match tokio::task::spawn_blocking(move || {
            let data = std::fs::read(&seg_dir_clone.join("seg_00000.m4s"))
                .map_err(|e| format!("read seg 0: {e}"))?;
            media::transcode::extract_ftyp_moov_pub(&data)
        }).await {
            Ok(Ok(data)) => data,
            Ok(Err(_e)) => {
                let _ = tokio::fs::remove_file(&seg0_path).await;
                return get_video_init_generate(abs_path, quality, &state, seg_dir).await;
            }
            Err(e) => return HttpResponse::InternalServerError()
                .body(format!("task panicked: {e}")),
        };
        return HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
            .body(init_data);
    }

    get_video_init_generate(abs_path, quality, &state, seg_dir).await
}

async fn get_video_init_generate(
    abs_path: std::path::PathBuf,
    quality: Quality,
    state: &web::Data<AppState>,
    seg_dir: std::path::PathBuf,
) -> HttpResponse {
    let resolved = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => return HttpResponse::InternalServerError().body(format!("path error: {e}")),
    };
    let abs_str = match resolved.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path not UTF-8"),
    };

    let hwaccel = state.hwaccel.read().clone();
    let seg_dir_clone = seg_dir.clone();
    let init_data = match tokio::task::spawn_blocking(move || {
        media::transcode::create_video_init_segment(&abs_str, quality, &hwaccel, &seg_dir_clone)
    }).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            error!(error = %e, "video init segment generation failed");
            return HttpResponse::ServiceUnavailable().body(format!("video init failed: {e}"));
        }
        Err(e) => return HttpResponse::InternalServerError().body(format!("task panicked: {e}")),
    };

    HttpResponse::Ok()
        .content_type("video/mp4")
        .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
        .body(init_data)
}

/// `GET /api/videos/{id}/audio/init.mp4` — audio-only init segment.
async fn get_audio_init(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let seg_dir = state.cache_dir.join(id.as_str()).join("audio");
    if let Err(e) = tokio::fs::create_dir_all(&seg_dir).await {
        return HttpResponse::InternalServerError().body(format!("cache dir: {e}"));
    }

    let seg0_path = seg_dir.join("seg_00000.m4s");
    if seg0_path.exists() {
        let seg_dir_clone = seg_dir.clone();
        let init_data = match tokio::task::spawn_blocking(move || {
            let data = std::fs::read(&seg_dir_clone.join("seg_00000.m4s"))
                .map_err(|e| format!("read audio seg 0: {e}"))?;
            media::transcode::extract_ftyp_moov_pub(&data)
        }).await {
            Ok(Ok(data)) => data,
            Ok(Err(_e)) => {
                let _ = tokio::fs::remove_file(&seg0_path).await;
                return get_audio_init_generate(abs_path, &state, seg_dir).await;
            }
            Err(e) => return HttpResponse::InternalServerError()
                .body(format!("task panicked: {e}")),
        };
        return HttpResponse::Ok()
            .content_type("audio/mp4")
            .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
            .body(init_data);
    }

    get_audio_init_generate(abs_path, &state, seg_dir).await
}

async fn get_audio_init_generate(
    abs_path: std::path::PathBuf,
    _state: &web::Data<AppState>,
    seg_dir: std::path::PathBuf,
) -> HttpResponse {
    let resolved = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => return HttpResponse::InternalServerError().body(format!("path error: {e}")),
    };
    let abs_str = match resolved.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path not UTF-8"),
    };

    let seg_dir_clone = seg_dir.clone();
    let init_data = match tokio::task::spawn_blocking(move || {
        media::transcode::create_audio_init_segment(&abs_str, &seg_dir_clone)
    }).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            error!(error = %e, "audio init segment generation failed");
            return HttpResponse::ServiceUnavailable().body(format!("audio init failed: {e}"));
        }
        Err(e) => return HttpResponse::InternalServerError().body(format!("task panicked: {e}")),
    };

    HttpResponse::Ok()
        .content_type("audio/mp4")
        .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
        .body(init_data)
}

/// `GET /api/videos/{id}/video/{quality}/{filename}` — video-only segment.
///
/// Serves cached video-only fMP4 segments or generates them on demand.
/// Strips ftyp+moov so the browser only receives moof+mdat fragments.
async fn get_video_segment(
    params: web::Path<(String, String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, quality_str, filename) = params.into_inner();

    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return HttpResponse::BadRequest().body("invalid filename");
    }
    if !filename.ends_with(".m4s") {
        return HttpResponse::BadRequest().body("invalid segment type");
    }

    let quality = Quality::from_str(&quality_str.to_lowercase());

    let seg_dir = state.cache_dir.join(&id).join("video").join(quality.as_str());

    // Parse seg_index from filename early so we can check the 0-based cache file.
    let url_seg_index: usize = match filename
        .strip_prefix("seg_")
        .and_then(|s| s.strip_suffix(".m4s"))
        .and_then(|s| s.parse().ok())
    {
        Some(idx) => idx,
        None => return HttpResponse::BadRequest().body("invalid segment filename"),
    };
    // MPD uses startNumber=1 but internal storage is 0-based.
    let seg_index = if url_seg_index > 0 { url_seg_index - 1 } else { 0 };
    let internal_seg_path = seg_dir.join(format!("seg_{:05}.m4s", seg_index));

    {
        let mut map = state.last_segment_access.write();
        map.insert(id.clone(), Instant::now());
    }
    state.playback_tx.send_if_modified(|v| {
        if *v { false } else { *v = true; true }
    });

    if let Ok(data) = tokio::fs::read(&internal_seg_path).await {
        let stripped = media::transcode::strip_init_boxes(&data);
        return HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
            .body(stripped);
    }

    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let resolved = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => return HttpResponse::InternalServerError()
            .body(format!("path error: {e}")),
    };
    let abs_str = match resolved.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path not UTF-8"),
    };

    if let Err(e) = tokio::fs::create_dir_all(&seg_dir).await {
        return HttpResponse::InternalServerError().body(format!("cache dir: {e}"));
    }

    // ── Inflight deduplication ────────────────────────────────────────────────
    // Only one transcode task per (id, seg_index, quality) at a time.
    // Concurrent requests wait on the watch channel; the owner does the work.
    let inflight_key = (id.clone(), seg_index, quality);
    let maybe_rx = {
        let mut map = state.video_segment_inflight.lock();
        if let Some(tx) = map.get(&inflight_key) {
            Some(tx.subscribe())
        } else {
            let (tx, _) = tokio::sync::watch::channel::<Option<Result<(), String>>>(None);
            map.insert(inflight_key.clone(), Arc::new(tx));
            None
        }
    };

    let transcode_result: Result<(), String> = if let Some(mut rx) = maybe_rx {
        loop {
            if rx.changed().await.is_err() {
                break Err(format!("video segment {seg_index} transcoding cancelled"));
            }
            if let Some(result) = rx.borrow().clone() {
                break result;
            }
        }
    } else {
        let _permit = match state.transcode_semaphore.acquire().await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "transcode semaphore closed");
                let tx = state.video_segment_inflight.lock().remove(&inflight_key);
                if let Some(tx) = tx { let _ = tx.send(Some(Err("semaphore closed".into()))); }
                return HttpResponse::InternalServerError().body("transcode unavailable");
            }
        };

        let result = media::transcode::transcode_video_segment(
            &abs_str, &seg_dir, seg_index, &state.hwaccel.read().clone(), quality, None,
        ).await;

        let tx = state.video_segment_inflight.lock().remove(&inflight_key);
        if let Some(tx) = tx { let _ = tx.send(Some(result.clone())); }
        result
    };

    // The file is stored with 0-based name; serve it regardless of the 1-based URL name.
    let internal_seg_path = seg_dir.join(format!("seg_{:05}.m4s", seg_index));

    match transcode_result {
        Ok(()) => {
            match tokio::fs::read(&internal_seg_path).await {
                Ok(data) => {
                    let stripped = media::transcode::strip_init_boxes(&data);
                    HttpResponse::Ok()
                        .content_type("video/mp4")
                        .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
                        .body(stripped)
                }
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("failed to read video segment: {e}")),
            }
        }
        Err(msg) => {
            error!(error = %msg, segment = seg_index, "video segment failed");
            HttpResponse::ServiceUnavailable()
                .body(format!("video segment {seg_index} failed"))
        }
    }
}

/// `GET /api/videos/{id}/audio/{filename}` — audio-only segment.
///
/// Serves cached audio-only fMP4 segments or generates them on demand.
async fn get_audio_segment(
    params: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, filename) = params.into_inner();

    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return HttpResponse::BadRequest().body("invalid filename");
    }
    if !filename.ends_with(".m4s") {
        return HttpResponse::BadRequest().body("invalid segment type");
    }

    let seg_dir = state.cache_dir.join(&id).join("audio");

    // Parse seg_index early so we can check the 0-based cache file.
    let url_seg_index: usize = match filename
        .strip_prefix("seg_")
        .and_then(|s| s.strip_suffix(".m4s"))
        .and_then(|s| s.parse().ok())
    {
        Some(idx) => idx,
        None => return HttpResponse::BadRequest().body("invalid segment filename"),
    };
    // MPD uses startNumber=1 but internal storage is 0-based.
    let seg_index = if url_seg_index > 0 { url_seg_index - 1 } else { 0 };
    let internal_seg_path = seg_dir.join(format!("seg_{:05}.m4s", seg_index));

    {
        let mut map = state.last_segment_access.write();
        map.insert(id.clone(), Instant::now());
    }
    state.playback_tx.send_if_modified(|v| {
        if *v { false } else { *v = true; true }
    });

    if let Ok(data) = tokio::fs::read(&internal_seg_path).await {
        let stripped = media::transcode::strip_init_boxes(&data);
        return HttpResponse::Ok()
            .content_type("audio/mp4")
            .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
            .body(stripped);
    }

    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let resolved = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => return HttpResponse::InternalServerError()
            .body(format!("path error: {e}")),
    };
    let abs_str = match resolved.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path not UTF-8"),
    };

    if let Err(e) = tokio::fs::create_dir_all(&seg_dir).await {
        return HttpResponse::InternalServerError().body(format!("cache dir: {e}"));
    }

    // ── Inflight deduplication ────────────────────────────────────────────────
    let inflight_key = (id.clone(), seg_index);
    let maybe_rx = {
        let mut map = state.audio_segment_inflight.lock();
        if let Some(tx) = map.get(&inflight_key) {
            Some(tx.subscribe())
        } else {
            let (tx, _) = tokio::sync::watch::channel::<Option<Result<(), String>>>(None);
            map.insert(inflight_key.clone(), Arc::new(tx));
            None
        }
    };

    let transcode_result: Result<(), String> = if let Some(mut rx) = maybe_rx {
        loop {
            if rx.changed().await.is_err() {
                break Err(format!("audio segment {seg_index} transcoding cancelled"));
            }
            if let Some(result) = rx.borrow().clone() {
                break result;
            }
        }
    } else {
        let _permit = match state.transcode_semaphore.acquire().await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "transcode semaphore closed");
                let tx = state.audio_segment_inflight.lock().remove(&inflight_key);
                if let Some(tx) = tx { let _ = tx.send(Some(Err("semaphore closed".into()))); }
                return HttpResponse::InternalServerError().body("transcode unavailable");
            }
        };

        let result = media::transcode::transcode_audio_segment(
            &abs_str, &seg_dir, seg_index, None,
        ).await;

        let tx = state.audio_segment_inflight.lock().remove(&inflight_key);
        if let Some(tx) = tx { let _ = tx.send(Some(result.clone())); }
        result
    };

    match transcode_result {
        Ok(()) => {
            match tokio::fs::read(&internal_seg_path).await {
                Ok(data) => {
                    let stripped = media::transcode::strip_init_boxes(&data);
                    HttpResponse::Ok()
                        .content_type("audio/mp4")
                        .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
                        .body(stripped)
                }
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("failed to read audio segment: {e}")),
            }
        }
        Err(msg) => {
            error!(error = %msg, segment = seg_index, "audio segment failed");
            HttpResponse::ServiceUnavailable()
                .body(format!("audio segment {seg_index} failed"))
        }
    }
}

// ── Cache management ─────────────────────────────────────────────────────────

/// `GET /api/debug/transcode` — transcode semaphore diagnostics.
///
/// Returns the number of currently available transcode permits so that
/// operators can observe concurrency headroom under load.
async fn get_transcode_debug(state: web::Data<AppState>) -> impl Responder {
    let available = state.transcode_semaphore.available_permits();
    HttpResponse::Ok().json(serde_json::json!({
        "available_permits": available,
    }))
}

/// `GET /api/hwaccel` — returns the detected hardware acceleration backend.
async fn get_hwaccel(state: web::Data<AppState>) -> impl Responder {
    let hw = state.hwaccel.read();
    HttpResponse::Ok().json(serde_json::json!({
        "label":   hw.label(),
        "encoder": hw.encoder(),
    }))
}

/// `DELETE /api/videos/{id}/cache` — clear cached segments for a video.
///
/// Removes non-pre-cached segments from all quality subdirectories of
/// `cache_dir/{id}/`.  The segments retained depend on the active
/// [`CacheStrategy`]: `balanced` keeps the first [`PRECACHE_SEGMENTS`]
/// segments and sparse seek anchors; `on-demand` removes everything;
/// `aggressive` keeps everything.  Called by the frontend when the user
/// navigates away from the player.
async fn clear_cache(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = id.into_inner();

    // Validate that the ID is a well-formed UUID to prevent path-traversal.
    if Uuid::parse_str(&id).is_err() {
        return HttpResponse::BadRequest().body("invalid video id");
    }

    let video_cache_dir = state.cache_dir.join(&id);

    remove_non_precached_segments_all_qualities(&video_cache_dir, state.cache_strategy).await;

    // Also cancel idle-eviction tracking so a stale entry doesn't
    // trigger a redundant removal on the next sweep.
    state
        .last_segment_access
        .write()
        .remove(&id);

    HttpResponse::NoContent().finish()
}

/// `GET /api/config` — return server configuration visible to the frontend.
///
/// Exposes the active caching strategy so the player can adjust its
/// behaviour (e.g. skip the cache-clear call when `aggressive` eviction is
/// disabled).
async fn get_config(state: web::Data<AppState>) -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({
        "cache_strategy": state.cache_strategy.as_str(),
    }))
}

// ── Thumbnail sprite generation ──────────────────────────────────────────────

/// Thumbnail sprite configuration
const THUMBNAIL_INTERVAL: f64 = media::sprite::THUMBNAIL_INTERVAL;
const THUMBNAIL_WIDTH: u32 = media::sprite::THUMBNAIL_WIDTH;
const THUMBNAIL_HEIGHT: u32 = media::sprite::THUMBNAIL_HEIGHT;
const THUMBNAILS_PER_ROW: u32 = media::sprite::THUMBNAILS_PER_ROW;

/// Response for thumbnail sprite info
#[derive(Clone, Serialize)]
struct ThumbnailInfo {
    url: String,
    sprite_width: u32,
    sprite_height: u32,
    thumb_width: u32,
    thumb_height: u32,
    columns: u32,
    rows: u32,
    interval: f64,
}

/// `GET /api/videos/{id}/thumbnails/info` — get thumbnail sprite info
async fn get_thumbnail_info(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    // Get video duration
    let (duration_secs, _) = probe_video(&abs_path).await;
    if duration_secs <= 0.0 {
        return HttpResponse::ServiceUnavailable().body("Could not determine video duration");
    }

    let duration = duration_secs;
    let num_thumbnails = ((duration / THUMBNAIL_INTERVAL).ceil() as u32).max(1);
    let columns = THUMBNAILS_PER_ROW.min(num_thumbnails);
    let rows = (num_thumbnails as f64 / columns as f64).ceil() as u32;

    let info = ThumbnailInfo {
        url: format!("/api/videos/{}/thumbnails/sprite.jpg", *id),
        sprite_width: columns * THUMBNAIL_WIDTH,
        sprite_height: rows * THUMBNAIL_HEIGHT,
        thumb_width: THUMBNAIL_WIDTH,
        thumb_height: THUMBNAIL_HEIGHT,
        columns,
        rows,
        interval: THUMBNAIL_INTERVAL,
    };

    HttpResponse::Ok().json(info)
}

/// `GET /api/videos/{id}/thumbnails/sprite-status` — check if sprite is cached
///
/// Returns `{"ready": true}` when the sprite sheet has already been generated
/// and is available in the cache.  Returns `{"ready": false}` otherwise.
/// This endpoint never triggers ffmpeg — it is a cheap filesystem check so
/// the frontend can decide whether to show a hover preview.
async fn get_sprite_status(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    // Validate that the ID is a well-formed UUID to prevent path-traversal.
    if Uuid::parse_str(&id).is_err() {
        return HttpResponse::BadRequest().body("invalid video id");
    }

    let sprite_path = state
        .cache_dir
        .join(format!("{}_thumbs", *id))
        .join("sprite.jpg");

    let ready = sprite_path.exists();
    HttpResponse::Ok().json(serde_json::json!({ "ready": ready }))
}

/// `GET /api/videos/{id}/processing-status` — processing status for a video.
///
/// Returns one of three states:
/// - `{"status":"processed"}` — all operations complete: thumbnail (`.jpg`),
///   sprite sheet (`_thumbs/sprite.jpg`), and segment pre-cache (first
///   [`PRECACHE_SEGMENTS`] `.m4s` files)
/// - `{"status":"processing"}` — a background worker is actively working on
///   this specific video right now
/// - `{"status":"pending"}`   — not fully processed and no worker is currently
///   working on this specific video
///
/// This is a cheap filesystem + lock-read check; it never triggers ffmpeg.
async fn get_processing_status(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if Uuid::parse_str(&id).is_err() {
        return HttpResponse::BadRequest().body("invalid video id");
    }

    let thumbnail_path = state.cache_dir.join(format!("{}.jpg", *id));
    let sprite_path = state
        .cache_dir
        .join(format!("{}_thumbs", *id))
        .join("sprite.jpg");

    // Check whether the pre-cached segments exist.  We only check for
    // seg_00000.m4s as a lightweight proxy — if the pre-cache worker
    // finished, all PRECACHE_SEGMENTS files will be present.
    // Segments are stored in quality-specific subdirectories under video/;
    // the precache worker always operates on the `original` quality level
    // (direct remux for compatible sources, fast transcode fallback).
    let precache_marker = state
        .cache_dir
        .join(id.as_str())
        .join("video")
        .join(Quality::Original.as_str())
        .join("seg_00000.m4s");

    let all_done = thumbnail_path.exists()
        && sprite_path.exists()
        && precache_marker.exists();

    let status = if all_done {
        "processed"
    } else {
        // "processing" only when THIS video is the one a worker is actively working on.
        let thumb_on_this = state
            .thumb_progress
            .read()
            .current_ids.contains(id.as_str());
        let sprite_on_this = state
            .sprite_progress
            .read()
            .current_ids.contains(id.as_str());
        let precache_on_this = state
            .precache_progress
            .read()
            .current_id.as_deref() == Some(id.as_str());

        if thumb_on_this || sprite_on_this || precache_on_this {
            "processing"
        } else {
            "pending"
        }
    };

    HttpResponse::Ok().json(serde_json::json!({ "status": status }))
}

/// `GET /api/videos/{id}/cache-status` — whether a video is fully cached (aggressive mode).
///
/// Only meaningful when `cache_strategy` is `aggressive`.  Returns:
/// - `{"fully_cached": true}`  — the `.fully_cached` marker file exists, meaning the
///   pre-cache worker has finished transcoding every segment at every quality level
/// - `{"fully_cached": false}` — not yet fully cached (or strategy is not aggressive)
///
/// This is a cheap filesystem check; it never triggers ffmpeg.
async fn get_cache_status(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if Uuid::parse_str(&id).is_err() {
        return HttpResponse::BadRequest().body("invalid video id");
    }

    if state.cache_strategy != CacheStrategy::Aggressive {
        return HttpResponse::Ok().json(serde_json::json!({ "fully_cached": false }));
    }

    let marker = state.cache_dir.join(id.as_str()).join(".fully_cached");
    let fully_cached = marker.exists();
    HttpResponse::Ok().json(serde_json::json!({ "fully_cached": fully_cached }))
}

/// `GET /api/videos/{id}/thumbnails/sprite.jpg` — get thumbnail sprite image
async fn get_thumbnail_sprite(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let sprite_dir = state.cache_dir.join(format!("{}_thumbs", *id));
    let sprite_path = sprite_dir.join("sprite.jpg");

    // Check if sprite already exists
    if let Ok(data) = tokio::fs::read(&sprite_path).await {
        return HttpResponse::Ok()
            .content_type("image/jpeg")
            .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
            .body(data);
    }

    // Refuse to start generation while any video is being streamed.
    // The background worker will generate this sprite once playback ends.
    if *state.playback_tx.borrow() {
        return HttpResponse::ServiceUnavailable()
            .body("sprite generation paused during playback");
    }

    // Generate the sprite using the shared helper (creates dir, runs ffmpeg).
    // For on-demand requests, we never cancel — pass an inert kill flag.
    let no_kill = Arc::new(AtomicBool::new(false));
    if generate_sprite(&id, &abs_path, &state.cache_dir, no_kill).await {
        match tokio::fs::read(&sprite_path).await {
            Ok(data) => HttpResponse::Ok()
                .content_type("image/jpeg")
                .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
                .body(data),
            Err(e) => HttpResponse::InternalServerError()
                .body(format!("failed to read sprite: {e}")),
        }
    } else {
        HttpResponse::ServiceUnavailable().body("sprite generation failed or unavailable")
    }
}

/// Generates the thumbnail sprite sheet for a video using in-process
/// keyframe-only decoding via `ffmpeg-next`.
///
/// Creates `{cache_dir}/{id}_thumbs/sprite.jpg`.  Returns `true` on success.
///
/// When `kill` is set to `true`, the in-progress keyframe decoding bails out
/// early so that background work yields I/O and CPU to playback.
async fn generate_sprite(
    id: &str,
    abs_path: &Path,
    cache_dir: &Path,
    kill: Arc<AtomicBool>,
) -> bool {
    let sprite_dir = cache_dir.join(format!("{}_thumbs", id));
    let sprite_path = sprite_dir.join("sprite.jpg");

    if sprite_path.exists() {
        return true;
    }

    if tokio::fs::create_dir_all(&sprite_dir).await.is_err() {
        return false;
    }

    let (duration_secs, _) = probe_video(abs_path).await;
    if duration_secs <= 0.0 {
        return false;
    }

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };

    let sprite_dir_owned = sprite_dir.clone();
    let duration_secs_u32 = duration_secs as u32;
    tokio::task::spawn_blocking(move || {
        media::sprite::generate_sprite_sheet(&resolved_path, duration_secs_u32, &sprite_dir_owned, &kill)
    })
    .await
    .unwrap_or(false)
}

/// Background worker that proactively generates sprite sheets for every video.
///
/// Mirrors `run_thumb_worker`: waits for a notification, walks the library,
/// skips videos whose `{id}_thumbs/sprite.jpg` already exists, generates the
/// rest, and updates progress counters as it goes.
async fn run_sprite_worker(
    library_path: PathBuf,
    cache_dir: PathBuf,
    progress: Arc<RwLock<SpriteProgress>>,
    trigger: Arc<tokio::sync::Notify>,
    mut playback_rx: tokio::sync::watch::Receiver<bool>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    // Sprite generation uses multi-threaded keyframe-only decoding internally,
    // so running one video at a time maximises throughput without CPU contention.
    let concurrency = 1_usize;
    loop {
        // Fast exit if shutdown was already signaled before we block.
        if *shutdown_rx.borrow() {
            return;
        }
        tokio::select! {
            _ = trigger.notified() => {}
            _ = shutdown_rx.changed() => { return; }
        }
        if *shutdown_rx.borrow() {
            return;
        }

        let (sprite_done, entries): (Vec<_>, Vec<_>) = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .partition(|e| {
                let abs = e.path();
                let rel = abs
                    .strip_prefix(&library_path)
                    .unwrap_or(abs)
                    .to_string_lossy();
                let id = video_id(&rel);
                cache_dir
                    .join(format!("{}_thumbs", id))
                    .join("sprite.jpg")
                    .exists()
            });

        {
            let mut p = progress.write();
            p.current = sprite_done.len() as u32;
            p.total = (sprite_done.len() + entries.len()) as u32;
            p.active = !entries.is_empty();
        }

        let kill = Arc::new(AtomicBool::new(false));
        let mut join_set: tokio::task::JoinSet<(String, bool)> = tokio::task::JoinSet::new();
        let mut iter = entries.into_iter().peekable();
        loop {
            // Suspend while a video is being streamed: signal in-flight
            // tasks to bail out, drain them, then wait for playback to end.
            if *playback_rx.borrow() {
                kill.store(true, Ordering::SeqCst);
                while join_set.join_next().await.is_some() {}
                while *playback_rx.borrow() {
                    let _ = playback_rx.changed().await;
                }
                kill.store(false, Ordering::SeqCst);
            }
            if *shutdown_rx.borrow() {
                return;
            }
            // Fill empty slots up to the concurrency limit.
            while join_set.len() < concurrency && iter.peek().is_some() {
                if *shutdown_rx.borrow() {
                    return;
                }
                let entry = iter.next().unwrap();
                let abs = entry.path().to_path_buf();
                let rel = abs
                    .strip_prefix(&library_path)
                    .unwrap_or(&abs)
                    .to_string_lossy()
                    .to_string();
                let id = video_id(&rel);
                {
                    let mut p = progress.write();
                    p.current_ids.insert(id.clone());
                }
                let cache_dir = cache_dir.clone();
                let k = Arc::clone(&kill);
                join_set.spawn(async move {
                    let ok = generate_sprite(&id, &abs, &cache_dir, k).await;
                    (id, ok)
                });
            }
            if join_set.is_empty() {
                break;
            }
            // Collect the next completed task.  React immediately to
            // playback or shutdown so in-flight work is cancelled promptly.
            let next = tokio::select! {
                r = join_set.join_next() => r,
                _ = playback_rx.changed() => { continue; }
                _ = shutdown_rx.changed() => { return; }
            };
            if let Some(Ok((id, _ok))) = next {
                let mut p = progress.write();
                p.current_ids.remove(&id);
                p.current += 1;
                if p.current >= p.total {
                    p.active = false;
                }
            }
        }
    }
}

// ── Segment pre-caching ──────────────────────────────────────────────────────

/// Background worker that proactively transcodes the first few minutes of
/// every video so that playback can begin instantly.
///
/// Mirrors `run_thumb_worker` / `run_sprite_worker`: waits for a notification
/// on `trigger`, walks the library, skips videos whose segments are already
/// cached, and transcodes the missing ones.  Behaviour depends on
/// [`CacheStrategy`]:
///
/// - `OnDemand` — skips all work; the worker exits each pass immediately.
/// - `Balanced` — pre-caches the first [`PRECACHE_SEGMENTS`] segments for
///   instant start, and every [`SPARSE_CACHE_STRIDE`]-th segment as a seek
///   anchor (current default).
/// - `Aggressive` — pre-transcodes every segment at every quality level
///   applicable to the video's native resolution (no seek latency at any
///   position or quality).
///
/// Suspends while playback is active (checking between every individual
/// segment) and resumes automatically once idle.  Progress counters are
/// written to `progress` so the WS can drive a frontend progress bar.
async fn run_precache_worker(
    library_path: PathBuf,
    cache_dir: PathBuf,
    hwaccel: Arc<RwLock<HwAccel>>,
    progress: Arc<RwLock<PrecacheProgress>>,
    trigger: Arc<tokio::sync::Notify>,
    mut playback_rx: tokio::sync::watch::Receiver<bool>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    strategy: CacheStrategy,
) {
    loop {
        // Fast exit if shutdown was already signaled before we block.
        if *shutdown_rx.borrow() {
            return;
        }
        tokio::select! {
            _ = trigger.notified() => {}
            _ = shutdown_rx.changed() => { return; }
        }
        if *shutdown_rx.borrow() {
            return;
        }

        // On-demand mode: no pre-caching at all — skip this pass.
        if strategy == CacheStrategy::OnDemand {
            continue;
        }

        let entries: Vec<_> = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .collect();

        // Partition into already-cached and needs-work.
        let mut done_count: usize = 0;
        let mut pending: Vec<_> = Vec::new();
        for e in entries {
            let abs = e.path();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(abs)
                .to_string_lossy();
            let id = video_id(&rel);
            let video_dir = cache_dir.join(&id).join("video").join(Quality::Original.as_str());

            let is_done = match strategy {
                // OnDemand: never reached — the early `continue` above skips
                // the entire partition loop body for this strategy.
                CacheStrategy::OnDemand => false,
                CacheStrategy::Balanced => {
                    if !video_dir.join("seg_00000.m4s").exists() {
                        false
                    } else {
                        // First segment exists; check whether the last expected
                        // sparse anchor is also present.
                        let (dur_secs, _) = probe_video(abs).await;
                        if dur_secs <= 0.0 {
                            true
                        } else {
                            let total_segs = (dur_secs / SEGMENT_DURATION).ceil() as usize;
                            if total_segs > PRECACHE_SEGMENTS {
                                let last_anchor =
                                    ((total_segs - 1) / SPARSE_CACHE_STRIDE) * SPARSE_CACHE_STRIDE;
                                if last_anchor >= PRECACHE_SEGMENTS {
                                    video_dir.join(format!("seg_{:05}.m4s", last_anchor)).exists()
                                } else {
                                    true
                                }
                            } else {
                                true
                            }
                        }
                    }
                }
                CacheStrategy::Aggressive => {
                    // A video is fully cached when the worker has written the
                    // `.fully_cached` marker at the end of a successful pass.
                    cache_dir.join(&id).join(".fully_cached").exists()
                }
            };

            if is_done {
                done_count += 1;
            } else {
                pending.push(e);
            }
        }

        {
            let mut p = progress.write();
            p.current = done_count as u32;
            p.total = (done_count + pending.len()) as u32;
            p.active = !pending.is_empty();
        }

        for entry in pending {
            if *shutdown_rx.borrow() {
                return;
            }
            // Suspend while a video is being streamed; resume once idle.
            while *playback_rx.borrow() {
                let _ = playback_rx.changed().await;
            }
            if *shutdown_rx.borrow() {
                return;
            }

            let abs = entry.path().to_path_buf();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            let id = video_id(&rel);

            {
                let mut p = progress.write();
                p.current_id = Some(id.clone());
            }

            // Determine how many segments to pre-cache (capped by video duration).
            let (duration_secs, _) = probe_video(&abs).await;
            if duration_secs <= 0.0 {
                progress.write().advance();
                continue;
            }
            let total_segments = (duration_secs / SEGMENT_DURATION).ceil() as usize;

            // Resolve the source path once for all segments of this video.
            let resolved_path = match abs.canonicalize() {
                Ok(p) => p,
                Err(_) => {
                    progress.write().advance();
                    continue;
                }
            };
            let abs_str = match resolved_path.to_str() {
                Some(s) => s.to_owned(),
                None => {
                    progress.write().advance();
                    continue;
                }
            };

            match strategy {
                // OnDemand: never reached — the early `continue` above skips
                // the entire pending loop body for this strategy.
                CacheStrategy::OnDemand => {}

                CacheStrategy::Balanced => {
                    let video_dir = cache_dir.join(&id).join("video").join(Quality::Original.as_str());
                    let audio_dir = cache_dir.join(&id).join("audio");

                    // Dense: every segment in the initial pre-cache window for instant playback start.
                    // Sparse: every SPARSE_CACHE_STRIDE-th segment beyond that window as seek anchors.
                    let segments_to_cache: Vec<usize> = (0..total_segments)
                        .filter(|&i| i < PRECACHE_SEGMENTS || i % SPARSE_CACHE_STRIDE == 0)
                        .collect();

                    // Collect only the segments that are missing (check video dir only;
                    // audio will be generated alongside video).
                    let missing: Vec<usize> = segments_to_cache.iter()
                        .copied()
                        .filter(|i| !video_dir.join(format!("seg_{:05}.m4s", i)).exists())
                        .collect();
                    if missing.is_empty() {
                        progress.write().advance();
                        continue;
                    }

                    if let Err(e) = tokio::fs::create_dir_all(&video_dir).await {
                        error!(video_id = %id, error = %e, "precache: video cache dir error");
                        progress.write().advance();
                        continue;
                    }
                    if let Err(e) = tokio::fs::create_dir_all(&audio_dir).await {
                        error!(video_id = %id, error = %e, "precache: audio cache dir error");
                        progress.write().advance();
                        continue;
                    }

                    info!(
                        video_id = %id,
                        missing_segments = missing.len(),
                        total_segments = segments_to_cache.len(),
                        "pre-caching segments (balanced)"
                    );

                    let kill = Arc::new(AtomicBool::new(false));
                    'seg_loop: for i in missing {
                        if *shutdown_rx.borrow() { return; }
                        if *playback_rx.borrow() {
                            kill.store(true, Ordering::SeqCst);
                            while *playback_rx.borrow() {
                                let _ = playback_rx.changed().await;
                            }
                            kill.store(false, Ordering::SeqCst);
                        }
                        if *shutdown_rx.borrow() { return; }

                        let hw = hwaccel.read().clone();
                        let video_result = tokio::select! {
                            r = media::transcode::transcode_video_segment_with_kill(&abs_str, &video_dir, i, &hw, Quality::Original, Arc::clone(&kill)) => r,
                            _ = playback_rx.changed() => {
                                if *playback_rx.borrow() { kill.store(true, Ordering::SeqCst); }
                                continue 'seg_loop;
                            }
                            _ = shutdown_rx.changed() => { return; }
                        };
                        if let Err(e) = video_result {
                            if e == media::transcode::CANCELLED { continue 'seg_loop; }
                            error!(video_id = %id, segment = i, error = %e, "precache: video segment transcode failed");
                            break 'seg_loop;
                        }
                        let audio_result = tokio::select! {
                            r = media::transcode::transcode_audio_segment_with_kill(&abs_str, &audio_dir, i, Arc::clone(&kill)) => r,
                            _ = playback_rx.changed() => {
                                if *playback_rx.borrow() { kill.store(true, Ordering::SeqCst); }
                                continue 'seg_loop;
                            }
                            _ = shutdown_rx.changed() => { return; }
                        };
                        if let Err(e) = audio_result {
                            if e == media::transcode::CANCELLED { continue 'seg_loop; }
                            error!(video_id = %id, segment = i, error = %e, "precache: audio segment transcode failed");
                        }
                    }
                }

                CacheStrategy::Aggressive => {
                    // Probe native resolution to determine applicable quality tiers.
                    let abs_for_probe = abs.clone();
                    let source_height = tokio::task::spawn_blocking(move || {
                        media::probe::probe_stream_info(&abs_for_probe).height
                    }).await.unwrap_or(0);

                    let res_qualities: &[(Quality, u32)] = &[
                        (Quality::Q2160, 2160),
                        (Quality::Q1080, 1080),
                        (Quality::Q720,  720),
                        (Quality::Q480,  480),
                        (Quality::Q360,  360),
                    ];
                    // Always include Original (direct remux); add all applicable
                    // transcoded tiers based on native resolution.
                    let mut applicable: Vec<Quality> = vec![Quality::Original];
                    for &(q, target_h) in res_qualities {
                        if source_height >= target_h {
                            applicable.push(q);
                        }
                    }

                    let audio_dir = cache_dir.join(&id).join("audio");

                    // Transcode audio once (quality-independent).
                    let audio_missing: Vec<usize> = (0..total_segments)
                        .filter(|i| !audio_dir.join(format!("seg_{:05}.m4s", i)).exists())
                        .collect();

                    if let Err(e) = tokio::fs::create_dir_all(&audio_dir).await {
                        error!(video_id = %id, error = %e, "precache: audio cache dir error");
                        progress.write().advance();
                        continue;
                    }

                    let kill = Arc::new(AtomicBool::new(false));
                    let mut all_complete = true;

                    // Cache video segments for each quality tier.
                    'qual_loop: for quality in applicable {
                        let q_video_dir = cache_dir.join(&id).join("video").join(quality.as_str());
                        if let Err(e) = tokio::fs::create_dir_all(&q_video_dir).await {
                            error!(video_id = %id, quality = %quality.as_str(), error = %e, "precache: video cache dir error");
                            continue 'qual_loop;
                        }

                        let missing: Vec<usize> = (0..total_segments)
                            .filter(|i| !q_video_dir.join(format!("seg_{:05}.m4s", i)).exists())
                            .collect();
                        if missing.is_empty() {
                            continue 'qual_loop;
                        }

                        info!(
                            video_id = %id,
                            quality = %quality.as_str(),
                            missing_segments = missing.len(),
                            total_segments,
                            "pre-caching segments (aggressive)"
                        );

                        'seg_loop: for i in missing {
                            if *shutdown_rx.borrow() { return; }
                            if *playback_rx.borrow() {
                                kill.store(true, Ordering::SeqCst);
                                while *playback_rx.borrow() {
                                    let _ = playback_rx.changed().await;
                                }
                                kill.store(false, Ordering::SeqCst);
                            }
                            if *shutdown_rx.borrow() { return; }

                            let hw = hwaccel.read().clone();
                            let video_result = tokio::select! {
                                r = media::transcode::transcode_video_segment_with_kill(&abs_str, &q_video_dir, i, &hw, quality, Arc::clone(&kill)) => r,
                                _ = playback_rx.changed() => {
                                    if *playback_rx.borrow() { kill.store(true, Ordering::SeqCst); }
                                    all_complete = false;
                                    continue 'seg_loop;
                                }
                                _ = shutdown_rx.changed() => { return; }
                            };
                            if let Err(e) = video_result {
                                if e == media::transcode::CANCELLED { all_complete = false; continue 'seg_loop; }
                                error!(video_id = %id, quality = %quality.as_str(), segment = i, error = %e, "precache: video segment transcode failed");
                                all_complete = false;
                                break 'seg_loop;
                            }
                        }
                    }

                    // Cache audio segments.
                    'audio_loop: for i in audio_missing {
                        if *shutdown_rx.borrow() { return; }
                        if *playback_rx.borrow() {
                            kill.store(true, Ordering::SeqCst);
                            while *playback_rx.borrow() {
                                let _ = playback_rx.changed().await;
                            }
                            kill.store(false, Ordering::SeqCst);
                        }
                        if *shutdown_rx.borrow() { return; }

                        let audio_result = tokio::select! {
                            r = media::transcode::transcode_audio_segment_with_kill(&abs_str, &audio_dir, i, Arc::clone(&kill)) => r,
                            _ = playback_rx.changed() => {
                                if *playback_rx.borrow() { kill.store(true, Ordering::SeqCst); }
                                all_complete = false;
                                continue 'audio_loop;
                            }
                            _ = shutdown_rx.changed() => { return; }
                        };
                        if let Err(e) = audio_result {
                            if e == media::transcode::CANCELLED { all_complete = false; continue 'audio_loop; }
                            error!(video_id = %id, segment = i, error = %e, "precache: audio segment transcode failed");
                            all_complete = false;
                            break 'audio_loop;
                        }
                    }

                    // Unconditionally write `.fully_cached` once all transcode loops
                    // finish without errors or interruptions.  This avoids a
                    // float-rounding off-by-one where `ffmpeg` generates one fewer
                    // segment than `ceil(duration / SEGMENT_DURATION)` predicts.
                    if all_complete {
                        let marker_path = cache_dir.join(&id).join(".fully_cached");
                        if let Err(e) = tokio::fs::write(&marker_path, b"").await {
                            warn!(video_id = %id, error = %e, "precache: could not write .fully_cached marker");
                        }
                    }
                }
            }

            progress.write().advance();
        }
    }
}

// ── Subtitle extraction ──────────────────────────────────────────────────────

/// Response for subtitle tracks info
#[derive(Clone, Serialize)]
struct SubtitleTrack {
    index: u32,
    language: Option<String>,
    title: Option<String>,
    codec: String,
}

#[derive(Clone, Serialize)]
struct SubtitleTracksResponse {
    tracks: Vec<SubtitleTrack>,
}

/// `GET /api/videos/{id}/subtitles` — list available subtitle tracks
///
/// Uses in-process ffmpeg-next probing to enumerate subtitle streams.
async fn list_subtitles(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };

    let streams = tokio::task::spawn_blocking(move || {
        media::probe::list_subtitle_streams(&resolved_path)
    })
    .await
    .unwrap_or_default();

    let tracks: Vec<SubtitleTrack> = streams
        .into_iter()
        .map(|s| SubtitleTrack {
            index: s.index,
            language: s.language,
            title: s.title,
            codec: s.codec_name,
        })
        .collect();

    HttpResponse::Ok().json(SubtitleTracksResponse { tracks })
}

/// `GET /api/videos/{id}/subtitles/{index}.vtt` — get subtitle track as WebVTT
///
/// Uses the media::subtitle module for extraction.
async fn get_subtitle(
    params: web::Path<(String, u32)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, track_index) = params.into_inner();

    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let sub_dir = state.cache_dir.join(format!("{}_subs", id));
    let vtt_path = sub_dir.join(format!("{}.vtt", track_index));

    // Check if subtitle already exists
    if let Ok(data) = tokio::fs::read_to_string(&vtt_path).await {
        return HttpResponse::Ok()
            .content_type("text/vtt")
            .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
            .body(data);
    }

    // Create cache directory
    if let Err(e) = tokio::fs::create_dir_all(&sub_dir).await {
        return HttpResponse::InternalServerError().body(format!("cache dir error: {e}"));
    }

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };

    match media::subtitle::extract_subtitle_to_vtt(&resolved_path, track_index, &vtt_path).await {
        Ok(()) => {
            match tokio::fs::read_to_string(&vtt_path).await {
                Ok(data) => HttpResponse::Ok()
                    .content_type("text/vtt")
                    .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
                    .body(data),
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("failed to read subtitle: {e}")),
            }
        }
        Err(e) => {
            error!(error = %e, "subtitle extraction failed");
            HttpResponse::ServiceUnavailable().body(format!("subtitle extraction failed: {e}"))
        }
    }
}

// ── Password protection ──────────────────────────────────────────────────────

/// Hash a password with Argon2id (salted, memory-hard).
fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("hashing error: {e}"))
}

/// Verify a password against a stored Argon2 hash.
fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match argon2::PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Generate a random session token.
fn generate_token() -> String {
    format!("{}{}", Uuid::new_v4().as_simple(), Uuid::new_v4().as_simple())
}

/// Extract the session token from the `starfin_token` cookie.
fn extract_token(req: &HttpRequest) -> Option<String> {
    req.cookie("starfin_token").map(|c| c.value().to_string())
}

/// Check whether the request carries a valid session token.
fn is_authenticated(req: &HttpRequest, state: &AppState) -> bool {
    if !state.password_protection {
        return true;
    }
    if let Some(token) = extract_token(req) {
        let tokens = state.auth_tokens.read();
        tokens.contains(&token)
    } else {
        false
    }
}

/// `GET /api/auth/status` — returns whether password protection is enabled,
/// whether a password has been set, and whether the current request is
/// authenticated.
#[derive(Serialize)]
struct AuthStatusResponse {
    password_protection: bool,
    password_set: bool,
    authenticated: bool,
}

async fn auth_status(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let password_set = state.password_hash_path.exists();
    let authenticated = is_authenticated(&req, &state);
    HttpResponse::Ok().json(AuthStatusResponse {
        password_protection: state.password_protection,
        password_set,
        authenticated,
    })
}

/// `POST /api/auth/set-password` — set the initial password (only allowed when
/// no password has been set yet).
#[derive(Deserialize)]
struct SetPasswordRequest {
    password: String,
    confirm: String,
}

async fn set_password(
    body: web::Json<SetPasswordRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if !state.password_protection {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Password protection is not enabled"
        }));
    }
    if state.password_hash_path.exists() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Password is already set"
        }));
    }
    if body.password.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Password cannot be empty"
        }));
    }
    if body.password != body.confirm {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Passwords do not match"
        }));
    }

    let hashed = match hash_password(&body.password) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "password hashing failed");
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to hash password"
            }));
        }
    };
    if let Err(e) = std::fs::write(&state.password_hash_path, &hashed) {
        error!(error = %e, "failed to write password hash");
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "Failed to save password"
        }));
    }

    // Auto-login after setting password.
    let token = generate_token();
    {
        let mut tokens = state.auth_tokens.write();
        tokens.insert(token.clone());
    }

    HttpResponse::Ok()
        .cookie(
            actix_web::cookie::Cookie::build("starfin_token", &token)
                .path("/")
                .http_only(true)
                .same_site(actix_web::cookie::SameSite::Lax)
                .finish(),
        )
        .json(serde_json::json!({ "ok": true }))
}

/// `POST /api/auth/login` — authenticate with the password.
#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

async fn login(
    body: web::Json<LoginRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if !state.password_protection {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Password protection is not enabled"
        }));
    }

    let stored_hash = match std::fs::read_to_string(&state.password_hash_path) {
        Ok(h) => h.trim().to_string(),
        Err(_) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "No password has been set"
            }));
        }
    };

    if !verify_password(&body.password, &stored_hash) {
        return HttpResponse::Unauthorized().json(serde_json::json!({
            "error": "Incorrect password"
        }));
    }

    let token = generate_token();
    {
        let mut tokens = state.auth_tokens.write();
        tokens.insert(token.clone());
    }

    HttpResponse::Ok()
        .cookie(
            actix_web::cookie::Cookie::build("starfin_token", &token)
                .path("/")
                .http_only(true)
                .same_site(actix_web::cookie::SameSite::Lax)
                .finish(),
        )
        .json(serde_json::json!({ "ok": true }))
}

/// `POST /api/auth/logout` — invalidate the current session.
async fn logout(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Some(token) = extract_token(&req) {
        let mut tokens = state.auth_tokens.write();
        tokens.remove(&token);
    }

    // Clear the cookie by setting it with an empty value and max-age 0.
    HttpResponse::Ok()
        .cookie(
            actix_web::cookie::Cookie::build("starfin_token", "")
                .path("/")
                .http_only(true)
                .same_site(actix_web::cookie::SameSite::Lax)
                .max_age(actix_web::cookie::time::Duration::ZERO)
                .finish(),
        )
        .json(serde_json::json!({ "ok": true }))
}

/// Middleware: returns `401 Unauthorized` for unauthenticated requests to
/// protected API routes when password protection is enabled.
async fn auth_middleware(
    req: ServiceRequest,
    next: Next<impl MessageBody + 'static>,
) -> Result<ServiceResponse<impl MessageBody + 'static>, Error> {
    let path = req.path().to_string();

    // Auth endpoints, theme CSS, and static frontend assets are always accessible.
    let is_exempt = path.starts_with("/api/auth/")
        || path == "/api/health"
        || path == "/api/theme.css"
        || !path.starts_with("/api/");

    if is_exempt {
        return next.call(req).await.map(|res| res.map_into_left_body());
    }

    let state = req
        .app_data::<web::Data<AppState>>()
        .expect("AppState not configured");

    if !state.password_protection {
        return next.call(req).await.map(|res| res.map_into_left_body());
    }

    // Check for a valid session token in the cookie.
    let authenticated = req
        .cookie("starfin_token")
        .map(|c| {
            let tokens = state.auth_tokens.read();
            tokens.contains(c.value())
        })
        .unwrap_or(false);

    if authenticated {
        return next.call(req).await.map(|res| res.map_into_left_body());
    }

    let response = HttpResponse::Unauthorized()
        .json(serde_json::json!({ "error": "Authentication required" }));
    Ok(req.into_response(response).map_into_right_body())
}

// ── Static asset serving ─────────────────────────────────────────────────────

fn content_type(path: &str) -> header::HeaderValue {
    let mime = MimeGuess::from_path(path).first_or_octet_stream();
    header::HeaderValue::from_str(mime.as_ref()).unwrap()
}

async fn frontend(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> actix_web::Result<HttpResponse> {
    let tail = req.match_info().query("tail");
    let mut path = tail.trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }

    if let Some(file) = Assets::get(path) {
        if path == "index.html" {
            // Inject the theme stylesheet link before </head> so it loads
            // after main.css and overrides the default Jetson variables.
            let html = String::from_utf8_lossy(&file.data);
            let themed = if !state.theme_css.is_empty() {
                html.replacen(
                    "</head>",
                    "<link rel=\"stylesheet\" href=\"/api/theme.css\" />\n</head>",
                    1,
                )
            } else {
                html.into_owned()
            };
            return Ok(HttpResponse::Ok()
                .insert_header((header::CONTENT_TYPE, content_type(path)))
                .insert_header((header::CACHE_CONTROL, "no-cache"))
                .body(themed));
        }
        return Ok(HttpResponse::Ok()
            .insert_header((header::CONTENT_TYPE, content_type(path)))
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(file.data.into_owned()));
    }

    if let Some(index) = Assets::get("index.html") {
        let html = String::from_utf8_lossy(&index.data);
        let themed = if !state.theme_css.is_empty() {
            html.replacen(
                "</head>",
                "<link rel=\"stylesheet\" href=\"/api/theme.css\" />\n</head>",
                1,
            )
        } else {
            html.into_owned()
        };
        return Ok(HttpResponse::Ok()
            .insert_header((
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/html; charset=utf-8"),
            ))
            .insert_header((header::CACHE_CONTROL, "no-cache"))
            .body(themed));
    }

    Err(actix_web::error::ErrorNotFound("asset not found"))
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8089);

    let library_path = PathBuf::from(
        std::env::var("VIDEO_LIBRARY_PATH").unwrap_or_else(|_| "./test_videos".into()),
    );

    let cache_dir = PathBuf::from(
        std::env::var("CACHE_DIR").unwrap_or_else(|_| "./starfin_cache".into()),
    );

    if !library_path.exists() {
        std::fs::create_dir_all(&library_path)?;
    }
    std::fs::create_dir_all(&cache_dir)?;

    // ── Cache strategy ───────────────────────────────────────────────────
    // Resolved here (early) so it can gate the tmp-file cleanup below.
    let cache_strategy = std::env::var("CACHE_STRATEGY")
        .map(|v| CacheStrategy::from_str(&v))
        .unwrap_or(CacheStrategy::Balanced);
    info!(strategy = %cache_strategy.as_str(), "cache strategy (set CACHE_STRATEGY to override)");

    // Remove any *.tmp files left behind by a previous shutdown.
    // These are always incomplete and can never be reused.  Run in the
    // background so the HTTP server is not delayed by the cache-tree walk.
    //
    // Skipped in Aggressive mode: every media file is fully cached on disk
    // and no eviction occurs, so orphaned tmp files are harmless disk
    // artefacts rather than a correctness concern.
    if cache_strategy != CacheStrategy::Aggressive {
        let cleanup_cache_dir = cache_dir.clone();
        tokio::spawn(async move {
            info!("Cleaning any orphaned temp files.");
            tokio::task::spawn_blocking(move || {
                cleanup_orphaned_tmp_files(&cleanup_cache_dir);
            })
            .await
            .ok();
        });
    }

    // ── Startup healthchecks (logged for journalctl) ─────────────────────
    // Run in the background so the HTTP server starts immediately.
    {
        let hc_library = library_path.clone();
        let hc_cache = cache_dir.clone();
        tokio::spawn(async move {
            run_startup_healthchecks(&hc_library, &hc_cache).await;
        });
    }

    // Hardware acceleration detection — start with Software fallback so the
    // server is usable immediately, then upgrade once the GPU probe completes.
    let hwaccel: Arc<RwLock<HwAccel>> = Arc::new(RwLock::new(HwAccel::Software));
    {
        let hwaccel_bg = Arc::clone(&hwaccel);
        tokio::spawn(async move {
            let detected = media::hwaccel::detect_hwaccel().await;
            *hwaccel_bg.write() = detected;
        });
    }

    // Load any previously-persisted video index so the server starts with
    // known media immediately.
    let initial_items = load_video_cache(&cache_dir);
    if !initial_items.is_empty() {
        info!(count = initial_items.len(), "loaded videos from persisted index");
    }
    let video_cache: Arc<RwLock<Vec<VideoItem>>> = Arc::new(RwLock::new(initial_items));

    // Path index starts empty; a background task populates it via a fast,
    // probe-free library walk so the server starts without waiting for the
    // full directory traversal (which can be slow for large libraries).
    // The startup scan (below) also rebuilds the full index, so even if
    // this walk hasn't finished by the time a request arrives, the scan
    // will fill it shortly after.
    let video_path_index: Arc<RwLock<VideoPathIndex>> =
        Arc::new(RwLock::new(HashMap::new()));
    {
        let idx_library = library_path.clone();
        let idx_path_index = Arc::clone(&video_path_index);
        tokio::spawn(async move {
            let index = match tokio::task::spawn_blocking(move || {
                build_video_index(&idx_library)
            }).await {
                Ok(idx) => idx,
                Err(e) => {
                    warn!(error = %e, "background index walk panicked");
                    HashMap::new()
                }
            };
            *idx_path_index.write() = index;
        });
    }

    let thumb_progress = Arc::new(RwLock::new(ThumbProgress {
        current: 0,
        total: 0,
        active: false,
        current_ids: HashSet::new(),
    }));
    let thumb_trigger = Arc::new(tokio::sync::Notify::new());

    let sprite_progress = Arc::new(RwLock::new(SpriteProgress {
        current: 0,
        total: 0,
        active: false,
        current_ids: HashSet::new(),
    }));
    let sprite_trigger = Arc::new(tokio::sync::Notify::new());
    let precache_trigger = Arc::new(tokio::sync::Notify::new());
    let precache_hwaccel = Arc::clone(&hwaccel);
    let precache_progress = Arc::new(RwLock::new(PrecacheProgress {
        current: 0,
        total: 0,
        active: false,
        current_id: None,
    }));

    let (playback_tx, playback_rx) = tokio::sync::watch::channel(false);
    let playback_tx = Arc::new(playback_tx);

    // ── Shutdown channel ─────────────────────────────────────────────────
    // Sending `true` signals all background workers to stop at their next
    // checkpoint.  The sender is wrapped in Arc so it can be shared with
    // the signal-handler task.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);

    // ── HTTP server worker threads ────────────────────────────────────────
    // Each actix-web worker is a dedicated OS thread.  The default (num_cpus)
    // is far more than needed for a local media server because the actual
    // CPU-intensive work (transcoding, sprite generation, thumbnailing) runs
    // on tokio `spawn_blocking` threads, not on the HTTP workers themselves.
    // Keeping this small dramatically reduces the visible "starfin" thread
    // count reported by tools like `ps`.
    //
    // Override with the HTTP_WORKERS environment variable.
    let http_workers = std::env::var("HTTP_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok().filter(|&n| n > 0))
        .unwrap_or(2);
    info!(workers = http_workers, "HTTP server worker threads (set HTTP_WORKERS to override)");

    // ── On-demand playback transcode semaphore ───────────────────────────
    // Limits the number of simultaneous on-demand segment transcode operations
    // so that concurrent DASH segment requests don't overload the system.  Used
    // exclusively by the `get_segment` handler; the pre-cache background
    // worker is fully suspended during playback and does not compete for
    // these permits.  Defaults to the number of available CPU threads; override
    // with the TRANSCODE_CONCURRENCY environment variable.
    let transcode_concurrency = std::env::var("TRANSCODE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok().filter(|&n| n > 0))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                // Fall back to 4 if the OS cannot report parallelism — a
                // reasonable minimum that avoids saturating most systems while
                // still allowing meaningful concurrent load.
                .unwrap_or(4)
        });
    info!(limit = transcode_concurrency, "max concurrent on-demand transcodes (set TRANSCODE_CONCURRENCY to override)");
    let transcode_semaphore = Arc::new(tokio::sync::Semaphore::new(transcode_concurrency));

    // ── Password protection ──────────────────────────────────────────────
    let password_protection = std::env::var("PASSWORD_PROTECTION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
        .unwrap_or(false);
    let password_hash_path = cache_dir.join(".hash");
    let auth_tokens: Arc<RwLock<HashSet<String>>> = Arc::new(RwLock::new(HashSet::new()));

    if password_protection {
        info!("password protection: ENABLED");
        if password_hash_path.exists() {
            info!(path = %password_hash_path.display(), "password hash found");
        } else {
            warn!("no password set — first visitor will be prompted to create one");
        }
    } else {
        info!("password protection: disabled");
    }

    // ── Theme & Design ──────────────────────────────────────────────────
    let theme = resolve_theme();
    let design = resolve_design(&theme);
    let design_css = design.to_css();
    let theme_css_raw = theme.to_css();
    // Design CSS is loaded first so theme colors can override design defaults.
    let theme_css = if design_css.is_empty() {
        theme_css_raw
    } else {
        format!("{}{}", design_css, theme_css_raw)
    };

    let library_version = Arc::new(AtomicU64::new(0));

    let state = web::Data::new(AppState {
        library_path: library_path.clone(),
        cache_dir: cache_dir.clone(),
        video_cache: Arc::clone(&video_cache),
        video_path_index: Arc::clone(&video_path_index),
        last_segment_access: RwLock::new(HashMap::new()),
        thumb_progress: Arc::clone(&thumb_progress),
        thumb_trigger: Arc::clone(&thumb_trigger),
        sprite_progress: Arc::clone(&sprite_progress),
        sprite_trigger: Arc::clone(&sprite_trigger),
        precache_progress: Arc::clone(&precache_progress),
        precache_trigger: Arc::clone(&precache_trigger),
        hwaccel: Arc::clone(&hwaccel),
        transcode_semaphore: Arc::clone(&transcode_semaphore),
        playback_tx: Arc::clone(&playback_tx),
        password_protection,
        password_hash_path,
        auth_tokens,
        segment_inflight: Arc::new(Mutex::new(HashMap::new())),
        video_segment_inflight: Arc::new(Mutex::new(HashMap::new())),
        audio_segment_inflight: Arc::new(Mutex::new(HashMap::new())),
        theme_css,
        playback_positions: Arc::new(RwLock::new(HashMap::new())),
        library_version: Arc::clone(&library_version),
        cache_strategy,
    });

    // One-time background scan at startup to refresh the index immediately.
    // The persisted cache is pre-loaded above so clients see known media
    // instantly; this scan updates with any new/removed files.
    {
        let startup_library = library_path.clone();
        let startup_cache_dir = cache_dir.clone();
        let startup_cache = Arc::clone(&video_cache);
        let startup_index = Arc::clone(&video_path_index);
        let startup_thumb_trigger = Arc::clone(&thumb_trigger);
        let startup_sprite_trigger = Arc::clone(&sprite_trigger);
        let startup_lib_ver = Arc::clone(&library_version);
        tokio::spawn(async move {
            let previous = startup_cache.read().clone();
            let (mut items, index) = scan_library(&startup_library).await;
            merge_user_metadata(&mut items, &previous);
            save_video_cache(&items, &startup_cache_dir);
            *startup_cache.write() = items;
            *startup_index.write() = index;
            startup_lib_ver.fetch_add(1, Ordering::Relaxed);
            startup_thumb_trigger.notify_one();
            startup_sprite_trigger.notify_one();
        });
    }

    // Background task: re-scan the library every 60 seconds.
    let bg_library_path = library_path.clone();
    let bg_cache_dir = cache_dir.clone();
    let bg_cache = Arc::clone(&video_cache);
    let bg_index = Arc::clone(&video_path_index);
    let bg_thumb_trigger = Arc::clone(&thumb_trigger);
    let bg_sprite_trigger = Arc::clone(&sprite_trigger);
    let bg_precache_trigger = Arc::clone(&precache_trigger);
    let bg_lib_ver = Arc::clone(&library_version);
    let mut bg_shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        // Skip the immediate first tick (covered by startup scan).
        tokio::select! {
            _ = interval.tick() => {}
            _ = bg_shutdown_rx.changed() => { return; }
        }
        loop {
            if *bg_shutdown_rx.borrow() { return; }
            tokio::select! {
                _ = interval.tick() => {}
                _ = bg_shutdown_rx.changed() => { return; }
            }
            if *bg_shutdown_rx.borrow() { return; }
            let (mut items, index) = scan_library(&bg_library_path).await;
            {
                let previous = bg_cache.read();
                merge_user_metadata(&mut items, &previous);
            }
            save_video_cache(&items, &bg_cache_dir);
            *bg_cache.write() = items;
            *bg_index.write() = index;
            bg_lib_ver.fetch_add(1, Ordering::Relaxed);
            bg_thumb_trigger.notify_one();
            bg_sprite_trigger.notify_one();
            bg_precache_trigger.notify_one();
        }
    });

    // ── Thumbnail background worker ───────────────────────────────────────────
    {
        let worker_library = library_path.clone();
        let worker_cache = cache_dir.clone();
        let worker_progress = Arc::clone(&thumb_progress);
        let worker_trigger = Arc::clone(&thumb_trigger);
        let worker_playback_rx = playback_rx.clone();
        let worker_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            run_thumb_worker(worker_library, worker_cache, worker_progress, worker_trigger, worker_playback_rx, worker_shutdown_rx).await;
        });
        // Kick off the first batch immediately after startup.
        thumb_trigger.notify_one();
    }
    // ─────────────────────────────────────────────────────────────────────────

    // ── Sprite background worker ──────────────────────────────────────────────
    {
        let worker_library = library_path.clone();
        let worker_cache = cache_dir.clone();
        let worker_progress = Arc::clone(&sprite_progress);
        let worker_trigger = Arc::clone(&sprite_trigger);
        let worker_playback_rx = playback_rx.clone();
        let worker_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            run_sprite_worker(worker_library, worker_cache, worker_progress, worker_trigger, worker_playback_rx, worker_shutdown_rx).await;
        });
        // Kick off the first batch immediately after startup.
        sprite_trigger.notify_one();
    }
    // ─────────────────────────────────────────────────────────────────────────

    // ── Segment pre-cache background worker ──────────────────────────────────
    {
        let worker_library = library_path.clone();
        let worker_cache = cache_dir.clone();
        let worker_progress = Arc::clone(&precache_progress);
        let worker_trigger = Arc::clone(&precache_trigger);
        let worker_playback_rx = playback_rx.clone();
        let worker_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            run_precache_worker(worker_library, worker_cache, precache_hwaccel, worker_progress, worker_trigger, worker_playback_rx, worker_shutdown_rx, cache_strategy).await;
        });
        // Kick off the first batch immediately after startup.
        precache_trigger.notify_one();
    }
    // ─────────────────────────────────────────────────────────────────────────

    // ── Playback monitor ─────────────────────────────────────────────────────
    // Every 2 seconds, check whether any video has had a recent segment
    // request.  While the channel value is `true`, background workers suspend
    // themselves between tasks (graceful suspend — any already-running task
    // finishes and saves its result before the worker pauses).  When playback
    // stops the channel flips to `false`, waking suspended workers so
    // processing resumes immediately without waiting for the next 60-second
    // library scan.
    {
        let monitor_state = state.clone();
        let monitor_tx = Arc::clone(&playback_tx);
        let monitor_thumb_trigger = Arc::clone(&thumb_trigger);
        let monitor_sprite_trigger = Arc::clone(&sprite_trigger);
        let monitor_precache_trigger = Arc::clone(&precache_trigger);
        let mut monitor_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                if *monitor_shutdown_rx.borrow() { return; }
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = monitor_shutdown_rx.changed() => { return; }
                }
                if *monitor_shutdown_rx.borrow() { return; }
                let is_playing = {
                    let map = monitor_state
                        .last_segment_access
                        .read();
                    map.values().any(|t| t.elapsed() < PLAYBACK_IDLE_TIMEOUT)
                };
                // Only send when the value actually changes to avoid
                // spuriously waking workers.
                let changed = monitor_tx.send_if_modified(|v| {
                    if *v == is_playing { false } else { *v = is_playing; true }
                });
                // When transitioning from playing → idle, re-trigger workers
                // so they resume immediately without waiting for the next
                // scheduled library scan.
                if changed && !is_playing {
                    monitor_thumb_trigger.notify_one();
                    monitor_sprite_trigger.notify_one();
                    monitor_precache_trigger.notify_one();
                }
            }
        });
    }
    // ─────────────────────────────────────────────────────────────────────────

    // ── Idle-eviction background task ────────────────────────────────────────
    // Every CACHE_SWEEP_INTERVAL, remove the cached segments of any video that
    // has not had a segment request for at least CACHE_IDLE_TIMEOUT.
    {
        let sweep_state = state.clone();
        let mut sweep_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CACHE_SWEEP_INTERVAL);
            loop {
                if *sweep_shutdown_rx.borrow() { return; }
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = sweep_shutdown_rx.changed() => { return; }
                }
                if *sweep_shutdown_rx.borrow() { return; }

                // Collect IDs whose caches have gone idle.
                // The read lock is held only for the in-memory scan; it is
                // released (by dropping `map`) before any filesystem work.
                let idle_ids: Vec<String> = {
                    let map = sweep_state
                        .last_segment_access
                        .read();
                    map.iter()
                        .filter(|(_, t)| t.elapsed() >= CACHE_IDLE_TIMEOUT)
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                for id in idle_ids {
                    let video_cache_dir = sweep_state.cache_dir.join(&id);
                    remove_non_precached_segments_all_qualities(&video_cache_dir, sweep_state.cache_strategy).await;
                    info!(video_id = %id, "cache evicted (idle)");
                    sweep_state
                        .last_segment_access
                        .write()
                        .remove(&id);
                }
            }
        });
    }
    // ─────────────────────────────────────────────────────────────────────────

    info!(library = %library_path.display(), cache = %cache_dir.display(), "starting starfin");
    // Bind to loopback by default; set BIND_ADDR=0.0.0.0 to expose to the network.
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1".into());
    info!(bind_addr = %bind_addr, port, "listening");

    let server = HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .wrap(Logger::default())
            .wrap(middleware::from_fn(auth_middleware))
            // ── Auth routes (always accessible) ──────────────────────────
            .route("/api/auth/status", web::get().to(auth_status))
            .route("/api/auth/set-password", web::post().to(set_password))
            .route("/api/auth/login", web::post().to(login))
            .route("/api/auth/logout", web::post().to(logout))
            // ── Theme (always accessible) ────────────────────────────────
            .route("/api/theme.css", web::get().to(get_theme_css))
            // ── Protected API routes ─────────────────────────────────────
            .route("/api/health", web::get().to(|| async { "ok" }))
            .route("/api/config", web::get().to(get_config))
            .route("/api/debug/transcode", web::get().to(get_transcode_debug))
            .route("/api/hwaccel", web::get().to(get_hwaccel))
            .route("/api/quality-options", web::get().to(get_quality_options))
            .route("/api/scan/ws", web::get().to(scan_ws))
            .route("/api/progress/ws", web::get().to(progress_ws))
            .route("/api/player/ws", web::get().to(player_ws))
            .route("/api/player/position/{id}", web::get().to(get_playback_position))
            .route("/api/thumbnails/progress", web::get().to(get_thumb_progress))
            .route("/api/videos", web::get().to(list_videos))
            .route(
                "/api/videos/{id}/metadata",
                web::patch().to(update_metadata),
            )
            .route("/api/videos/{id}/thumbnail", web::get().to(get_thumbnail))
            .route(
                "/api/videos/{id}/thumbnails/info",
                web::get().to(get_thumbnail_info),
            )
            .route(
                "/api/videos/{id}/thumbnails/sprite-status",
                web::get().to(get_sprite_status),
            )
            .route(
                "/api/videos/{id}/processing-status",
                web::get().to(get_processing_status),
            )
            .route(
                "/api/videos/{id}/cache-status",
                web::get().to(get_cache_status),
            )
            .route(
                "/api/videos/{id}/thumbnails/sprite.jpg",
                web::get().to(get_thumbnail_sprite),
            )
            .route(
                "/api/videos/{id}/subtitles",
                web::get().to(list_subtitles),
            )
            .route(
                "/api/videos/{id}/subtitles/{index}.vtt",
                web::get().to(get_subtitle),
            )
            .route(
                "/api/videos/{id}/manifest.mpd",
                web::get().to(get_manifest),
            )
            .route(
                "/api/videos/{id}/quality-info",
                web::get().to(get_video_quality_info),
            )
            .route(
                "/api/videos/{id}/stream",
                web::get().to(stream_video),
            )
            .route(
                "/api/videos/{id}/init.mp4",
                web::get().to(get_init_segment),
            )
            .route(
                "/api/videos/{id}/segments/{filename}",
                web::get().to(get_segment),
            )
            // ── Demuxed DASH-IF IOP v5 routes ─────────────────────────────
            .route(
                "/api/videos/{id}/video/{quality}/init.mp4",
                web::get().to(get_video_init),
            )
            .route(
                "/api/videos/{id}/audio/init.mp4",
                web::get().to(get_audio_init),
            )
            .route(
                "/api/videos/{id}/video/{quality}/{filename}",
                web::get().to(get_video_segment),
            )
            .route(
                "/api/videos/{id}/audio/{filename}",
                web::get().to(get_audio_segment),
            )
            // ─────────────────────────────────────────────────────────────
            .route(
                "/api/videos/{id}/cache",
                web::delete().to(clear_cache),
            )
            .route("/{tail:.*}", web::get().to(frontend))
    })
    .workers(http_workers)
    .bind((bind_addr.as_str(), port))?
    .run();

    let server_handle = server.handle();

    // ── Graceful shutdown on SIGINT / SIGTERM ─────────────────────────────────
    let shutdown_tx_signal = Arc::clone(&shutdown_tx);
    let playback_tx_signal = Arc::clone(&playback_tx);
    let semaphore_signal = Arc::clone(&transcode_semaphore);
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        let terminate = {
            let mut sig = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate(),
            )
            .expect("failed to install SIGTERM handler");
            async move { sig.recv().await }
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => info!("received SIGINT, shutting down gracefully"),
            _ = terminate => info!("received SIGTERM, shutting down gracefully"),
        }

        // Unblock workers that are paused waiting for playback to finish.
        let _ = playback_tx_signal.send(false);
        // Signal all background workers to exit their loops.
        let _ = shutdown_tx_signal.send(true);
        // Fast-fail any pending semaphore acquires in the precache worker.
        semaphore_signal.close();
        // Stop accepting new HTTP requests and drain in-flight ones.
        server_handle.stop(true).await;
    });
    // ─────────────────────────────────────────────────────────────────────────

    // ── Force-exit watchdog (OS thread) ──────────────────────────────────────
    // When actix-web receives SIGINT it handles the signal internally and
    // `server.await` returns, which drops the tokio runtime and cancels all
    // spawned tasks.  A tokio::spawn watchdog would be cancelled before it
    // could call process::exit().  An OS thread survives the runtime teardown.
    //
    // spawn_blocking threads (in-process ffmpeg transcodes) also cannot be
    // cancelled mid-execution; they keep the process alive until they finish.
    // This watchdog guarantees a timely exit regardless.
    {
        let watchdog_rx = shutdown_rx.clone();
        std::thread::spawn(move || {
            // Poll the shutdown channel until it fires.  We can't use async
            // here because this is a plain OS thread.
            loop {
                if *watchdog_rx.borrow() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // Brief grace period to let async tasks clean up.
            std::thread::sleep(std::time::Duration::from_secs(5));
            eprintln!("shutdown grace period elapsed, forcing exit");
            std::process::exit(0);
        });
    }
    // ─────────────────────────────────────────────────────────────────────────

    // `server.await` resolves when actix-web finishes (either from its own
    // internal SIGINT handling or from server_handle.stop()).  After it
    // returns, ensure all background workers are signalled and force exit
    // to terminate any lingering spawn_blocking threads immediately.
    let result = server.await;

    info!("HTTP server stopped, cleaning up");
    // Ensure shutdown is signalled even if actix handled the signal before
    // our tokio signal handler ran.
    let _ = shutdown_tx.send(true);
    let _ = playback_tx.send(false);
    transcode_semaphore.close();
    // Brief grace for async tasks to see the signal and exit cleanly.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    info!("forcing process exit");
    std::process::exit(result.map(|_| 0).unwrap_or(1));
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetson_theme_produces_empty_css() {
        let theme = theme_jetson();
        assert_eq!(theme.meta.name, "Jetson");
        assert!(theme.to_css().is_empty(), "Jetson (default) should emit no CSS overrides");
    }

    #[test]
    fn nord_theme_produces_css() {
        let theme = theme_nord();
        let css = theme.to_css();
        assert!(css.contains("/* Theme: Nord */"));
        assert!(css.contains(":root{"));
        assert!(css.contains(".app.dark-mode{"));
        assert!(css.contains("--accent: #5e81ac"));
    }

    #[test]
    fn catppuccin_theme_produces_css() {
        let theme = theme_catppuccin();
        let css = theme.to_css();
        assert!(css.contains("/* Theme: Catppuccin */"));
        assert!(css.contains("--accent: #8839ef"));
    }

    #[test]
    fn dracula_theme_produces_css() {
        let theme = theme_dracula();
        let css = theme.to_css();
        assert!(css.contains("/* Theme: Dracula */"));
        assert!(css.contains("--accent: #bd93f9"));
    }

    #[test]
    fn toml_round_trip() {
        let toml_str = r##"
[meta]
name = "Test Theme"

[light]
bg = "#ffffff"
accent = "#ff0000"

[dark]
bg = "#000000"
accent = "#00ff00"
"##;
        let config: ThemeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.meta.name, "Test Theme");
        let css = config.to_css();
        assert!(css.contains("--bg: #ffffff"));
        assert!(css.contains("--accent: #ff0000"));
        assert!(css.contains("--bg: #000000"));
        assert!(css.contains("--accent: #00ff00"));
    }

    #[test]
    fn partial_toml_works() {
        let toml_str = r##"
[meta]
name = "Minimal"

[light]
accent = "#123456"
"##;
        let config: ThemeConfig = toml::from_str(toml_str).unwrap();
        let css = config.to_css();
        assert!(css.contains("--accent: #123456"));
        // Dark section should be absent (no dark overrides).
        assert!(!css.contains(".app.dark-mode"));
    }

    #[test]
    fn example_toml_file_parses() {
        let contents = std::fs::read_to_string("themes/example.toml")
            .expect("themes/example.toml should exist");
        let config: ThemeConfig = toml::from_str(&contents)
            .expect("example.toml should be valid");
        assert_eq!(config.meta.name, "My Custom Theme");
        let css = config.to_css();
        assert!(css.contains(":root{"));
        assert!(css.contains(".app.dark-mode{"));
    }

    #[test]
    fn css_values_are_sanitized() {
        // Braces, semicolons, and angle brackets are stripped.
        assert_eq!(sanitize_css_value("red; } .x { color: blue"), "red  .x  color: blue");
        // url() is rejected.
        assert_eq!(sanitize_css_value("url(http://evil.com)"), "");
        // expression() is rejected.
        assert_eq!(sanitize_css_value("expression(alert(1))"), "");
        // javascript: is rejected.
        assert_eq!(sanitize_css_value("javascript:alert(1)"), "");
        // Normal values pass through unchanged.
        assert_eq!(sanitize_css_value("#ff4500"), "#ff4500");
        assert_eq!(
            sanitize_css_value("rgba(255,255,255,.6)"),
            "rgba(255,255,255,.6)"
        );
        assert_eq!(
            sanitize_css_value("2px solid rgba(0,0,0,.3)"),
            "2px solid rgba(0,0,0,.3)"
        );
    }

    // ── Design tests ──────────────────────────────────────────────────────

    #[test]
    fn editorial_design_produces_empty_css() {
        let design = design_editorial();
        assert_eq!(design.name, "Editorial");
        assert!(design.to_css().is_empty(), "Editorial (default) should emit no CSS");
    }

    #[test]
    fn neubrutalist_design_produces_css() {
        let design = design_neubrutalist();
        let css = design.to_css();
        assert!(css.contains("/* Design: Neubrutalist */"));
        assert!(css.contains("--font-body:"));
        assert!(css.contains("--border-width: 3px"));
        assert!(css.contains("--radius: 0px"));
        // Structural overrides present.
        assert!(css.contains("box-shadow:"));
    }

    #[test]
    fn aero_design_produces_css() {
        let design = design_aero();
        let css = design.to_css();
        assert!(css.contains("/* Design: Aero */"));
        assert!(css.contains("--font-body:"));
        assert!(css.contains("--border-width: 1px"));
        assert!(css.contains("--radius: 16px"));
        assert!(css.contains("backdrop-filter:"));
    }

    #[test]
    fn design_tokens_merge() {
        let mut base = DesignTokens {
            font_body: Some("monospace".into()),
            border_width: Some("2px".into()),
            ..Default::default()
        };
        let overrides = DesignTokens {
            border_width: Some("5px".into()),
            heading_weight: Some("400".into()),
            ..Default::default()
        };
        base.merge(&overrides);
        assert_eq!(base.font_body.as_deref(), Some("monospace"));
        assert_eq!(base.border_width.as_deref(), Some("5px"));
        assert_eq!(base.heading_weight.as_deref(), Some("400"));
    }

    #[test]
    fn toml_with_design_section_parses() {
        let toml_str = r##"
[meta]
name = "Design Test"
design = "aero"

[design]
font_body = "system-ui, sans-serif"

[light]
accent = "#123456"
"##;
        let config: ThemeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.meta.name, "Design Test");
        assert_eq!(config.meta.design.as_deref(), Some("aero"));
        assert_eq!(config.design.font_body.as_deref(), Some("system-ui, sans-serif"));
        let css = config.to_css();
        assert!(css.contains("--accent: #123456"));
    }
}


