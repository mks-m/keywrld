//! A deliberately non-executing reader for literal input declarations.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::{DiscoveryStatus, GestureBinding, GestureDiscovery};
use crate::core::Modifiers;
use crate::system::normalize_device_name;

#[derive(Default)]
pub(super) struct Config {
    pub devices: BTreeMap<String, BTreeMap<String, String>>,
    pub gestures: GestureDiscovery,
}

pub(super) fn load() -> Config {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Config::default();
    };
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"));
    let omarchy = std::env::var_os("OMARCHY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/share/omarchy"));
    let default = config_home.join("hypr");
    let root = active_config_path().unwrap_or_else(|| {
        let lua = default.join("hyprland.lua");
        if lua.exists() {
            lua
        } else {
            default.join("hyprland.conf")
        }
    });
    if !root.is_file() {
        return Config::default();
    }
    let mut reader = Reader {
        config: Config::default(),
        roots: vec![
            state_home,
            config_home,
            omarchy,
            root.parent().unwrap_or(Path::new(".")).to_path_buf(),
        ],
        stack: BTreeSet::new(),
        required: BTreeSet::new(),
        variables: BTreeMap::from([("HOME".into(), home.to_string_lossy().into_owned())]),
    };
    reader.read(&root, false);
    reader.config
}

fn active_config_path() -> Option<PathBuf> {
    // Honor Hyprland -c/--config without running or reloading any configuration.
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    let lock =
        fs::read_to_string(runtime.join("hypr").join(signature).join("hyprland.lock")).ok()?;
    let pid: u32 = lock.lines().next()?.parse().ok()?;
    let args = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<_> = args
        .split(|b| *b == 0)
        .map(String::from_utf8_lossy)
        .collect();
    for (i, arg) in args.iter().enumerate() {
        if matches!(arg.as_ref(), "-c" | "--config") {
            return args.get(i + 1).map(|s| PathBuf::from(s.as_ref()));
        }
        if let Some(path) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(path));
        }
    }
    None
}

struct Reader {
    config: Config,
    roots: Vec<PathBuf>,
    stack: BTreeSet<PathBuf>,
    required: BTreeSet<PathBuf>,
    variables: BTreeMap<String, String>,
}

impl Reader {
    fn incomplete(&mut self) {
        self.config.gestures.status = DiscoveryStatus::Incomplete;
    }

    fn read(&mut self, path: &Path, require: bool) {
        let Ok(path) = fs::canonicalize(path) else {
            self.incomplete();
            return;
        };
        if self.stack.len() >= 32 || self.stack.contains(&path) {
            self.incomplete();
            return;
        }
        if require && !self.required.insert(path.clone()) {
            return;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            self.incomplete();
            return;
        };
        if self.config.gestures.status == DiscoveryStatus::Unavailable {
            self.config.gestures.status = DiscoveryStatus::Complete;
        }
        self.stack.insert(path.clone());
        if path.extension().is_some_and(|e| e == "lua") {
            self.lua(&text, &path);
        } else {
            self.legacy(&text, &path);
        }
        self.stack.remove(&path);
    }

    fn include(&mut self, value: &str, path: &Path, require: bool) {
        if require {
            let relative = value.replace('.', "/");
            let found = self
                .roots
                .iter()
                .flat_map(|r| {
                    [
                        r.join(format!("{relative}.lua")),
                        r.join(&relative).join("init.lua"),
                    ]
                })
                .find(|p| p.is_file());
            if let Some(found) = found {
                self.read(&found, true);
            } else {
                self.incomplete();
            }
        } else {
            let value = self.expand(value);
            let include = if value.starts_with('/') {
                PathBuf::from(value)
            } else {
                path.parent().unwrap_or(Path::new(".")).join(value)
            };
            if include.to_string_lossy().contains(['*', '?']) {
                let mut matches = expand_glob(&include);
                matches.sort();
                if matches.is_empty() {
                    self.incomplete();
                }
                for include in matches {
                    self.read(&include, false);
                }
            } else {
                self.read(&include, false);
            }
        }
    }

    fn expand(&self, value: &str) -> String {
        let mut value = value.to_string();
        if value.starts_with("~/")
            && let Some(home) = self.variables.get("HOME")
        {
            value = format!("{home}/{}", &value[2..]);
        }
        // Longest names first prevents $foo from consuming the start of $foobar.
        let mut variables: Vec<_> = self.variables.iter().collect();
        variables.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        for (name, replacement) in variables {
            value = value.replace(&format!("${name}"), replacement);
        }
        value
    }

