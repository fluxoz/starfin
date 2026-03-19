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
use rust_embed::RustEmbed;
use mime_guess::MimeGuess;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
        { "value": "high",     "label": Quality::High.label() },
        { "value": "medium",   "label": Quality::Medium.label() },
        { "value": "low",      "label": Quality::Low.label() },
    ]))
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

// ── App state ────────────────────────────────────────────────────────────────

/// Tracks the progress of the thumbnail generation background job.
struct ThumbProgress {
    current: u32,
    total: u32,
    active: bool,
    /// Which generation phase is running: `"quick"` or `"deep"`.
    phase: &'static str,
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
    /// Progress counters for the background deep-thumbnail generation worker.
    thumb_progress: Arc<RwLock<ThumbProgress>>,
    /// Notified to (re-)start the deep thumbnail generation batch.
    thumb_trigger: Arc<tokio::sync::Notify>,
    /// Progress counters for the background sprite generation worker.
    sprite_progress: Arc<RwLock<SpriteProgress>>,
    /// Notified to (re-)start the sprite generation batch.
    sprite_trigger: Arc<tokio::sync::Notify>,
    /// Progress counters for the background segment pre-caching worker.
    precache_progress: Arc<RwLock<PrecacheProgress>>,
    /// Notified to (re-)start the segment pre-caching batch.
    precache_trigger: Arc<tokio::sync::Notify>,
    /// Detected hardware acceleration backend (detected once at startup).
    hwaccel: HwAccel,
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
    /// Pre-rendered CSS for the active theme (served at `/api/theme.css`).
    theme_css: String,
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

        // Re-trigger deep thumbnail generation for any newly discovered videos.
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
/// (quick random-frame grab first, then upgraded to a signalstats-selected
/// frame).  If the thumbnail has not yet been generated this returns 404 so
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

/// Quick one-shot thumbnail: seeks to a **fresh random** position within
/// 20–80% of the video runtime and grabs a single frame.  The position is
/// Quick thumbnail: seek to a random position in [20%, 80%) of the video and
/// extract a single frame as JPEG.  Uses the in-process ffmpeg-next library.
async fn generate_quick_thumbnail(
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
    let duration = duration_secs;

    // Pick a fresh random position in [20 %, 80 %) of the runtime.
    let random_byte = Uuid::new_v4().as_bytes()[0];
    let fraction = random_byte as f64 / 255.0;
    let seek_secs = (duration * (0.20 + fraction * 0.60)).max(1.0);

    let video_path = video_path.to_path_buf();
    let thumb_path_clone = thumb_path.clone();

    // Run the CPU-intensive frame extraction on a blocking thread.  The
    // result is always awaited to completion: letting the task finish means
    // the thumbnail is saved and won't need to be re-done when workers
    // resume after playback ends.  The outer worker loop suspends between
    // tasks while playback is active, so no new work is started during
    // playback.
    tokio::task::spawn_blocking(move || {
        media::thumbnail::extract_frame_as_jpeg(&video_path, seek_secs, &thumb_path_clone)
    })
    .await
    .unwrap_or(false)
}

/// Two-pass deep thumbnail using in-process signalstats analysis.
///
/// Pass 1 — analyse frames in the 20–80% window using YUV signal statistics
/// to find the most visually appealing frame (highest saturation, lowest
/// out-of-range pixel ratio).
///
/// Pass 2 — extract and encode that specific frame as JPEG.
///
/// A side-car marker file `{id}.deep` is created on success.
///
/// When `kill` is set to `true`, pass 1 bails out early and pass 2 is skipped
/// entirely so that background work yields I/O and CPU to playback.
async fn generate_deep_thumbnail(
    id: &str,
    video_path: &Path,
    cache_dir: &Path,
    kill: Arc<AtomicBool>,
) -> bool {
    let deep_marker = cache_dir.join(format!("{}.deep", id));
    if deep_marker.exists() {
        return true;
    }

    let (duration_secs, _) = probe_video(video_path).await;
    if duration_secs <= 0.0 {
        return false;
    }

    let duration = duration_secs;
    let start = duration * 0.20;
    let length = duration * 0.60;

    let video_path_owned = video_path.to_path_buf();
    let default_time = start + length * 0.5;

    // Pass 1: find the best frame time via in-process signal analysis.
    let video_path_for_analysis = video_path_owned.clone();
    let kill_for_analysis = kill.clone();
    let best_time = tokio::task::spawn_blocking(move || {
        media::thumbnail::find_best_frame_via_signalstats(
            &video_path_for_analysis,
            start,
            length,
            default_time,
            &kill_for_analysis,
        )
    })
    .await
    .unwrap_or(default_time);

    // Bail before pass 2 if playback has started.
    if kill.load(Ordering::Relaxed) {
        return false;
    }

    // Pass 2: extract the chosen frame.
    let thumb_path = cache_dir.join(format!("{}.jpg", id));
    let video_path_for_extract = video_path_owned.clone();
    let thumb_path_clone = thumb_path.clone();

    let success = tokio::task::spawn_blocking(move || {
        media::thumbnail::extract_frame_as_jpeg(&video_path_for_extract, best_time, &thumb_path_clone)
    })
    .await
    .unwrap_or(false);

    if success {
        let _ = tokio::fs::write(&deep_marker, b"").await;
        true
    } else {
        false
    }
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

/// Background worker that processes videos one at a time in two sequential
/// phases.
///
/// **Phase 1 — quick thumbnails**: for every video whose `.jpg` is absent,
/// grab a single deterministic random frame within 20–80% of the runtime.
/// This is fast (one short ffmpeg invocation per file) and gives the UI
/// something to show immediately.
///
/// **Phase 2 — deep thumbnails**: for every video whose `.deep` marker is
/// absent, run the two-pass signalstats analysis to select and extract the
/// most visually representative frame, then replace the quick thumbnail with
/// the better one.
///
/// Both phases are triggered by a notification on `trigger` (sent at startup
/// and after every library re-scan).  Progress counters are written to
/// `progress` so `GET /api/thumbnails/progress` can drive the frontend bar.
///
/// All ffmpeg invocations in this worker suppress their stdout **and** stderr
/// so no ffmpeg output appears in the main process.
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

        // ── Phase 1: quick thumbnails ─────────────────────────────────────

        let (quick_done, quick_entries): (Vec<_>, Vec<_>) = WalkDir::new(&library_path)
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
            p.current = quick_done.len() as u32;
            p.total = (quick_done.len() + quick_entries.len()) as u32;
            p.active = !quick_entries.is_empty();
            p.phase = "quick";
        }

