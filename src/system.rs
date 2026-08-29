use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::core::{
    Binding, DeviceCandidate, KeyboardLayout, Palette, detect_keyboard_layout, parse_bindings,
};

#[derive(Clone, Debug)]
pub struct RuntimeSnapshot {
    pub bindings: Vec<Binding>,
    pub binding_error: Option<String>,
    pub layout: KeyboardLayout,
    pub device_name: String,
    pub monitor_connector: Option<String>,
    pub palette: Palette,
}

pub fn load_runtime(layout_override: Option<KeyboardLayout>) -> RuntimeSnapshot {
    let (bindings, binding_error) = match load_bindings() {
        Ok(bindings) => (bindings, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let devices = discover_keyboards();
    let layout = detect_keyboard_layout(layout_override, &devices);
    let device_name = display_device_name(&devices, layout);

    RuntimeSnapshot {
        bindings,
        binding_error,
        layout,
        device_name,
        monitor_connector: active_monitor_connector(),
        palette: load_palette(),
    }
}

fn load_bindings() -> Result<Vec<Binding>, String> {
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

    if !bindings
        .iter()
        .any(|binding| binding.modifiers.contains(crate::core::Modifiers::SUPER))
    {
        return Err("Omarchy returned no SUPER bindings".into());
    }

    Ok(bindings)
}

fn load_palette() -> Palette {
    let Some(home) = std::env::var_os("HOME") else {
        return Palette::default();
    };
    let state_root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".local/state"));
    let path = state_root
        .join("omarchy")
        .join("current")
        .join("theme")
        .join("colors.toml");

    fs::read_to_string(path)
        .map(|contents| Palette::from_toml(&contents))
        .unwrap_or_default()
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

fn discover_keyboards() -> Vec<DeviceCandidate> {
    let hyprland_names = hyprland_keyboard_names();
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

fn hyprland_keyboard_names() -> BTreeSet<String> {
    let Some(value) = Command::new("hyprctl")
        .args(["-j", "devices"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
    else {
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

fn normalize_device_name(name: &str) -> String {
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

fn parse_key_bitmap(bitmap: &str) -> BTreeSet<u16> {
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
}
