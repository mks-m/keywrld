//! Shared overlay styling. Read desktop settings without changing global GTK state.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use gtk4::{gdk, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl Color {
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red: red as f64 / 255.0,
            green: green as f64 / 255.0,
            blue: blue as f64 / 255.0,
            alpha: 1.0,
        }
    }

    pub const fn with_alpha(self, alpha: f64) -> Self {
        Self { alpha, ..self }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ControlStyle {
    pub fill: Color,
    pub border: Color,
    pub width: f64,
}

#[derive(Clone, Debug)]
pub struct Style {
    pub scrim: Color,
    pub darker_background: Color,
    pub lighter_background: Color,
    pub muted: Color,
    pub red: Color,
    pub selected: ControlStyle,
    pub background: Color,
    pub foreground: Color,
    pub secondary: Color,
    pub accent: Color,
    pub border: Color,
    pub border_width: f64,
    pub radius: f64,
    pub controls: [ControlStyle; 3], // normal, hover, focus
    pub selection: Color,
    pub selected_text: Color,
    pub row_hover: ControlStyle,
    pub row_hover_text: Color,
    pub font: String,
    pub body: f64,
    pub caption: f64,
    pub heading: f64,
    pub padding_x: f64,
    pub padding_y: f64,
    pub row_gap: f64,
    pub row_padding_x: f64,
}

impl Default for Style {
    fn default() -> Self {
        let background = Color::rgb(26, 27, 38);
        let foreground = Color::rgb(192, 202, 245);
        let accent = Color::rgb(122, 162, 247);
        let muted = Color::rgb(65, 72, 104);
        let selection = Color::rgb(41, 46, 66);
        Self {
            scrim: background.with_alpha(0.5),
            darker_background: Color::rgb(14, 14, 20),
            lighter_background: Color::rgb(36, 40, 59),
            muted,
            red: Color::rgb(247, 118, 142),
            selected: ControlStyle {
                fill: selection,
                border: accent,
                width: 1.0,
            },
            background,
            foreground,
            secondary: Color::rgb(86, 95, 137),
            accent,
            border: muted,
            border_width: 1.0,
            radius: 7.0,
            controls: [ControlStyle {
                fill: Color::rgb(14, 14, 20),
                border: muted,
                width: 1.0,
            }; 3],
            selection,
            selected_text: foreground,
            row_hover: ControlStyle {
                fill: selection,
                border: accent,
                width: 1.0,
            },
            row_hover_text: foreground,
            font: "sans".into(),
            body: 12.0,
            caption: 10.0,
            heading: 15.0,
            padding_x: 10.0,
            padding_y: 7.0,
            row_gap: 6.0,
            row_padding_x: 10.0,
        }
    }
}

// Only the scalar shell tokens used by the overlay are read; this does not execute theme/config code.
#[derive(Default)]
struct Values(BTreeMap<String, String>);
impl Values {
    fn parse(input: &str) -> Self {
        let mut values = Self::default();
        let mut section = String::new();
        for line in input.lines().map(str::trim) {
            if line.starts_with('[') {
                section = line
                    .strip_prefix('[')
                    .and_then(|s| s.split_once(']'))
                    .map(|(s, _)| s.trim().to_owned())
                    .unwrap_or_default();
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let Some((key, raw)) = line.split_once('=') else {
                continue;
            };
            let raw = raw.trim();
            let value = if raw.starts_with(['"', '\'']) {
                let quote = raw.chars().next().unwrap();
                let Some(end) = raw[1..].find(quote) else {
                    continue;
                };
                &raw[1..end + 1]
            } else {
                raw.split('#').next().unwrap_or("").trim()
            };
            let key = if section.is_empty() {
                key.trim().into()
            } else {
                format!("{section}.{}", key.trim())
            };
            values.0.insert(key, value.into());
        }
        values
    }
    fn value(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    fn number(&self, key: &str) -> Option<f64> {
        self.value(key)?
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n >= 0.0)
    }
    fn color(&self, key: &str, palette: &Values) -> Option<Color> {
        self.resolve_color(self.value(key)?, palette)
    }
    fn resolve_color<'a>(&'a self, mut value: &'a str, palette: &'a Values) -> Option<Color> {
        for _ in 0..8 {
            if let Some(c) = parse_color(value) {
                return Some(c);
            }
            value = self.value(value).or_else(|| palette.value(value))?;
        }
        None
    }
    fn alpha(&self, key: &str, fallback: f64) -> f64 {
        self.number(key).unwrap_or(fallback).clamp(0.0, 1.0)
    }
}

fn resolved_colors(directory: &Path) -> Option<Values> {
    let file = directory.join("colors.toml");
    let input = fs::read_to_string(&file).ok()?;
    // Omarchy's line reader skips a final line without a newline. Normalize
    // only those files in a temporary copy; leave the installed theme alone.
    let temporary = if input.ends_with('\n') {
        None
    } else {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "keywrld-colors-{}-{stamp}.toml",
            std::process::id()
        ));
        let mut file = fs::File::create_new(&path).ok()?;
        if writeln!(file, "{input}").is_err() {
            let _ = fs::remove_file(&path);
            return None;
        }
        Some(path)
    };
    let output = Command::new("omarchy")
        .args(["theme", "color", "--all", "--file"])
        .arg(temporary.as_deref().unwrap_or(&file))
        .output();
    if let Some(path) = temporary {
        let _ = fs::remove_file(path);
    }
    let output = output.ok()?;
    if !output.status.success() {
        return None;
    }
    let values = String::from_utf8(output.stdout).ok()?;
    let values = Values(
        values
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once('\t')?;
                Some((key.to_owned(), value.to_owned()))
            })
            .collect(),
    );
    (!values.0.is_empty()).then_some(values)
}