    fn lua(&mut self, text: &str, path: &Path) {
        let tokens = lex(text);
        let mut i = 0;
        let mut blocks = 0usize;
        let mut nesting = 0usize;
        while i < tokens.len() {
            let token = tokens[i].word();
            if matches!(token, "load" | "loadfile" | "loadstring" | "eval") {
                self.incomplete();
            }
            let call = if i + 3 < tokens.len()
                && token == "hl"
                && tokens[i + 1].word() == "."
                && tokens[i + 3].word() == "("
            {
                Some((tokens[i + 2].word(), i + 3))
            } else if matches!(token, "require" | "dofile")
                && tokens.get(i + 1).is_some_and(|t| t.word() == "(")
            {
                Some((token, i + 1))
            } else {
                None
            };
            if let Some((name, opening)) = call
                .filter(|(name, _)| matches!(*name, "gesture" | "device" | "require" | "dofile"))
            {
                let Some(end) = closing(&tokens, opening) else {
                    self.incomplete();
                    break;
                };
                if blocks > 0 || nesting > 0 {
                    self.incomplete();
                } else if matches!(name, "require" | "dofile") {
                    if let Some(value) = literal(&tokens[opening + 1..end]) {
                        self.include(&value, path, name == "require");
                    } else {
                        self.incomplete();
                    }
                } else if let Some(fields) = fields(&tokens[opening + 1..end]) {
                    if name == "gesture" {
                        self.lua_gesture(&fields);
                    } else {
                        self.device(&fields);
                    }
                } else {
                    self.incomplete();
                }
                i = end + 1;
                continue;
            }
            match token {
                "function" | "if" | "do" | "repeat" => blocks += 1,
                "end" | "until" => blocks = blocks.saturating_sub(1),
                "(" | "{" | "[" => nesting += 1,
                ")" | "}" | "]" => nesting = nesting.saturating_sub(1),
                _ => {}
            }
            // Assigning or passing the gesture function can generate registrations later.
            if token == "gesture"
                && tokens
                    .get(i.wrapping_sub(1))
                    .is_some_and(|t| t.word() == ".")
            {
                self.incomplete();
            }
            i += 1;
        }
    }

    fn lua_gesture(&mut self, fields: &BTreeMap<String, Vec<Token>>) {
        let get = |key: &str| fields.get(key).and_then(|v| literal(v));
        let (Some(fingers), Some(direction)) = (
            get("fingers").and_then(|v| v.parse::<u8>().ok()),
            get("direction"),
        ) else {
            self.incomplete();
            return;
        };
        let mods = if let Some(value) = fields.get("mods") {
            let Some(value) = literal(value) else {
                self.incomplete();
                return;
            };
            value
        } else {
            String::new()
        };
        let Some(scale) = fields
            .get("scale")
            .map(|v| literal(v))
            .unwrap_or(Some("1".into()))
        else {
            self.incomplete();
            return;
        };
        let opaque_callback = fields.get("action").is_some_and(|value| {
            value
                .first()
                .is_some_and(|token| token.word() == "function")
                || matches!(value.as_slice(), [Token::Word(name)]
                    if !matches!(name.as_str(), "true" | "false" | "nil")
                    && name.chars().next().is_some_and(char::is_alphabetic))
        });
        let action = if opaque_callback {
            "Custom action".into()
        } else if let Some(action) = get("action") {
            action
        } else {
            self.incomplete();
            return;
        };
        let mut arguments = Vec::new();
        for key in ["dispatcher", "arg", "workspace_name", "mode", "zoom_level"] {
            if fields.contains_key(key) {
                if let Some(value) = get(key) {
                    arguments.push(value);
                } else {
                    self.incomplete();
                    return;
                }
            }
        }
        let extra = arguments.join(": ");
        self.gesture(
            fingers,
            direction,
            &mods,
            &scale,
            if extra.is_empty() {
                action
            } else {
                format!("{action}: {extra}")
            },
        );
    }

    fn device(&mut self, fields: &BTreeMap<String, Vec<Token>>) {
        let Some(name) = fields.get("name").and_then(|v| literal(v)) else {
            self.incomplete();
            return;
        };
        for &(key, _) in super::SETTINGS {
            if let Some(tokens) = fields.get(key) {
                if let Some(value) = literal(tokens) {
                    self.config
                        .devices
                        .entry(normalize_device_name(&name))
                        .or_default()
                        .insert(key.into(), value);
                } else {
                    self.incomplete();
                }
            }
        }
    }

