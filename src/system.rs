use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::peripherals::{PointingDevices, discover_pointing_devices};
use serde_json::Value;

use crate::core::{
    Binding, DeviceCandidate, KeyboardLayout, Modifiers, detect_keyboard_layout, normalize_key,
    parse_bindings,
};

#[derive(Clone, Debug)]
pub struct RuntimeSnapshot {
    pub bindings: Vec<Binding>,
    pub binding_error: Option<String>,
    pub layout: KeyboardLayout,
    pub device_name: String,
    pub monitor_connector: Option<String>,
    pub pointing: PointingDevices,
}

pub fn load_runtime(layout_override: Option<KeyboardLayout>) -> RuntimeSnapshot {
    let (bindings, binding_error) = match load_bindings() {
        Ok(bindings) => (bindings, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let registry = hyprland_devices();
    let devices = discover_keyboards(registry.as_ref());
    let layout = detect_keyboard_layout(layout_override, &devices);
    let device_name = display_device_name(&devices, layout);

    RuntimeSnapshot {
        bindings,
        binding_error,
        layout,
        device_name,
        monitor_connector: active_monitor_connector(),
        pointing: discover_pointing_devices(registry.as_ref()),
    }
}

fn load_bindings() -> Result<Vec<Binding>, String> {
    let hyprland = Command::new("hyprctl")
        .arg("binds")
        .output()
        .map_err(|error| format!("Could not query Hyprland's binding registry: {error}"))?;

    if !hyprland.status.success() {
        let stderr = String::from_utf8_lossy(&hyprland.stderr);
        return Err(format!(
            "Hyprland could not read the system binding registry{}",
            stderr
                .lines()
                .next()
                .map(|line| format!(": {line}"))
                .unwrap_or_default()
        ));
    }

    let supplements = load_omarchy_bindings().unwrap_or_default();
    let stdout = String::from_utf8_lossy(&hyprland.stdout);
    let bindings = parse_hyprland_bindings(&stdout, &supplements);
    if bindings.is_empty() {
        return Err("Hyprland returned no displayable system bindings".into());
    }

    Ok(bindings)
}

fn load_omarchy_bindings() -> Result<Vec<Binding>, String> {
    let output = Command::new("omarchy")
        .args(["menu", "keybindings", "--print"])
        .output()
        .map_err(|error| format!("Could not run Omarchy: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let bindings = parse_bindings(&stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Omarchy could not read the active bindings{}",
            stderr
                .lines()
                .next()
                .map(|line| format!(": {line}"))
                .unwrap_or_default()
        ));
    }

    Ok(bindings)
}

#[derive(Default)]
struct HyprlandBindingRecord {
    modmask: u16,
    key: String,
    keycode: u16,
    description: String,
    dispatcher: String,
    arg: String,
}

fn parse_hyprland_bindings(output: &str, supplements: &[Binding]) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut record = None::<HyprlandBindingRecord>;

    for line in output.lines() {
        if line.starts_with("bind") {
            if let Some(record) = record.take() {
                add_hyprland_binding(&mut bindings, record, supplements);
            }
            record = Some(HyprlandBindingRecord::default());
            continue;
        }

        let Some(record) = record.as_mut() else {
            continue;
        };
        let Some((field, value)) = line.trim_start().split_once(':') else {
            continue;
        };
        let value = value.trim();
        match field {
            "modmask" => record.modmask = value.parse().unwrap_or_default(),
            "key" => record.key = value.to_string(),
            "keycode" => record.keycode = value.parse().unwrap_or_default(),
            "description" => record.description = value.to_string(),
            "dispatcher" => record.dispatcher = value.to_string(),
            "arg" => record.arg = value.to_string(),
            _ => {}
        }
    }

    if let Some(record) = record {
        add_hyprland_binding(&mut bindings, record, supplements);
    }
    bindings
}

fn add_hyprland_binding(
    bindings: &mut Vec<Binding>,
    record: HyprlandBindingRecord,
    supplements: &[Binding],
) {
    let modifiers = Modifiers::from_hyprland_mask(record.modmask);
    let mut key = record
        .key
        .rsplit_once(" + ")
        .map_or(record.key.as_str(), |(_, key)| key)
        .trim()
        .to_string();

    if key.is_empty() && record.keycode != 0 {
        key = format!("code:{}", record.keycode);
    }
    if key.is_empty() && !record.description.is_empty() {
        key = supplements
            .iter()
            .find(|binding| binding.modifiers == modifiers && binding.action == record.description)
            .map(|binding| binding.key.clone())
            .unwrap_or_default();
    }
    if key.is_empty() || key.to_ascii_lowercase().starts_with("switch:") {
        return;
    }

    let action = if record.description.is_empty() {
        match (record.dispatcher.as_str(), record.arg.as_str()) {
            ("", "") => "System binding".to_string(),
            (dispatcher, "") => dispatcher.to_string(),
            ("exec", arg) => arg.to_string(),
            (dispatcher, arg) => format!("{dispatcher}: {arg}"),
        }
    } else {
        record.description
    };
    let binding = Binding {
        modifiers,
        key: normalize_key(&key),
        action,
        dispatcher: record.dispatcher,
        arg: record.arg,
    };

    if !is_obvious_labeled_binding(&binding) && !bindings.contains(&binding) {
        bindings.push(binding);
    }
}

