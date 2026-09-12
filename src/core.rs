use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const SHIFT: Self = Self(1 << 0);
    pub const CTRL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const SUPER: Self = Self(1 << 3);
    pub const CAPS_LOCK: Self = Self(1 << 4);
    pub const MOD2: Self = Self(1 << 5);
    pub const MOD3: Self = Self(1 << 6);
    pub const MOD5: Self = Self(1 << 7);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn toggled(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }

    pub fn from_hyprland_mask(mask: u16) -> Self {
        let mut modifiers = Self::default();
        for (hyprland_bit, modifier) in [
            (1, Self::SHIFT),
            (2, Self::CAPS_LOCK),
            (4, Self::CTRL),
            (8, Self::ALT),
            (16, Self::MOD2),
            (32, Self::MOD3),
            (64, Self::SUPER),
            (128, Self::MOD5),
        ] {
            if mask & hyprland_bit != 0 {
                modifiers = modifiers.with(modifier);
            }
        }
        modifiers
    }

    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.contains(Self::SUPER) {
            parts.push("SUPER");
        }
        if self.contains(Self::SHIFT) {
            parts.push("SHIFT");
        }
        if self.contains(Self::CTRL) {
            parts.push("CTRL");
        }
        if self.contains(Self::ALT) {
            parts.push("ALT");
        }
        if self.contains(Self::CAPS_LOCK) {
            parts.push("CAPS");
        }
        if self.contains(Self::MOD2) {
            parts.push("MOD2");
        }
        if self.contains(Self::MOD3) {
            parts.push("MOD3");
        }
        if self.contains(Self::MOD5) {
            parts.push("MOD5");
        }
        parts.join(" + ")
    }

    pub(crate) fn from_words(words: &str) -> Self {
        words
            .split_whitespace()
            .fold(Self::default(), |mods, word| {
                match word.to_ascii_uppercase().as_str() {
                    "SUPER" | "META" | "MOD4" => mods.with(Self::SUPER),
                    "SHIFT" => mods.with(Self::SHIFT),
                    "CTRL" | "CONTROL" => mods.with(Self::CTRL),
                    "ALT" | "MOD1" => mods.with(Self::ALT),
                    "CAPS" | "CAPSLOCK" | "CAPS_LOCK" => mods.with(Self::CAPS_LOCK),
                    "MOD2" => mods.with(Self::MOD2),
                    "MOD3" => mods.with(Self::MOD3),
                    "MOD5" => mods.with(Self::MOD5),
                    _ => mods,
                }
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub modifiers: Modifiers,
    pub key: String,
    pub action: String,
}

pub fn parse_bindings(output: &str) -> Vec<Binding> {
    output.lines().filter_map(parse_binding_line).collect()
}

fn parse_binding_line(line: &str) -> Option<Binding> {
    let (combo, action) = line.split_once('→')?;
    let combo = combo.trim();
    let action = action.trim();
    if combo.is_empty() || action.is_empty() {
        return None;
    }

    let (modifier_words, key) = combo
        .rsplit_once(" + ")
        .map_or(("", combo), |(mods, key)| (mods, key));

    Some(Binding {
        modifiers: Modifiers::from_words(modifier_words),
        key: normalize_key(key),
        action: action.to_string(),
    })
}

pub fn actions_for(bindings: &[Binding], modifiers: Modifiers) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for binding in bindings {
        if binding.modifiers == modifiers {
            let actions = grouped.entry(binding.key.clone()).or_default();
            if !actions.contains(&binding.action) {
                actions.push(binding.action.clone());
            }
        }
    }
    grouped
}

pub fn normalize_key(key: &str) -> String {
    // Supplemented bindings already carry canonical IDs. In Hyprland's raw
    // spelling, lowercase mouse_left/mouse_right mean horizontal scrolling.
    if is_mouse_key(key) {
        return key.to_string();
    }
    let compact = key.trim().replace(['_', '-'], " ");
    match compact.to_ascii_uppercase().as_str() {
        "ESC" | "ESCAPE" => "ESCAPE".into(),
        "ENTER" | "RETURN" | "KP ENTER" => "RETURN".into(),
        "DEL" | "DELETE" => "DELETE".into(),
        "INS" | "INSERT" => "INSERT".into(),
        "PGUP" | "PAGE UP" | "PAGEUP" => "PAGEUP".into(),
        "PGDN" | "PAGE DOWN" | "PAGEDOWN" => "PAGEDOWN".into(),
        "SPACEBAR" | "SPACE" => "SPACE".into(),
        "COMMA" => "COMMA".into(),
        "PERIOD" | "DOT" => "PERIOD".into(),
        "MINUS" => "MINUS".into(),
        "EQUAL" | "EQUALS" => "EQUAL".into(),
        "SLASH" => "SLASH".into(),
        "BACKSLASH" => "BACKSLASH".into(),
        "BRACKETLEFT" | "LEFT BRACKET" => "BRACKETLEFT".into(),
        "BRACKETRIGHT" | "RIGHT BRACKET" => "BRACKETRIGHT".into(),
        "SEMICOLON" => "SEMICOLON".into(),
        "APOSTROPHE" | "QUOTE" => "APOSTROPHE".into(),
        "GRAVE" | "GRAVE ACCENT" => "GRAVE".into(),
        "MOUSE LEFT" => "MOUSE_SCROLL_LEFT".into(),
        "MOUSE RIGHT" => "MOUSE_SCROLL_RIGHT".into(),
        "MOUSE DOWN" => "MOUSE_DOWN".into(),
        "MOUSE UP" => "MOUSE_UP".into(),
        "MOUSE:272" | "LEFT MOUSE BUTTON" => "MOUSE_LEFT".into(),
        "MOUSE:273" | "RIGHT MOUSE BUTTON" => "MOUSE_RIGHT".into(),
        "MOUSE:274" | "MIDDLE MOUSE BUTTON" => "MOUSE_MIDDLE".into(),
        "CODE:10" => "1".into(),
        "CODE:11" => "2".into(),
        "CODE:12" => "3".into(),
        "CODE:13" => "4".into(),
        "CODE:14" => "5".into(),
        "CODE:15" => "6".into(),
        "CODE:16" => "7".into(),
        "CODE:17" => "8".into(),
        "CODE:18" => "9".into(),
        "CODE:19" => "0".into(),
        "CODE:20" => "MINUS".into(),
        "CODE:21" => "EQUAL".into(),
        "CODE:59" => "COMMA".into(),
        "CODE:60" => "PERIOD".into(),
        "CODE:61" => "SLASH".into(),
        value => value.to_string(),
    }
}

pub fn is_mouse_key(key: &str) -> bool {
    matches!(
        key,
        "MOUSE_DOWN" | "MOUSE_UP" | "MOUSE_LEFT" | "MOUSE_RIGHT" | "MOUSE_MIDDLE" | "MOUSE_SCROLL_LEFT" | "MOUSE_SCROLL_RIGHT"
    ) || key.starts_with("MOUSE:")
}

pub fn chord_label(modifiers: Modifiers, key: &str) -> String {
    if modifiers.is_empty() {
        key.to_string()
    } else {
        format!("{} + {key}", modifiers.label())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyboardLayout {
    Full,
    Tkl,
    Compact75,
    Compact65,
    Compact60,
}

impl KeyboardLayout {
    pub fn parse_override(value: &str) -> Result<Option<Self>, String> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(None),
            "full" | "100" | "100%" => Ok(Some(Self::Full)),
            "tkl" | "80" | "80%" => Ok(Some(Self::Tkl)),
            "75" | "75%" => Ok(Some(Self::Compact75)),
            "65" | "65%" => Ok(Some(Self::Compact65)),
            "60" | "60%" => Ok(Some(Self::Compact60)),
            _ => Err(format!(
                "invalid layout '{value}'; expected auto, full, 100, tkl, 80, 75, 65, or 60"
            )),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "Full size",
            Self::Tkl => "TKL · 80%",
            Self::Compact75 => "75%",
            Self::Compact65 => "65%",
            Self::Compact60 => "60%",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeviceCandidate {
    pub name: String,
    pub vendor: Option<u16>,
    pub product: Option<u16>,
    pub key_codes: BTreeSet<u16>,
    pub virtual_like: bool,
}

pub fn detect_keyboard_layout(
    layout_override: Option<KeyboardLayout>,
    devices: &[DeviceCandidate],
) -> KeyboardLayout {
    if let Some(layout) = layout_override {
        return layout;
    }

    let physical: Vec<_> = devices
        .iter()
        .filter(|device| !device.virtual_like)
        .collect();

    for device in &physical {
        if device.vendor == Some(0x1b1c) && device.product == Some(0x1b45) {
            return KeyboardLayout::Tkl;
        }
    }

    for device in &physical {
        let name = device.name.to_ascii_lowercase();
        if name.contains("tenkeyless") || name.contains(" tkl") {
            return KeyboardLayout::Tkl;
        }
        if has_form_factor_hint(&name, "75") {
            return KeyboardLayout::Compact75;
        }
        if has_form_factor_hint(&name, "65") {
            return KeyboardLayout::Compact65;
        }
        if has_form_factor_hint(&name, "60") {
            return KeyboardLayout::Compact60;
        }
        if name.contains("full size")
            || name.contains("full-size")
            || has_form_factor_hint(&name, "100")
        {
            return KeyboardLayout::Full;
        }
    }

    let Some(device) = physical.iter().max_by_key(|device| device.key_codes.len()) else {
        return KeyboardLayout::Tkl;
    };

    let has_numpad = [71, 72, 73, 75, 76, 77, 79, 80, 81, 82, 83, 96]
        .iter()
        .all(|code| device.key_codes.contains(code));
    let has_function_row = [59, 60, 61, 62, 63, 64, 65, 66, 67, 68]
        .iter()
        .all(|code| device.key_codes.contains(code));
    let has_arrows = [103, 105, 106, 108]
        .iter()
        .all(|code| device.key_codes.contains(code));
    let has_navigation = [102, 104, 107, 109, 110, 111]
        .iter()
        .all(|code| device.key_codes.contains(code));

    match (has_numpad, has_function_row, has_navigation, has_arrows) {
        (true, _, _, _) => KeyboardLayout::Full,
        (false, true, true, _) => KeyboardLayout::Tkl,
        (false, true, false, true) => KeyboardLayout::Compact75,
        (false, false, _, true) => KeyboardLayout::Compact65,
        _ => KeyboardLayout::Compact60,
    }
}

fn has_form_factor_hint(name: &str, number: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|word| word == number || word == format!("{number}percent"))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x <= self.x + self.width && y <= self.y + self.height
    }
}

#[derive(Clone, Debug)]
pub struct KeySpec {
    pub id: String,
    pub legend: String,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct KeyboardGeometry {
    pub keys: Vec<KeySpec>,
    pub width: f64,
    pub height: f64,
}

impl KeyboardGeometry {
    pub fn new(layout: KeyboardLayout) -> Self {
        build_geometry(layout)
    }

    pub fn key_ids(&self) -> BTreeSet<&str> {
        self.keys.iter().map(|key| key.id.as_str()).collect()
    }
}

fn key(id: &str, legend: &str, x: f64, y: f64, width: f64) -> KeySpec {
    KeySpec {
        id: id.into(),
        legend: legend.into(),
        rect: Rect {
            x,
            y,
            width,
            height: 1.0,
        },
    }
}

fn build_geometry(layout: KeyboardLayout) -> KeyboardGeometry {
    let mut keys = Vec::new();
    let has_function_row =
        layout != KeyboardLayout::Compact60 && layout != KeyboardLayout::Compact65;
    let main_y = if has_function_row { 1.45 } else { 0.0 };

    if has_function_row {
        keys.push(key("ESCAPE", "Esc", 0.0, 0.0, 1.0));
        let function_positions = match layout {
            KeyboardLayout::Tkl | KeyboardLayout::Full => [
                1.75, 2.75, 3.75, 4.75, 6.0, 7.0, 8.0, 9.0, 10.25, 11.25, 12.25, 13.25,
            ],
            _ => [
                1.25, 2.25, 3.25, 4.25, 5.25, 6.25, 7.25, 8.25, 9.25, 10.25, 11.25, 12.25,
            ],
        };
        for (index, x) in function_positions.into_iter().enumerate() {
            let name = format!("F{}", index + 1);
            keys.push(key(&name, &name, x, 0.0, 1.0));
        }
        if matches!(layout, KeyboardLayout::Tkl | KeyboardLayout::Full) {
            keys.push(key("PRINT", "Prt", 15.0, 0.0, 1.0));
            keys.push(key("SCROLLLOCK", "Scr", 16.0, 0.0, 1.0));
            keys.push(key("PAUSE", "Pause", 17.0, 0.0, 1.0));
        }
    }

    let number_row = [
        ("GRAVE", "`"),
        ("1", "1"),
        ("2", "2"),
        ("3", "3"),
        ("4", "4"),
        ("5", "5"),
        ("6", "6"),
        ("7", "7"),
        ("8", "8"),
        ("9", "9"),
        ("0", "0"),
        ("MINUS", "-"),
        ("EQUAL", "="),
    ];
    for (index, (id, legend)) in number_row.into_iter().enumerate() {
        keys.push(key(id, legend, index as f64, main_y, 1.0));
    }
    keys.push(key("BACKSPACE", "Backspace", 13.0, main_y, 2.0));

    let q_row = [
        ("Q", "Q"),
        ("W", "W"),
        ("E", "E"),
        ("R", "R"),
        ("T", "T"),
        ("Y", "Y"),
        ("U", "U"),
        ("I", "I"),
        ("O", "O"),
        ("P", "P"),
        ("BRACKETLEFT", "["),
        ("BRACKETRIGHT", "]"),
    ];
    keys.push(key("TAB", "Tab", 0.0, main_y + 1.0, 1.5));
    for (index, (id, legend)) in q_row.into_iter().enumerate() {
        keys.push(key(id, legend, 1.5 + index as f64, main_y + 1.0, 1.0));
    }
    keys.push(key("BACKSLASH", "\\", 13.5, main_y + 1.0, 1.5));

    let a_row = [
        ("A", "A"),
        ("S", "S"),
        ("D", "D"),
        ("F", "F"),
        ("G", "G"),
        ("H", "H"),
        ("J", "J"),
        ("K", "K"),
        ("L", "L"),
        ("SEMICOLON", ";"),
        ("APOSTROPHE", "'"),
    ];
    keys.push(key("CAPSLOCK", "Caps", 0.0, main_y + 2.0, 1.75));
    for (index, (id, legend)) in a_row.into_iter().enumerate() {
        keys.push(key(id, legend, 1.75 + index as f64, main_y + 2.0, 1.0));
    }
    keys.push(key("RETURN", "Enter", 12.75, main_y + 2.0, 2.25));

    let z_row = [
        ("Z", "Z"),
        ("X", "X"),
        ("C", "C"),
        ("V", "V"),
        ("B", "B"),
        ("N", "N"),
        ("M", "M"),
        ("COMMA", ","),
        ("PERIOD", "."),
        ("SLASH", "/"),
    ];
    keys.push(key("SHIFTLEFT", "Shift", 0.0, main_y + 3.0, 2.25));
    for (index, (id, legend)) in z_row.into_iter().enumerate() {
        keys.push(key(id, legend, 2.25 + index as f64, main_y + 3.0, 1.0));
    }
    keys.push(key("SHIFTRIGHT", "Shift", 12.25, main_y + 3.0, 2.75));

    keys.push(key("CTRLLEFT", "Ctrl", 0.0, main_y + 4.0, 1.25));
    keys.push(key("SUPERLEFT", "Super", 1.25, main_y + 4.0, 1.25));
    keys.push(key("ALTLEFT", "Alt", 2.5, main_y + 4.0, 1.25));
    keys.push(key("SPACE", "Space", 3.75, main_y + 4.0, 6.25));
    keys.push(key("ALTRIGHT", "Alt", 10.0, main_y + 4.0, 1.25));
    keys.push(key("SUPERRIGHT", "Super", 11.25, main_y + 4.0, 1.25));
    keys.push(key("MENU", "Menu", 12.5, main_y + 4.0, 1.25));
    keys.push(key("CTRLRIGHT", "Ctrl", 13.75, main_y + 4.0, 1.25));

    match layout {
        KeyboardLayout::Tkl | KeyboardLayout::Full => {
            let nav_x = 15.5;
            keys.push(key("INSERT", "Ins", nav_x, main_y, 1.0));
            keys.push(key("HOME", "Home", nav_x + 1.0, main_y, 1.0));
            keys.push(key("PAGEUP", "PgUp", nav_x + 2.0, main_y, 1.0));
            keys.push(key("DELETE", "Del", nav_x, main_y + 1.0, 1.0));
            keys.push(key("END", "End", nav_x + 1.0, main_y + 1.0, 1.0));
            keys.push(key("PAGEDOWN", "PgDn", nav_x + 2.0, main_y + 1.0, 1.0));
            keys.push(key("UP", "↑", nav_x + 1.0, main_y + 3.0, 1.0));
            keys.push(key("LEFT", "←", nav_x, main_y + 4.0, 1.0));
            keys.push(key("DOWN", "↓", nav_x + 1.0, main_y + 4.0, 1.0));
            keys.push(key("RIGHT", "→", nav_x + 2.0, main_y + 4.0, 1.0));
        }
        KeyboardLayout::Compact75 => {
            let x = 15.1;
            keys.push(key("HOME", "Home", x, main_y, 1.0));
            keys.push(key("PAGEUP", "PgUp", x, main_y + 1.0, 1.0));
            keys.push(key("PAGEDOWN", "PgDn", x, main_y + 2.0, 1.0));
            keys.push(key("UP", "↑", 13.0, main_y + 3.0, 1.0));
            keys.push(key("LEFT", "←", 12.0, main_y + 4.0, 1.0));
            keys.push(key("DOWN", "↓", 13.0, main_y + 4.0, 1.0));
            keys.push(key("RIGHT", "→", 14.0, main_y + 4.0, 1.0));
        }
        KeyboardLayout::Compact65 => {
            let x = 15.1;
            keys.push(key("DELETE", "Del", x, main_y, 1.0));
            keys.push(key("PAGEUP", "PgUp", x, main_y + 1.0, 1.0));
            keys.push(key("PAGEDOWN", "PgDn", x, main_y + 2.0, 1.0));
            keys.push(key("UP", "↑", 13.0, main_y + 3.0, 1.0));
            keys.push(key("LEFT", "←", 12.0, main_y + 4.0, 1.0));
            keys.push(key("DOWN", "↓", 13.0, main_y + 4.0, 1.0));
            keys.push(key("RIGHT", "→", 14.0, main_y + 4.0, 1.0));
        }
        KeyboardLayout::Compact60 => {}
    }

    let mut width: f64 = keys
        .iter()
        .map(|item| item.rect.x + item.rect.width)
        .fold(0.0, f64::max);

    if layout == KeyboardLayout::Full {
        let x = 19.0;
        let y = main_y;
        for (id, legend, column, row) in [
            ("NUMLOCK", "Num", 0.0, 0.0),
            ("KPSLASH", "/", 1.0, 0.0),
            ("KPASTERISK", "*", 2.0, 0.0),
            ("KPMINUS", "-", 3.0, 0.0),
            ("KP7", "7", 0.0, 1.0),
            ("KP8", "8", 1.0, 1.0),
            ("KP9", "9", 2.0, 1.0),
            ("KP4", "4", 0.0, 2.0),
            ("KP5", "5", 1.0, 2.0),
            ("KP6", "6", 2.0, 2.0),
            ("KP1", "1", 0.0, 3.0),
            ("KP2", "2", 1.0, 3.0),
            ("KP3", "3", 2.0, 3.0),
            ("KP0", "0", 0.0, 4.0),
            ("KPDOT", ".", 2.0, 4.0),
        ] {
            keys.push(key(
                id,
                legend,
                x + column,
                y + row,
                if id == "KP0" { 2.0 } else { 1.0 },
            ));
        }
        keys.push(KeySpec {
            id: "KPPLUS".into(),
            legend: "+".into(),
            rect: Rect {
                x: x + 3.0,
                y: y + 1.0,
                width: 1.0,
                height: 2.0,
            },
        });
        keys.push(KeySpec {
            id: "KPENTER".into(),
            legend: "Enter".into(),
            rect: Rect {
                x: x + 3.0,
                y: y + 3.0,
                width: 1.0,
                height: 2.0,
            },
        });
        width = 23.0;
    }

    KeyboardGeometry {
        keys,
        width,
        height: main_y + 5.0,
    }
}

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

    fn from_hex(value: &str) -> Option<Self> {
        let value = value.trim().trim_matches('"').strip_prefix('#')?;
        if value.len() != 6 {
            return None;
        }
        let red = u8::from_str_radix(&value[0..2], 16).ok()?;
        let green = u8::from_str_radix(&value[2..4], 16).ok()?;
        let blue = u8::from_str_radix(&value[4..6], 16).ok()?;
        Some(Self::rgb(red, green, blue))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Color,
    pub darker_background: Color,
    pub lighter_background: Color,
    pub foreground: Color,
    pub dark_foreground: Color,
    pub accent: Color,
    pub selection: Color,
    pub muted: Color,
    pub red: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: Color::rgb(26, 27, 38),
            darker_background: Color::rgb(14, 14, 20),
            lighter_background: Color::rgb(36, 40, 59),
            foreground: Color::rgb(192, 202, 245),
            dark_foreground: Color::rgb(86, 95, 137),
            accent: Color::rgb(122, 162, 247),
            selection: Color::rgb(41, 46, 66),
            muted: Color::rgb(65, 72, 104),
            red: Color::rgb(247, 118, 142),
        }
    }
}

impl Palette {
    pub fn from_toml(input: &str) -> Self {
        let mut palette = Self::default();
        for line in input.lines() {
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let Some(color) = Color::from_hex(value) else {
                continue;
            };
            match name.trim() {
                "background" => palette.background = color,
                "darker_background" => palette.darker_background = color,
                "lighter_background" => palette.lighter_background = color,
                "foreground" | "bright_foreground" => palette.foreground = color,
                "dark_foreground" => palette.dark_foreground = color,
                "accent" => palette.accent = color,
                "selection" => palette.selection = color,
                "muted" => palette.muted = color,
                "red" => palette.red = color,
                _ => {}
            }
        }
        palette
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_omarchy_lines_and_normalizes_modifiers() {
        let bindings = parse_bindings(
            "SUPER SHIFT + RETURN                → Browser\n\
             SUPER + comma                       → Dismiss notification\n\
             PRINT                               → Screenshot\n",
        );

        assert_eq!(bindings.len(), 3);
        assert_eq!(
            bindings[0].modifiers,
            Modifiers::SUPER.with(Modifiers::SHIFT)
        );
        assert_eq!(bindings[0].key, "RETURN");
        assert_eq!(bindings[1].key, "COMMA");
        assert_eq!(bindings[2].modifiers, Modifiers::default());
    }

    #[test]
    fn selects_an_exact_super_chord_and_deduplicates_actions() {
        let bindings = parse_bindings(
            "SUPER + K → Keybindings\n\
             SUPER + K → Keybindings\n\
             SUPER SHIFT + K → Other\n\
             ALT + K → Not shown\n",
        );

        let actions = actions_for(&bindings, Modifiers::SUPER);
        assert_eq!(actions.get("K"), Some(&vec!["Keybindings".to_string()]));
        assert_eq!(actions.len(), 1);

        let alt_actions = actions_for(&bindings, Modifiers::ALT);
        assert_eq!(alt_actions.get("K"), Some(&vec!["Not shown".to_string()]));
    }

    #[test]
    fn maps_hyprland_masks_and_normalizes_pointing_device_keys() {
        assert_eq!(
            Modifiers::from_hyprland_mask(64 | 1 | 8),
            Modifiers::SUPER.with(Modifiers::SHIFT).with(Modifiers::ALT)
        );
        assert_eq!(normalize_key("mouse:272"), "MOUSE_LEFT");
        assert_eq!(normalize_key("code:10"), "1");
        assert!(is_mouse_key("MOUSE_UP"));
        assert_eq!(normalize_key("mouse_left"), "MOUSE_SCROLL_LEFT");
        assert_eq!(normalize_key("mouse_right"), "MOUSE_SCROLL_RIGHT");
        assert_eq!(normalize_key(&normalize_key("mouse:272")), "MOUSE_LEFT");
        assert_eq!(normalize_key(&normalize_key("mouse_left")), "MOUSE_SCROLL_LEFT");
        assert!(!is_mouse_key("UP"));
    }

    #[test]
    fn override_and_exact_device_profile_take_precedence() {
        let k63 = DeviceCandidate {
            name: "CORSAIR K63 Wireless Mechanical Gaming Keyboard".into(),
            vendor: Some(0x1b1c),
            product: Some(0x1b45),
            ..Default::default()
        };

        assert_eq!(
            detect_keyboard_layout(None, std::slice::from_ref(&k63)),
            KeyboardLayout::Tkl
        );
        assert_eq!(
            detect_keyboard_layout(Some(KeyboardLayout::Compact65), &[k63]),
            KeyboardLayout::Compact65
        );
    }

    #[test]
    fn name_hints_beat_capability_fallbacks() {
        let device = DeviceCandidate {
            name: "Example 75 percent keyboard".into(),
            key_codes: (1..120).collect(),
            ..Default::default()
        };
        assert_eq!(
            detect_keyboard_layout(None, &[device]),
            KeyboardLayout::Compact75
        );
    }

    #[test]
    fn geometry_hit_testing_respects_key_bounds() {
        let geometry = KeyboardGeometry::new(KeyboardLayout::Tkl);
        let hit = geometry.keys.iter().find(|key| key.rect.contains(8.0, 3.6));
        assert_eq!(hit.map(|key| key.id.as_str()), Some("J"));
        assert!(
            geometry
                .keys
                .iter()
                .all(|key| !key.rect.contains(100.0, 100.0))
        );
    }

    #[test]
    fn theme_parser_keeps_fallbacks_for_missing_colors() {
        let palette = Palette::from_toml("accent = \"#010203\"\nbackground = invalid");
        assert_eq!(palette.accent, Color::rgb(1, 2, 3));
        assert_eq!(palette.background, Palette::default().background);
    }
}