    fn gesture(&mut self, fingers: u8, direction: String, mods: &str, scale: &str, action: String) {
        if !(2..=10).contains(&fingers)
            || !matches!(
                direction.as_str(),
                "swipe"
                    | "horizontal"
                    | "vertical"
                    | "left"
                    | "right"
                    | "up"
                    | "down"
                    | "pinch"
                    | "pinchin"
                    | "pinchout"
            )
        {
            self.incomplete();
            return;
        }
        let Some(scale) = scale
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v.to_string())
        else {
            self.incomplete();
            return;
        };
        let mods = self.expand(mods).replace('+', " ");
        if mods.split_whitespace().any(|m| {
            !matches!(
                m.to_ascii_uppercase().as_str(),
                "SUPER" | "SHIFT" | "CTRL" | "CONTROL" | "ALT" | "MOD4" | "MOD1"
            )
        }) {
            self.incomplete();
            return;
        }
        let modifiers = Modifiers::from_words(&mods);
        self.config.gestures.bindings.retain(|g| {
            !(g.fingers == fingers
                && g.direction == direction
                && g.modifiers == modifiers
                && g.scale == scale)
        });
        if action != "unset" {
            self.config.gestures.bindings.push(GestureBinding {
                fingers,
                direction,
                modifiers,
                action,
                scale,
            });
        }
    }

    fn legacy(&mut self, text: &str, path: &Path) {
        let mut section = Vec::<String>::new();
        let mut device = BTreeMap::<String, String>::new();
        for raw in text.lines() {
            let line = strip_legacy_comment(raw).trim().to_string();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_suffix('{') {
                section.push(name.trim().into());
                continue;
            }
            if line == "}" {
                if section.last().is_some_and(|s| s == "device")
                    && let Some(name) = device.remove("name")
                {
                    self.config
                        .devices
                        .entry(normalize_device_name(&name))
                        .or_default()
                        .extend(std::mem::take(&mut device));
                }
                section.pop();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = self.expand(value.trim());
            if let Some(key) = key.strip_prefix('$') {
                self.variables.insert(key.into(), value);
                continue;
            }
            if key == "source" {
                self.include(value.trim_matches(['\'', '"']), path, false);
            } else if key == "gesture" {
                let parts: Vec<_> = value.split(',').map(str::trim).collect();
                if parts.len() < 3 {
                    self.incomplete();
                    continue;
                }
                let Ok(fingers) = parts[0].parse() else {
                    self.incomplete();
                    continue;
                };
                let mut index = 2;
                let mut mods = "";
                let mut scale = "1";
                while index < parts.len() {
                    if let Some(value) = parts[index].strip_prefix("mod:") {
                        mods = value;
                    } else if let Some(value) = parts[index].strip_prefix("scale:") {
                        scale = value.trim();
                    } else {
                        break;
                    }
                    index += 1;
                }
                if index >= parts.len() {
                    self.incomplete();
                    continue;
                }
                self.gesture(
                    fingers,
                    parts[1].into(),
                    mods,
                    scale,
                    parts[index..].join(": "),
                );
            } else if section.last().is_some_and(|s| s == "device") {
                device.insert(
                    key.replace('-', "_"),
                    value.trim_matches(['\'', '"']).into(),
                );
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    String(String),
}
impl Token {
    fn word(&self) -> &str {
        match self {
            Self::Word(s) => s,
            Self::String(_) => "",
        }
    }
}
fn literal(tokens: &[Token]) -> Option<String> {
    match tokens {
        [Token::String(value)] => Some(value.clone()),
        [Token::Word(value)]
            if matches!(value.as_str(), "true" | "false") || value.parse::<f64>().is_ok() =>
        {
            Some(value.clone())
        }
        _ if !tokens.is_empty() && tokens.iter().all(|t| matches!(t, Token::Word(_))) => {
            let number = tokens.iter().map(Token::word).collect::<String>();
            number.parse::<f64>().ok().map(|v| v.to_string())
        }
        _ => None,
    }
}
fn closing(tokens: &[Token], start: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, t) in tokens.iter().enumerate().skip(start) {
        match t.word() {
            "(" | "{" | "[" => depth += 1,
            ")" | "}" | "]" => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}
fn fields(tokens: &[Token]) -> Option<BTreeMap<String, Vec<Token>>> {
    if tokens.first()?.word() != "{" || closing(tokens, 0)? != tokens.len() - 1 {
        return None;
    }
    let mut result = BTreeMap::new();
    let mut start = 1;
    let mut nesting = 0usize;
    let mut blocks = 0usize;
    for i in 1..tokens.len() {
        let word = tokens[i].word();
        if (word == "," || word == ";" || i == tokens.len() - 1) && nesting == 0 && blocks == 0 {
            if i > start {
                if tokens.get(start + 1)?.word() != "=" {
                    return None;
                }
                result.insert(
                    tokens[start].word().to_string(),
                    tokens[start + 2..i].to_vec(),
                );
            }
            start = i + 1;
        } else {
            match word {
                "(" | "{" | "[" => nesting += 1,
                ")" | "}" | "]" => nesting = nesting.saturating_sub(1),
                "function" | "if" | "do" | "repeat" => blocks += 1,
                "end" | "until" => blocks = blocks.saturating_sub(1),
                _ => {}
            }
        }
    }
    Some(result)
}

fn lex(text: &str) -> Vec<Token> {
    let chars: Vec<_> = text.chars().collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '-' && chars.get(i + 1) == Some(&'-') {
            i += 2;
            if let Some((_, end)) = long_string(&chars, i) {
                i = end;
            } else {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            continue;
        }
        if let Some((s, end)) = long_string(&chars, i) {
            result.push(Token::String(s));
            i = end;
            continue;
        }
        if matches!(chars[i], '\'' | '"') {
            let quote = chars[i];
            i += 1;
            let mut value = String::new();
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                value.push(chars[i]);
                i += 1;
            }
            i += 1;
            result.push(Token::String(value));
        } else if chars[i].is_alphanumeric() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            result.push(Token::Word(chars[start..i].iter().collect()));
        } else {
            result.push(Token::Word(chars[i].to_string()));
            i += 1;
        }
    }
    result
}
fn long_string(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut opening = start + 1;
    while chars.get(opening) == Some(&'=') {
        opening += 1;
    }
    if chars.get(opening) != Some(&'[') {
        return None;
    }
    let equals = opening - start - 1;
    let mut i = opening + 1;
    while i < chars.len() {
        if chars[i] == ']'
            && (0..equals).all(|n| chars.get(i + 1 + n) == Some(&'='))
            && chars.get(i + 1 + equals) == Some(&']')
        {
            return Some((chars[opening + 1..i].iter().collect(), i + equals + 2));
        }
        i += 1;
    }
    Some((chars[opening + 1..].iter().collect(), chars.len()))
}
fn strip_legacy_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if Some(c) == quote {
            quote = None;
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
        } else if quote.is_none() && c == '#' {
            return &line[..i];
        }
    }
    line
}
fn expand_glob(path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::new()];
    for component in path.components() {
        let part = component.as_os_str().to_string_lossy();
        if part.contains(['*', '?']) {
            paths = paths
                .into_iter()
                .flat_map(|base| {
                    fs::read_dir(base)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .filter(|e| wildcard(&part, &e.file_name().to_string_lossy()))
                        .map(|e| e.path())
                        .collect::<Vec<_>>()
                })
                .collect();
        } else {
            paths = paths.into_iter().map(|p| p.join(component)).collect();
        }
    }
    paths.into_iter().filter(|p| p.is_file()).collect()
}
fn wildcard(pattern: &str, value: &str) -> bool {
    let p: Vec<_> = pattern.chars().collect();
    let v: Vec<_> = value.chars().collect();
    let mut previous = vec![false; v.len() + 1];
    previous[0] = true;
    for c in p {
        let mut next = vec![false; v.len() + 1];
        next[0] = c == '*' && previous[0];
        for j in 1..=v.len() {
            next[j] = if c == '*' {
                previous[j] || next[j - 1]
            } else {
                (c == '?' || c == v[j - 1]) && previous[j - 1]
            };
        }
        previous = next;
    }
    previous[v.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reader() -> Reader {
        Reader {
            config: Config {
                gestures: GestureDiscovery {
                    status: DiscoveryStatus::Complete,
                    ..Default::default()
                },
                ..Default::default()
            },
            roots: Vec::new(),
            stack: BTreeSet::new(),
            required: BTreeSet::new(),
            variables: BTreeMap::new(),
        }
    }
    #[test]
    fn lua_literals_comments_callbacks_and_unset() {
        let mut r = reader();
        r.lua(r#"
-- hl.gesture({fingers=4, direction="down", action="close"})
--[=[ hl.gesture({fingers=4, direction="up", action="close"}) ]=]
hl.gesture({ fingers=3, direction="horizontal", mods="SUPER", action="workspace" })
hl.gesture({ fingers=3, direction="horizontal", mods="SUPER", action="unset" })
hl.gesture({ fingers=3, direction="left", action=function() hl.dispatch(hl.dsp.focus({direction="l"})) end })
hl.device({name="Test Pad", left_handed=true, tap_to_click=false})
"#, Path::new("test.lua"));
        assert_eq!(r.config.gestures.bindings.len(), 1);
        assert_eq!(r.config.gestures.bindings[0].action, "Custom action");
        assert_eq!(r.config.devices["test-pad"]["left_handed"], "true");
        assert_eq!(r.config.gestures.status, DiscoveryStatus::Complete);
    }
    #[test]
    fn dynamic_lua_is_incomplete_and_never_displayed_as_active() {
        let mut r = reader();
        r.lua(
            r#"if laptop then hl.gesture({fingers=3,direction="up",action="close"}) end
hl.gesture({fingers=count,direction="down",action="close"})
local register = hl.gesture"#,
            Path::new("test.lua"),
        );
        assert!(r.config.gestures.bindings.is_empty());
        assert_eq!(r.config.gestures.status, DiscoveryStatus::Incomplete);
    }
    #[test]
    fn legacy_modifiers_removal_and_devices() {
        let mut r = reader();
        r.legacy("$mod = SUPER\ngesture = 3, left, mod: $mod, scale: 1.2, workspace\ngesture = 3, left, mod: $mod, scale: 1.2, unset\ngesture = 4, up, dispatcher, exec, example # comment\ndevice {\nname = pad\nleft_handed = 1\n}\n",Path::new("test.conf"));
        assert_eq!(r.config.gestures.bindings.len(), 1);
        assert_eq!(
            r.config.gestures.bindings[0].action,
            "dispatcher: exec: example"
        );
        assert_eq!(r.config.devices["pad"]["left_handed"], "1");
    }
    #[test]
    fn includes_follow_order_and_cycles_are_incomplete() {
        let root = std::env::temp_dir().join(format!("keywrld-config-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("main.conf"),
            "gesture = 3, left, workspace\nsource = child.conf\n",
        )
        .unwrap();
        fs::write(
            root.join("child.conf"),
            "gesture = 3, left, unset\nsource = main.conf\n",
        )
        .unwrap();
        let mut r = reader();
        r.read(&root.join("main.conf"), false);
        assert!(r.config.gestures.bindings.is_empty());
        assert_eq!(r.config.gestures.status, DiscoveryStatus::Incomplete);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn filename_patterns_match_without_running_a_shell() {
        assert!(wildcard("*.conf", "input.conf"));
        assert!(!wildcard("*.conf", "input.lua"));
        assert!(wildcard("pad?", "pad1"));
    }
    #[test]
    fn gesture_removal_matches_scale_and_named_actions_keep_details() {
        let mut r = reader();
        r.lua(
            r#"
hl.gesture({fingers=3,direction="up",scale=1.5,action="fullscreen",mode="maximize"})
hl.gesture({fingers=3,direction="up",scale=1,action="unset"})
hl.gesture({fingers=4,direction="down",action=my_callback})
"#,
            Path::new("test.lua"),
        );
        assert_eq!(r.config.gestures.bindings.len(), 2);
        assert_eq!(r.config.gestures.bindings[0].scale, "1.5");
        assert_eq!(r.config.gestures.bindings[0].action, "fullscreen: maximize");
        assert_eq!(r.config.gestures.bindings[1].action, "Custom action");
        r.lua(
            r#"hl.gesture({fingers=3,direction="up",scale=1.5,action="unset"})"#,
            Path::new("test.lua"),
        );
        assert_eq!(r.config.gestures.bindings.len(), 1);
    }

    #[test]
    fn lua_requires_run_in_place_once_with_device_overrides() {
        let root = std::env::temp_dir().join(format!("keywrld-lua-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("main.lua"),
            r#"
hl.gesture({fingers=3,direction="left",action="workspace"})
require("child")
hl.device({name="pad",left_handed=false})
require("child")
"#,
        )
        .unwrap();
        fs::write(
            root.join("child.lua"),
            r#"
hl.gesture({fingers=3,direction="left",action="unset"})
hl.device({name="pad",left_handed=true,tap_to_click=false})
"#,
        )
        .unwrap();
        let mut r = reader();
        r.roots.push(root.clone());
        r.read(&root.join("main.lua"), false);
        assert_eq!(r.config.gestures.status, DiscoveryStatus::Complete);
        assert!(r.config.gestures.bindings.is_empty());
        assert_eq!(r.config.devices["pad"]["left_handed"], "false");
        assert_eq!(r.config.devices["pad"]["tap_to_click"], "false");
        fs::remove_dir_all(root).unwrap();
    }
}