        let mut join_set: tokio::task::JoinSet<(String, bool)> = tokio::task::JoinSet::new();
        let mut iter = quick_entries.into_iter().peekable();
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
                    let ok = generate_quick_thumbnail(&id, &abs, &cache_dir).await;
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

        if *shutdown_rx.borrow() {
            return;
        }

        // ── Phase 2: deep thumbnails ──────────────────────────────────────

        let (deep_done, deep_entries): (Vec<_>, Vec<_>) = WalkDir::new(&library_path)
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
                cache_dir.join(format!("{}.deep", id)).exists()
            });

        {
            let mut p = progress.write();
            p.current = deep_done.len() as u32;
            p.total = (deep_done.len() + deep_entries.len()) as u32;
            p.active = !deep_entries.is_empty();
            p.phase = "deep";
        }

        let kill = Arc::new(AtomicBool::new(false));
        let mut join_set: tokio::task::JoinSet<(String, bool)> = tokio::task::JoinSet::new();
        let mut iter = deep_entries.into_iter().peekable();
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
                    let ok = generate_deep_thumbnail(&id, &abs, &cache_dir, k).await;
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

// ─────────────────────────────────────────────────────────────────────────────

/// `GET /api/thumbnails/progress` — current thumbnail generation progress.
///
/// Returns `{"current":N,"total":M,"active":bool,"phase":"quick"|"deep"}`.
/// The frontend polls this every few seconds to drive the progress bar on the
/// homepage.
#[derive(Clone, Serialize)]
struct ThumbProgressResponse {
    current: u32,
    total: u32,
    active: bool,
    phase: String,
}

async fn get_thumb_progress(state: web::Data<AppState>) -> impl Responder {
    let p = state.thumb_progress.read();
    HttpResponse::Ok().json(ThumbProgressResponse {
        current: p.current,
        total: p.total,
        active: p.active,
        phase: p.phase.to_owned(),
    })
}