fn binding_command(binding: &Binding) -> Result<Command, String> {
    let mut command = Command::new("hyprctl");
    match binding.dispatcher.as_str() {
        "" => return Err("Shortcut has no registered command".into()),
        "__lua" => {
            // Hyprland stores Lua binding callbacks as Lua registry references.
            let reference = binding
                .arg
                .parse::<u32>()
                .ok()
                .filter(|reference| *reference > 0)
                .ok_or("Shortcut has an invalid Lua callback reference")?;
            command.arg("eval").arg(format!(
                "local callback = debug.getregistry()[{reference}]; assert(type(callback) == 'function', 'Shortcut callback is no longer available'); callback()"
            ));
        }
        dispatcher => {
            command.args(["dispatch", "--", dispatcher, &binding.arg]);
        }
    }
    Ok(command)
}

pub fn execute_binding(binding: &Binding) -> Result<(), String> {
    let output = binding_command(binding)?
        .output()
        .map_err(|error| format!("Could not execute shortcut: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not execute shortcut: {} {}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn is_obvious_labeled_binding(binding: &Binding) -> bool {
    binding.modifiers.is_empty()
        && (binding.key.starts_with("XF86")
            || matches!(
                binding.key.as_str(),
                "PRINT" | "SCROLLLOCK" | "PAUSE" | "NUMLOCK" | "CAPSLOCK"
            ))
}

fn active_monitor_connector() -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["-j", "activeworkspace"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    value.get("monitor")?.as_str().map(str::to_string)
}

fn discover_keyboards(registry: Option<&Value>) -> Vec<DeviceCandidate> {
    let hyprland_names = hyprland_keyboard_names(registry);
    let Ok(entries) = fs::read_dir("/sys/class/input") else {
        return Vec::new();
    };

    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if !file_name.to_string_lossy().starts_with("event") {
            continue;
        }
        let device = entry.path().join("device");
        let Some(name) = read_trimmed(device.join("name")) else {
            continue;
        };
        let normalized = normalize_device_name(&name);
        let capabilities = read_trimmed(device.join("capabilities/key"))
            .map(|value| parse_key_bitmap(&value))
            .unwrap_or_default();

        let seen_by_hyprland = hyprland_names.is_empty() || hyprland_names.contains(&normalized);
        let keyboard_like =
            capabilities.contains(&30) && capabilities.contains(&44) && capabilities.contains(&57);
        if !seen_by_hyprland || !keyboard_like {
            continue;
        }

        let lower_name = name.to_ascii_lowercase();
        let virtual_like = [
            "virtual",
            "consumer control",
            "headset",
            "hotkeys",
            "power button",
            "video bus",
            "mouse",
            "deathadder",
        ]
        .iter()
        .any(|marker| lower_name.contains(marker));

        devices.push(DeviceCandidate {
            name,
            vendor: read_hex_u16(device.join("id/vendor")),
            product: read_hex_u16(device.join("id/product")),
            key_codes: capabilities,
            virtual_like,
        });
    }

    devices.sort_by(|left, right| {
        left.virtual_like
            .cmp(&right.virtual_like)
            .then_with(|| right.key_codes.len().cmp(&left.key_codes.len()))
            .then_with(|| left.name.cmp(&right.name))
    });
    devices.dedup_by(|left, right| {
        left.name == right.name && left.vendor == right.vendor && left.product == right.product
    });
    devices
}

fn hyprland_devices() -> Option<Value> {
    Command::new("hyprctl")
        .args(["-j", "devices"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
}

fn hyprland_keyboard_names(registry: Option<&Value>) -> BTreeSet<String> {
    let Some(value) = registry else {
        return BTreeSet::new();
    };

    value
        .get("keyboards")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|keyboard| keyboard.get("name").and_then(Value::as_str))
        .map(normalize_device_name)
        .collect()
}

fn display_device_name(devices: &[DeviceCandidate], layout: KeyboardLayout) -> String {
    let exact = devices.iter().find(|device| {
        !device.virtual_like && device.vendor == Some(0x1b1c) && device.product == Some(0x1b45)
    });
    let device = exact.or_else(|| devices.iter().find(|device| !device.virtual_like));
    device
        .map(|device| {
            format!(
                "{} · {}",
                simplify_device_name(&device.name),
                layout.label()
            )
        })
        .unwrap_or_else(|| format!("Detected keyboard · {}", layout.label()))
}

fn simplify_device_name(name: &str) -> String {
    let words: Vec<_> = name.split_whitespace().collect();
    let start = if words.len() > 1 && words[0].eq_ignore_ascii_case(words[1]) {
        1
    } else {
        0
    };
    words[start..].join(" ")
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

fn read_hex_u16(path: impl AsRef<Path>) -> Option<u16> {
    u16::from_str_radix(&read_trimmed(path)?, 16).ok()
}

pub(crate) fn normalize_device_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            separator = false;
        } else if !separator && !normalized.is_empty() {
            normalized.push('-');
            separator = true;
        }
    }
    normalized.trim_end_matches('-').to_string()
}