fn parse_color(value: &str) -> Option<Color> {
    // Hyprland JSON gradients contain ARGB words; config gradients use rgb()/rgba().
    let first = value.split_whitespace().next()?;
    let rgba = if let Some(hex) = first.strip_prefix('#').filter(|s| s.len() == 6) {
        u32::from_str_radix(hex, 16).ok().map(|n| (n, 1.0))
    } else if let Some(hex) = first
        .strip_prefix("rgba(")
        .and_then(|s| s.strip_suffix(')'))
    {
        if hex.len() != 8 {
            return None;
        }
        u32::from_str_radix(hex, 16)
            .ok()
            .map(|n| (n >> 8, (n & 255) as f64 / 255.0))
    } else if first.len() == 8 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(first, 16)
            .ok()
            .map(|n| (n & 0xffffff, (n >> 24) as f64 / 255.0))
    } else if let Some(hex) = first.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        (hex.len() == 6)
            .then(|| u32::from_str_radix(hex, 16).ok())
            .flatten()
            .map(|n| (n, 1.0))
    } else {
        return gdk::RGBA::parse(first).ok().map(from_rgba);
    }?;
    Some(Color::rgb((rgba.0 >> 16) as u8, (rgba.0 >> 8) as u8, rgba.0 as u8).with_alpha(rgba.1))
}
fn from_rgba(c: gdk::RGBA) -> Color {
    Color {
        red: c.red() as f64,
        green: c.green() as f64,
        blue: c.blue() as f64,
        alpha: c.alpha() as f64,
    }
}
fn mix(a: Color, b: Color, amount: f64) -> Color {
    Color {
        red: a.red + (b.red - a.red) * amount,
        green: a.green + (b.green - a.green) * amount,
        blue: a.blue + (b.blue - a.blue) * amount,
        alpha: 1.0,
    }
}
fn opacity(c: Color, alpha: f64) -> Color {
    c.with_alpha(c.alpha * alpha)
}

#[derive(Default)]
struct Desktop {
    background: Option<Color>,
    foreground: Option<Color>,
    accent: Option<Color>,
    selection: Option<Color>,
    selected_text: Option<Color>,
    font: Option<(String, f64)>,
    radius: Option<f64>,
    border_width: Option<f64>,
    border: Option<Color>,
}