/// `GET /api/progress/ws` — persistent WebSocket that streams live progress
/// updates from the thumbnail, sprite, and pre-cache background workers at
/// 500 ms intervals.
///
/// Each frame is a JSON text message:
/// ```json
/// {
///   "thumb":    { "current": N, "total": M, "active": bool, "phase": "quick", "current_ids": ["uuid", ...] },
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

    actix_web::rt::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(500));
        loop {
            ticker.tick().await;

            let (tc, tt, ta, tph, tids) = {
                let p = thumb_progress.read();
                let ids: Vec<String> = p.current_ids.iter().cloned().collect();
                (p.current, p.total, p.active, p.phase, ids)
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

            let msg = serde_json::json!({
                "thumb":    { "current": tc, "total": tt, "active": ta, "phase": tph, "current_ids": tids },
                "sprite":   { "current": sc, "total": st, "active": sa, "current_ids": sids },
                "precache": { "current": pc, "total": pt, "active": pa, "current_id": pid }
            })
            .to_string();

            if session.text(msg).await.is_err() {
                break; // Client disconnected.
            }
        }
    });

    Ok(response)
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
/// segment directory.  Segments with index < [`PRECACHE_SEGMENTS`] are
/// preserved so that playback can always begin instantly.
async fn remove_non_precached_segments(cache_dir: &Path) -> std::io::Result<()> {
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
            // Keep segments that are either in the dense pre-cache window OR are sparse seek anchors.
            let should_keep = idx < PRECACHE_SEGMENTS || idx % SPARSE_CACHE_STRIDE == 0;
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
/// video's cache folder (`{cache_dir}/{video_id}/{quality}/`).
async fn remove_non_precached_segments_all_qualities(video_cache_dir: &Path) {
    for quality_name in [Quality::Original.as_str(), Quality::High.as_str(), Quality::Medium.as_str(), Quality::Low.as_str()] {
        let q_dir = video_cache_dir.join(quality_name);
        if q_dir.exists() {
            if let Err(e) = remove_non_precached_segments(&q_dir).await {
                error!(dir = %q_dir.display(), error = %e, "cache eviction error");
            }
        }
    }
}

/// `GET /api/videos/{id}/manifest.mpd`
///
/// Generates a DASH MPD (Media Presentation Description) manifest for VOD
/// playback using fMP4 (CMAF) segments.
///
/// Accepts an optional `?quality=high|medium|low` query parameter (default:
/// `original`).  Segment URLs embed the same quality token so that the DASH
/// client fetches segments at the correct quality level.
///
/// This follows the DASH-IF IOP guidelines:
/// - fMP4 segment format (CMAF, requires init segment + media segments)
/// - Static MPD type for VOD content
/// - Segments are transcoded on-demand when first requested
async fn get_manifest(
    id: web::Path<String>,
    query: web::Query<QualityQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let quality = query.quality;

    let (abs_path, _) = match find_video(&state, &id) {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    // Get video duration via ffprobe (metadata is not needed for manifest generation)
    let (duration_secs, _metadata) = probe_video(&abs_path).await;
    if duration_secs <= 0.0 {
        return HttpResponse::ServiceUnavailable()
            .body("Could not determine video duration. Ensure ffprobe is installed and the video file is valid.");
    }

    // Segments are stored in a quality-specific subdirectory.
    let seg_dir = state.cache_dir.join(id.as_str()).join(quality.as_str());
    if let Err(e) = tokio::fs::create_dir_all(&seg_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    // Calculate number of segments based on duration (f64 for sub-second precision).
    let duration = duration_secs;
    let num_segments = (duration / SEGMENT_DURATION).ceil() as usize;

    // Format duration as ISO 8601 duration for MPD.
    // Use fractional seconds so that the frontend gets sub-second precision,
    // matching how dash.js MPD parser handles mediaPresentationDuration.
    let hours = (duration as u64) / 3600;
    let minutes = ((duration as u64) % 3600) / 60;
    let frac_seconds = duration - (hours * 3600 + minutes * 60) as f64;
    let pt_duration = format!("PT{hours}H{minutes}M{frac_seconds:.3}S");

    // Build the DASH MPD manifest.
    let mut mpd = String::new();
    mpd.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    mpd.push_str(&format!(
        "<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\" \
         profiles=\"urn:mpeg:dash:profile:isoff-live:2011\" \
         type=\"static\" \
         mediaPresentationDuration=\"{pt_duration}\" \
         minBufferTime=\"PT2S\">\n"
    ));
    mpd.push_str(&format!("  <Period duration=\"{pt_duration}\">\n"));
    mpd.push_str(&format!(
        "    <AdaptationSet mimeType=\"video/mp4\" contentType=\"video\" \
         segmentAlignment=\"true\" subsegmentAlignment=\"true\" \
         subsegmentStartsWithSAP=\"1\">\n"
    ));
    mpd.push_str(&format!(
        "      <Representation id=\"{quality}\" bandwidth=\"2000000\">\n",
        quality = quality.as_str()
    ));

    // SegmentTemplate with explicit SegmentTimeline for precise duration control.
    mpd.push_str(&format!(
        "        <SegmentTemplate timescale=\"1000\" \
         initialization=\"/api/videos/{id}/init.mp4?quality={quality}\" \
         media=\"/api/videos/{id}/segments/seg_$Number%05d$.m4s?quality={quality}\" \
         startNumber=\"0\">\n",
        id = *id,
        quality = quality.as_str()
    ));

    // Build SegmentTimeline — use the `r` (repeat) attribute per DASH-IF IOP
    // to compress identical-duration segments into a single <S> element.
    mpd.push_str("          <SegmentTimeline>\n");
    let normal_duration_ms = (SEGMENT_DURATION * 1000.0) as u64;
    if num_segments > 1 {
        // First (num_segments - 1) segments all have the same duration.
        // r= gives additional repetitions beyond the first occurrence, so
        // r=(num_segments - 2) encodes (num_segments - 1) segments total.
        let repeats = num_segments - 2; // r=N → N+1 segments of this duration
        mpd.push_str(&format!(
            "            <S d=\"{normal_duration_ms}\" r=\"{repeats}\"/>\n"
        ));
        // Last segment may be shorter.
        let last_start = (num_segments - 1) as f64 * SEGMENT_DURATION;
        let last_duration_ms = ((duration - last_start) * 1000.0).max(1.0) as u64;
        mpd.push_str(&format!(
            "            <S d=\"{last_duration_ms}\"/>\n"
        ));
    } else if num_segments == 1 {
        let seg_duration_ms = (duration * 1000.0).max(1.0) as u64;
        mpd.push_str(&format!(
            "            <S d=\"{seg_duration_ms}\"/>\n"
        ));
    }
    mpd.push_str("          </SegmentTimeline>\n");
    mpd.push_str("        </SegmentTemplate>\n");
    mpd.push_str("      </Representation>\n");
    mpd.push_str("    </AdaptationSet>\n");
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

    // Check cache first.
    let seg_dir = state.cache_dir.join(id.as_str()).join(quality.as_str());
    let init_path = seg_dir.join("init.mp4");

    if let Ok(data) = tokio::fs::read(&init_path).await {
        return HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(data);
    }

    // Generate init segment.
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

    let hwaccel = state.hwaccel.clone();
    let init_data = match tokio::task::spawn_blocking(move || {
        media::transcode::create_init_segment(&abs_str, quality, &hwaccel)
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

    // If segment exists, serve it immediately from cache
    if let Ok(data) = tokio::fs::read(&seg_path).await {
        return HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(data);
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
        let result = transcode_segment(&abs_str, &seg_dir, seg_index, &state.hwaccel, quality).await;

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
                Ok(data) => HttpResponse::Ok()
                    .content_type("video/mp4")
                    .insert_header((
                        header::CACHE_CONTROL,
                        "public, max-age=31536000, immutable",
                    ))
                    .body(data),
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
    HttpResponse::Ok().json(serde_json::json!({
        "label":   state.hwaccel.label(),
        "encoder": state.hwaccel.encoder(),
    }))
}

/// `DELETE /api/videos/{id}/cache` — clear cached segments for a video.
///
/// Removes non-pre-cached segments from all quality subdirectories of
/// `cache_dir/{id}/`.  The first [`PRECACHE_SEGMENTS`] segments are preserved
/// so that future playback can begin instantly.  Called by the frontend when
/// the user navigates away from the player so that disk space is reclaimed
/// immediately.
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

    remove_non_precached_segments_all_qualities(&video_cache_dir).await;

    // Also cancel idle-eviction tracking so a stale entry doesn't
    // trigger a redundant removal on the next sweep.
    state
        .last_segment_access
        .write()
        .remove(&id);

    HttpResponse::NoContent().finish()
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
/// - `{"status":"processed"}` — all operations complete: quick thumbnail
///   (`.jpg`), deep thumbnail (`.deep` marker), sprite sheet (`_thumbs/sprite.jpg`),
///   and segment pre-cache (first [`PRECACHE_SEGMENTS`] `.m4s` files)
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

    let quick_marker = state.cache_dir.join(format!("{}.jpg", *id));
    let deep_marker = state.cache_dir.join(format!("{}.deep", *id));
    let sprite_path = state
        .cache_dir
        .join(format!("{}_thumbs", *id))
        .join("sprite.jpg");

    // Check whether the pre-cached segments exist.  We only check for
    // seg_00000.m4s as a lightweight proxy — if the pre-cache worker
    // finished, all PRECACHE_SEGMENTS files will be present.
    // Segments are now stored in quality-specific subdirectories; the
    // precache worker always operates on the `original` quality level
    // (direct remux for compatible sources, fast transcode fallback).
    let precache_marker = state
        .cache_dir
        .join(id.as_str())
        .join(Quality::Original.as_str())
        .join("seg_00000.m4s");

    let all_done = quick_marker.exists()
        && deep_marker.exists()
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
/// on `trigger`, walks the library, skips videos whose first
/// [`PRECACHE_SEGMENTS`] segments already exist in the cache, and transcodes
/// the missing ones.  Suspends while playback is active (checking between
/// every individual segment) and resumes automatically once idle.  Progress
/// counters are written to `progress` so the WS can drive a frontend progress
/// bar.
async fn run_precache_worker(
    library_path: PathBuf,
    cache_dir: PathBuf,
    hwaccel: HwAccel,
    progress: Arc<RwLock<PrecacheProgress>>,
    trigger: Arc<tokio::sync::Notify>,
    mut playback_rx: tokio::sync::watch::Receiver<bool>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
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

        let entries: Vec<_> = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .collect();

        // Partition into already-cached and needs-work.  A video counts as
        // "done" when seg_00000.m4s already exists in the high-quality
        // subdirectory AND the last expected sparse anchor also exists
        // (so sparse anchors are added on re-runs if they were missing from
        // a previous precache pass).  We probe the duration here so that we
        // can compute the last anchor index without re-probing in the loop.
        let mut done_count: usize = 0;
        let mut pending: Vec<_> = Vec::new();
        for e in entries {
            let abs = e.path();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(abs)
                .to_string_lossy();
            let id = video_id(&rel);
            // Precache always uses the Original quality subdirectory.
            let hls_dir = cache_dir.join(&id).join(Quality::Original.as_str());

            let is_done = if !hls_dir.join("seg_00000.m4s").exists() {
                false
            } else {
                // First segment exists; check whether the last expected sparse
                // anchor is also present.
                let (dur_secs, _) = probe_video(abs).await;
                if dur_secs <= 0.0 {
                    // Can't determine duration — treat as done to avoid infinite retry.
                    true
                } else {
                    let total_segs = (dur_secs / SEGMENT_DURATION).ceil() as usize;
                    if total_segs > PRECACHE_SEGMENTS {
                        let last_anchor =
                            ((total_segs - 1) / SPARSE_CACHE_STRIDE) * SPARSE_CACHE_STRIDE;
                        if last_anchor >= PRECACHE_SEGMENTS {
                            hls_dir.join(format!("seg_{:05}.m4s", last_anchor)).exists()
                        } else {
                            true
                        }
                    } else {
                        true
                    }
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
            // Precache always uses the Original quality subdirectory.
            let hls_dir = cache_dir.join(&id).join(Quality::Original.as_str());

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

            // Dense: every segment in the initial pre-cache window for instant playback start.
            // Sparse: every SPARSE_CACHE_STRIDE-th segment beyond that window as seek anchors.
            let segments_to_cache: Vec<usize> = (0..total_segments)
                .filter(|&i| i < PRECACHE_SEGMENTS || i % SPARSE_CACHE_STRIDE == 0)
                .collect();

            // Collect only the segments that are missing.
            let missing: Vec<usize> = segments_to_cache.iter()
                .copied()
                .filter(|i| !hls_dir.join(format!("seg_{:05}.m4s", i)).exists())
                .collect();
            if missing.is_empty() {
                progress.write().advance();
                continue;
            }

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

            if let Err(e) = tokio::fs::create_dir_all(&hls_dir).await {
                error!(video_id = %id, error = %e, "precache: cache dir error");
                progress.write().advance();
                continue;
            }

            info!(
                video_id = %id,
                missing_segments = missing.len(),
                total_segments = segments_to_cache.len(),
                "pre-caching segments"
            );

            let kill = Arc::new(AtomicBool::new(false));
            for i in missing {
                if *shutdown_rx.borrow() {
                    return;
                }
                // Suspend between individual segments while playback is
                // active: signal the in-flight transcode to bail out, then
                // wait for playback to end before starting the next one.
                if *playback_rx.borrow() {
                    kill.store(true, Ordering::SeqCst);
                    while *playback_rx.borrow() {
                        let _ = playback_rx.changed().await;
                    }
                    kill.store(false, Ordering::SeqCst);
                }
                if *shutdown_rx.borrow() {
                    return;
                }

                // Run the segment creation inside a select! so a shutdown
                // or playback signal wakes us immediately rather than
                // waiting for the full operation to finish.  The kill flag
                // ensures the spawn_blocking task also bails out quickly.
                let result = tokio::select! {
                    r = media::transcode::transcode_segment_with_kill(&abs_str, &hls_dir, i, &hwaccel, Quality::Original, Arc::clone(&kill)) => r,
                    _ = playback_rx.changed() => {
                        if *playback_rx.borrow() {
                            kill.store(true, Ordering::SeqCst);
                        }
                        continue;
                    }
                    _ = shutdown_rx.changed() => { return; }
                };
                if let Err(e) = result {
                    if e == media::transcode::CANCELLED {
                        // Cancelled by playback — retry this segment later.
                        continue;
                    }
                    error!(video_id = %id, segment = i, error = %e, "precache: segment transcode failed");
                    break; // Stop for this video on error.
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

    // Remove any *.tmp files left behind by a previous shutdown.
    // These are always incomplete and can never be reused.
    info!("Cleaning any oprhaned temp files.");
    cleanup_orphaned_tmp_files(&cache_dir);

    // ── Startup healthchecks (logged for journalctl) ─────────────────────
    run_startup_healthchecks(&library_path, &cache_dir).await;
    let hwaccel = media::hwaccel::detect_hwaccel().await;

    // Load any previously-persisted video index so the server starts with
    // known media immediately.
    let initial_items = load_video_cache(&cache_dir);
    if !initial_items.is_empty() {
        info!(count = initial_items.len(), "loaded videos from persisted index");
    }
    let video_cache: Arc<RwLock<Vec<VideoItem>>> = Arc::new(RwLock::new(initial_items));

    // Build the initial path index via a fast, probe-free library walk so that
    // `find_video` works immediately—before the background scan completes.
    let initial_index = build_video_index(&library_path);
    let video_path_index: Arc<RwLock<VideoPathIndex>> =
        Arc::new(RwLock::new(initial_index));

    let thumb_progress = Arc::new(RwLock::new(ThumbProgress {
        current: 0,
        total: 0,
        active: false,
        phase: "quick",
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
    let precache_hwaccel = hwaccel.clone();
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
        hwaccel,
        transcode_semaphore: Arc::clone(&transcode_semaphore),
        playback_tx: Arc::clone(&playback_tx),
        password_protection,
        password_hash_path,
        auth_tokens,
        segment_inflight: Arc::new(Mutex::new(HashMap::new())),
        theme_css,
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
        tokio::spawn(async move {
            let previous = startup_cache.read().clone();
            let (mut items, index) = scan_library(&startup_library).await;
            merge_user_metadata(&mut items, &previous);
            save_video_cache(&items, &startup_cache_dir);
            *startup_cache.write() = items;
            *startup_index.write() = index;
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
            bg_thumb_trigger.notify_one();
            bg_sprite_trigger.notify_one();
            bg_precache_trigger.notify_one();
        }
    });

    // ── Deep thumbnail background worker ─────────────────────────────────────
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
            run_precache_worker(worker_library, worker_cache, precache_hwaccel, worker_progress, worker_trigger, worker_playback_rx, worker_shutdown_rx).await;
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
                    remove_non_precached_segments_all_qualities(&video_cache_dir).await;
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
            // ── Theme (always accessible) ────────────────────────────────
            .route("/api/theme.css", web::get().to(get_theme_css))
            // ── Protected API routes ─────────────────────────────────────
            .route("/api/health", web::get().to(|| async { "ok" }))
            .route("/api/debug/transcode", web::get().to(get_transcode_debug))
            .route("/api/hwaccel", web::get().to(get_hwaccel))
            .route("/api/quality-options", web::get().to(get_quality_options))
            .route("/api/scan/ws", web::get().to(scan_ws))
            .route("/api/progress/ws", web::get().to(progress_ws))
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
                "/api/videos/{id}/init.mp4",
                web::get().to(get_init_segment),
            )
            .route(
                "/api/videos/{id}/segments/{filename}",
                web::get().to(get_segment),
            )
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