pub(crate) fn parse_key_bitmap(bitmap: &str) -> BTreeSet<u16> {
    let words: Vec<_> = bitmap
        .split_whitespace()
        .filter_map(|word| u64::from_str_radix(word, 16).ok())
        .collect();
    let mut codes = BTreeSet::new();

    for (word_index, word) in words.iter().rev().enumerate() {
        for bit in 0..64 {
            if word & (1_u64 << bit) != 0 {
                let code = word_index * 64 + bit;
                if let Ok(code) = u16::try_from(code) {
                    codes.insert(code);
                }
            }
        }
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kernel_bitmap_words_from_lowest_word_on_the_right() {
        let keys = parse_key_bitmap("2 5");
        assert_eq!(keys, BTreeSet::from([0, 2, 65]));
    }

    #[test]
    fn normalizes_hyprland_and_sysfs_device_names_the_same_way() {
        assert_eq!(
            normalize_device_name("CORSAIR CORSAIR K63 Wireless Mechanical Gaming Keyboard"),
            "corsair-corsair-k63-wireless-mechanical-gaming-keyboard"
        );
    }

    #[test]
    fn commands_preserve_registered_dispatchers_and_arguments() {
        for (dispatcher, arg) in [
            ("exec", "terminal --title 'A; B' -e sh -c 'echo $HOME'"),
            ("workspace", "+1"),
            ("killactive", ""),
            ("exec", "--flag"),
        ] {
            let bindings = parse_hyprland_bindings(
                &format!(
                    "bind\n\tmodmask: 64\n\tkey: T\n\tdescription: Action\n\tdispatcher: {dispatcher}\n\targ: {arg}\n"
                ),
                &[],
            );
            let command = binding_command(&bindings[0]).unwrap();
            assert_eq!(command.get_program(), "hyprctl");
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                ["dispatch", "--", dispatcher, arg]
            );
        }
    }

    #[test]
    fn lua_commands_invoke_only_valid_registry_references() {
        let mut binding = Binding {
            dispatcher: "__lua".into(),
            arg: "71".into(),
            ..Default::default()
        };
        let command = binding_command(&binding).unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args[0], "eval");
        assert!(
            args[1]
                .to_str()
                .unwrap()
                .contains("debug.getregistry()[71]")
        );
        assert!(
            args[1]
                .to_str()
                .unwrap()
                .contains("type(callback) == 'function'")
        );
        for arg in ["", "0", "-1", "71]; os.execute('anything')"] {
            binding.arg = arg.into();
            assert!(binding_command(&binding).is_err());
        }
        assert!(binding_command(&Binding::default()).is_err());
    }

    #[test]
    fn parses_live_hyprland_records_and_recovers_lua_code_keys() {
        let supplements = parse_bindings(
            "SUPER + 1 → Switch to workspace 1\n\
             SHIFT ALT + D → App-only shortcut\n",
        );
        let output = "\
bindd
\tmodmask: 0
\tkey: XF86AudioRaiseVolume
\tkeycode: 0
\tdescription: Volume up
\tdispatcher: exec
\targ: volume-up

bindd
\tmodmask: 64
\tkey:
\tkeycode: 0
\tdescription: Switch to workspace 1
\tdispatcher: __lua
\targ: 71

bindd
\tmodmask: 72
\tkey: mouse:272
\tkeycode: 0
\tdescription: Move window precisely
\tdispatcher: __lua
\targ: 73

bind
\tmodmask: 12
\tkey: DELETE
\tkeycode: 0
\tdescription:
\tdispatcher: exec
\targ: close-everything
";

        let bindings = parse_hyprland_bindings(output, &supplements);
        assert_eq!(bindings.len(), 3);
        assert_eq!(
            bindings[0],
            Binding {
                modifiers: Modifiers::SUPER,
                key: "1".into(),
                action: "Switch to workspace 1".into(),
                dispatcher: "__lua".into(),
                arg: "71".into(),
            }
        );
        assert_eq!(bindings[1].key, "MOUSE_LEFT");
        assert_eq!(bindings[1].modifiers, Modifiers::SUPER.with(Modifiers::ALT));
        assert_eq!(bindings[2].action, "close-everything");
        assert!(
            bindings
                .iter()
                .all(|binding| binding.action != "App-only shortcut")
        );
    }
}