fn hypr_option(name: &str) -> Option<serde_json::Value> {
    let output = Command::new("hyprctl")
        .args(["-j", "getoption", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

impl Style {
    pub fn load(widget: &impl IsA<gtk4::Widget>) -> Self {
        let context = widget.style_context();
        let color = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| context.lookup_color(name).map(from_rgba))
        };
        let font = gtk4::Settings::default().and_then(|s| {
            let desc = gtk4::pango::FontDescription::from_string(&s.gtk_font_name()?);
            let family = desc.family()?.to_string();
            let size = desc.size() as f64 / gtk4::pango::SCALE as f64;
            let dpi = if s.gtk_xft_dpi() > 0 {
                s.gtk_xft_dpi() as f64 / 1024.0
            } else {
                96.0
            };
            Some((
                family,
                if desc.is_size_absolute() {
                    size
                } else {
                    size * dpi / 72.0
                },
            ))
        });
        let desktop = Desktop {
            background: color(&["window_bg_color", "theme_bg_color"]),
            foreground: color(&["window_fg_color", "theme_fg_color"]),
            accent: color(&["accent_color", "theme_selected_bg_color"]),
            selection: color(&["accent_bg_color", "theme_selected_bg_color"]),
            selected_text: color(&["accent_fg_color", "theme_selected_fg_color"]),
            font,
            ..Desktop::default()
        };
        Self::load_system(desktop)
    }

    #[cfg(test)]
    pub fn load_without_gtk() -> Self {
        Self::load_system(Desktop::default())
    }

    fn load_system(mut desktop: Desktop) -> Self {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            desktop.radius = hypr_option("decoration:rounding")
                .and_then(|v| v["int"].as_f64())
                .filter(|v| *v >= 0.0);
            desktop.border_width = hypr_option("general:border_size")
                .and_then(|v| v["int"].as_f64())
                .filter(|v| *v >= 0.0);
            desktop.border = hypr_option("general:col.active_border")
                .and_then(|v| parse_color(v["gradient"].as_str()?));
        }
        let root = std::env::var_os("XDG_STATE_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")));
        let theme = root.map(|p| p.join("omarchy/current/theme"));
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("omarchy/shell.toml"));
        Self::read_theme(theme.as_deref(), config.as_deref(), desktop)
    }

    fn read_theme(directory: Option<&Path>, user_shell: Option<&Path>, desktop: Desktop) -> Self {
        let read = |file| {
            directory
                .and_then(|p| fs::read_to_string(p.join(file)).ok())
                .unwrap_or_default()
        };
        let colors = directory
            .and_then(resolved_colors)
            .unwrap_or_else(|| Values::parse(&read("colors.toml")));
        let mut shell = Values::parse(&read("shell.toml"));
        if let Some(user) = user_shell.and_then(|p| fs::read_to_string(p).ok()) {
            shell.0.extend(Values::parse(&user).0);
        }
        Self::resolve(colors, shell, desktop)
    }

    fn resolve(p: Values, s: Values, desktop: Desktop) -> Self {
        let mut style = Self::default();
        if p.0.is_empty()
            && s.0.is_empty()
            && desktop.background.is_none()
            && desktop.foreground.is_none()
            && desktop.accent.is_none()
            && desktop.selection.is_none()
            && desktop.font.is_none()
            && desktop.radius.is_none()
            && desktop.border_width.is_none()
            && desktop.border.is_none()
        {
            return style;
        }
        let pc = |keys: &[&str]| keys.iter().find_map(|k| p.color(k, &p));
        let sc = |key: &str| s.color(key, &p);
        let omarchy =
            pc(&["background", "foreground"]).is_some() || sc("menu.background").is_some();
        style.background = opacity(
            sc("menu.background")
                .or_else(|| pc(&["background"]))
                .or(desktop.background)
                .unwrap_or(style.background),
            s.alpha("menu.background-alpha", 1.0),
        );
        style.foreground = sc("menu.text")
            .or_else(|| pc(&["foreground", "bright_foreground"]))
            .or(desktop.foreground)
            .unwrap_or(style.foreground);
        style.accent = pc(&["accent", "blue"])
            .or(desktop.border)
            .or(desktop.accent)
            .unwrap_or(style.accent);
        style.secondary = pc(&["dark_foreground"])
            .unwrap_or_else(|| mix(style.background, style.foreground, 0.65));
        style.darker_background = pc(&["darker_background"])
            .unwrap_or_else(|| mix(style.background, style.foreground, 0.03));
        style.lighter_background = pc(&["lighter_background"])
            .unwrap_or_else(|| mix(style.background, style.foreground, 0.08));
        style.muted = pc(&["muted"]).unwrap_or(style.secondary);
        style.red = pc(&["red"]).unwrap_or(style.accent);
        style.scrim = opacity(
            sc("menu.scrim")
                .or_else(|| pc(&["background"]))
                .unwrap_or(style.background.with_alpha(1.0)),
            s.alpha("menu.scrim-alpha", 0.5),
        );
        style.border = opacity(
            sc("menu.border")
                .or(desktop.border)
                .unwrap_or(style.foreground),
            s.alpha("menu.border-alpha", 1.0),
        );
        style.border_width = s
            .number("menu.border-width")
            .or(desktop.border_width)
            .unwrap_or(style.border_width);
        style.radius = desktop
            .radius
            .unwrap_or(if omarchy { 0.0 } else { style.radius });
        if omarchy {
            style.font = "monospace".into();
        } else if let Some((family, size)) = desktop.font {
            style.font = family;
            style.body = size.max(1.0);
        }
        let base = s.number("font.base-size").unwrap_or(style.body).max(1.0);
        style.body = s.number("font.body").unwrap_or(base).max(1.0);
        style.caption = s
            .number("font.caption")
            .unwrap_or((base * 0.833).round())
            .max(1.0);
        style.heading = s
            .number("font.heading")
            .unwrap_or((base * 1.333).round())
            .max(1.0);
        let scale = s.number("spacing.scale").unwrap_or(1.0)
            * if s.value("spacing.scale-with-font") == Some("false") {
                1.0
            } else {
                base / 12.0
            };
        style.padding_x = s
            .number("spacing.control-padding-x")
            .unwrap_or(10.0 * scale);
        style.padding_y = s.number("spacing.input-padding-y").unwrap_or(7.0 * scale);
        style.row_gap = s.number("spacing.row-gap").unwrap_or(8.0 * scale);
        style.row_padding_x = s.number("spacing.row-padding-x").unwrap_or(12.0 * scale);
        let state_color = |value: &str| match value {
            "foreground" | "text" => Some(style.foreground),
            "accent" => Some(style.accent),
            "urgent" => Some(style.red),
            _ => s.resolve_color(value, &p),
        };
        for (index, state) in ["normal", "hover-cursor", "focus", "selected"]
            .iter()
            .enumerate()
        {
            let key = |suffix: &str| format!("controls.{state}-{suffix}");
            let inherited = |suffix: &str| {
                s.value(&key(suffix)).or_else(|| {
                    if index == 2 {
                        s.value(&format!("controls.hover-cursor-{suffix}"))
                    } else {
                        None
                    }
                })
            };
            let color = inherited("color")
                .and_then(state_color)
                .unwrap_or(style.foreground);
            let border = inherited("border").and_then(state_color).unwrap_or(color);
            let number = |suffix: &str, fallback: f64| {
                inherited(suffix)
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .unwrap_or(fallback)
            };
            let control = ControlStyle {
                fill: opacity(
                    color,
                    number("fill-alpha", [0.04, 0.08, 0.08, 0.18][index]).clamp(0.0, 1.0),
                ),
                border: opacity(
                    border,
                    number("border-alpha", [0.4, 0.25, 0.25, 1.0][index]).clamp(0.0, 1.0),
                ),
                width: number(
                    "border-width",
                    if index == 3 {
                        0.0
                    } else {
                        style.controls[if index == 2 { 1 } else { 0 }].width
                    },
                ),
            };
            if index == 3 {
                style.selected = control;
            } else {
                style.controls[index] = control;
            }
        }
        style.selection = if omarchy {
            opacity(
                sc("controls.selection-color").unwrap_or(style.foreground),
                s.alpha("controls.selection-fill-alpha", 0.35),
            )
        } else {
            desktop.selection.unwrap_or(style.selection)
        };
        style.selected_text = if omarchy {
            style.foreground
        } else {
            desktop.selected_text.unwrap_or(style.foreground)
        };
        style.row_hover = ControlStyle {
            fill: opacity(
                sc("menu.selected-background").unwrap_or(style.foreground),
                s.alpha("menu.selected-background-alpha", 0.08),
            ),
            border: opacity(
                sc("menu.selected-border").unwrap_or(style.foreground),
                s.alpha("menu.selected-border-alpha", 0.25),
            ),
            width: s.number("menu.selected-border-width").unwrap_or(1.0),
        };
        style.row_hover_text = sc("menu.selected-text").unwrap_or(style.accent);
        style
    }

    #[cfg(test)]
    pub fn from_theme(colors: &str, shell: &str) -> Self {
        Self::resolve(
            Values::parse(colors),
            Values::parse(shell),
            Desktop::default(),
        )
    }

    pub fn panel(&self) -> ControlStyle {
        ControlStyle {
            fill: self.background,
            border: self.border,
            width: self.border_width,
        }
    }

    pub fn entry_height(&self) -> f64 {
        (self.body * 1.5 + self.padding_y * 2.0 + self.control_width() * 2.0).ceil()
    }
    pub fn control_width(&self) -> f64 {
        self.controls.iter().map(|c| c.width).fold(0.0, f64::max)
    }
    pub fn row_height(&self) -> f64 {
        (self.caption * 1.5 + self.body * 1.5 + self.padding_y * 2.0).ceil()
    }
    pub fn css(&self) -> String {
        let color = |c: Color| {
            format!(
                "rgba({:.0},{:.0},{:.0},{:.4})",
                c.red * 255.0,
                c.green * 255.0,
                c.blue * 255.0,
                c.alpha
            )
        };
        let font = self
            .font
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace(['\n', '\r'], " ");
        let selector = "window.keywrld-overlay entry.shortcut-search";
        let mut css = format!(
            "window.keywrld-overlay {{ background: transparent; }}\n{selector} {{ color: {}; caret-color: {}; font-family: \"{font}\"; font-size: {}px; border-radius: {}px; padding: {}px {}px; min-height: 0; min-width: 0; box-shadow: none; outline: none; }}\n",
            color(self.foreground),
            color(self.accent),
            self.body,
            self.radius,
            self.padding_y,
            self.padding_x
        );
        for (pseudo, control) in ["", ":hover", ":focus-within"].iter().zip(self.controls) {
            css.push_str(&format!(
                "{selector}{pseudo} {{ background: {}; border: {}px solid {}; }}\n",
                color(control.fill),
                control.width,
                color(control.border)
            ));
        }
        css.push_str(&format!("{selector} text placeholder {{ color: {}; opacity: 1; }}\n{selector} image {{ color: {}; }}\n{selector} image:hover {{ color: {}; }}\n{selector} selection {{ background: {}; color: {}; }}",color(self.secondary),color(self.secondary),color(self.accent),color(self.selection),color(self.selected_text)));
        css
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resolve(colors: &str, shell: &str, desktop: Desktop) -> Style {
        Style::resolve(Values::parse(colors), Values::parse(shell), desktop)
    }
    #[test]
    fn opening_rereads_theme_files_and_missing_files_use_defaults() {
        let directory = std::env::temp_dir().join(format!(
            "keywrld-style-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let read = || Style::read_theme(Some(&directory), None, Desktop::default());
        assert_eq!(read().background, Style::default().background);
        fs::write(directory.join("colors.toml"), "foreground = '#112233'").unwrap();
        assert_eq!(read().foreground, Color::rgb(17, 34, 51));
        fs::write(directory.join("colors.toml"), "foreground = '#abcdef'").unwrap();
        assert_eq!(read().foreground, Color::rgb(171, 205, 239));
        fs::write(
            directory.join("shell.toml"),
            "[font]\nbase-size = 12\n[menu]\ntext = '#223344'",
        )
        .unwrap();
        let user = directory.join("user.toml");
        fs::write(&user, "[font]\nbase-size = 20\n[menu]\nborder-width = 3").unwrap();
        let style = Style::read_theme(Some(&directory), Some(&user), Desktop::default());
        assert_eq!(style.body, 20.0);
        assert_eq!(style.border_width, 3.0);
        assert_eq!(style.foreground, Color::rgb(34, 51, 68));
        fs::write(&user, "[font]\nbase-size = 16").unwrap();
        let style = Style::read_theme(Some(&directory), Some(&user), Desktop::default());
        assert_eq!(style.body, 16.0);
        assert_eq!(style.border_width, Style::default().border_width);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn canonical_colors_and_shell_override_are_order_independent() {
        for colors in [
            "foreground = '#123456'\nbright_foreground = '#ffffff'",
            "bright_foreground = '#ffffff'\nforeground = '#123456'",
        ] {
            let style = resolve(colors, "", Desktop::default());
            assert_eq!(style.foreground, Color::rgb(18, 52, 86));
            assert_eq!(style.font, "monospace");
            let style = resolve(
                colors,
                "[menu]\ntext = '#abcdef' # comment",
                Desktop::default(),
            );
            assert_eq!(style.foreground, Color::rgb(171, 205, 239));
        }
    }
    #[test]
    fn partial_invalid_and_missing_sources_fall_through() {
        let style = resolve(
            "background = 'invalid'\nforeground = '#010203'",
            "[font]\nbase-size = nan\n[menu]\nborder-width = -2",
            Desktop {
                background: Some(Color::rgb(240, 240, 240)),
                border_width: Some(2.0),
                radius: Some(0.0),
                ..Desktop::default()
            },
        );
        assert_eq!(style.background, Color::rgb(240, 240, 240));
        assert_eq!(style.border_width, 2.0);
        assert_eq!(style.radius, 0.0);
        assert_eq!(style.body, 12.0);
        let style = resolve(
            "",
            "",
            Desktop {
                font: Some(("Example Sans".into(), 16.0)),
                ..Desktop::default()
            },
        );
        assert_eq!(style.body, 16.0);
        assert_eq!(style.font, "Example Sans");
        let style = resolve("", "", Desktop::default());
        assert_eq!(style.background, Style::default().background);
    }
    #[test]
    fn controls_share_surface_roles_and_focus_inherits_hover_without_compounding_alpha() {
        let style = Style::from_theme(
            "foreground = '#123456'",
            "[menu]\ntext = '#abcdef'\n[controls]\nhover-cursor-color = 'rgba(11223380)'\nhover-cursor-fill-alpha = 0.4\nhover-cursor-border-width = 3\nselected-color = 'foreground'\nselected-fill-alpha = 0.6\nselected-border-width = 0",
        );
        assert_eq!(style.controls[1].fill, style.controls[2].fill);
        assert!((style.controls[2].fill.alpha - 128.0 / 255.0 * 0.4).abs() < 1e-9);
        assert_eq!(style.controls[2].width, 3.0);
        assert_eq!(style.selected.fill, style.foreground.with_alpha(0.6));
        assert_eq!(style.selected.width, 0.0);
    }

    #[test]
    #[ignore = "requires Omarchy's shared semantic color resolver"]
    fn omarchy_resolves_legacy_palettes_and_derives_missing_shades() {
        let directory =
            std::env::temp_dir().join(format!("keywrld-resolver-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("colors.toml"), "bg = '#eeeeee'\nfg = '#222222'\ncolor4 = '#123456'\nred = '#ff0000'\nyellow = '#ffff00'").unwrap();
        let style = Style::resolve(
            resolved_colors(&directory).expect("Omarchy color resolver"),
            Values::default(),
            Desktop::default(),
        );
        assert_eq!(style.background, Color::rgb(238, 238, 238));
        assert_eq!(style.foreground, Color::rgb(34, 34, 34));
        assert_eq!(style.accent, Color::rgb(18, 52, 86));
        assert_eq!(style.darker_background, Color::rgb(119, 119, 119));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn shell_references_alpha_and_hyprland_gradient() {
        let style = resolve(
            "foreground = '#abcdef'",
            "[hyprland]\nactive-border = '#112233'\n[menu]\nborder = 'hyprland.active-border'\nbackground-alpha = 0.8\n[controls]\nfocus-border-width = 2\n[font]\nbase-size = 18",
            Desktop {
                border: parse_color("ff778899 ff000000 45deg"),
                ..Desktop::default()
            },
        );
        assert_eq!(style.border, Color::rgb(17, 34, 51));
        assert_eq!(style.background.alpha, 0.8);
        assert_eq!(style.controls[2].width, 2.0);
        assert!(style.entry_height() > 36.0);
        assert_eq!(
            parse_color("ff778899 ff000000 45deg"),
            Some(Color::rgb(119, 136, 153))
        );
        assert!(parse_color("broken").is_none());
        let cycle = Values::parse("a = 'b'\nb = 'a'");
        assert!(cycle.color("a", &cycle).is_none());
    }
}
