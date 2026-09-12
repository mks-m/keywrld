//! Read-only pointing-device discovery and display data, refreshed on activation.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::core::{Binding, Modifiers, chord_label};
use crate::system::{normalize_device_name, parse_key_bitmap};

#[path = "pointer_config.rs"]
mod config;

#[derive(Clone, Debug, Default)]
pub struct PointingDevices {
    pub mice: Vec<String>,
    pub mouse_left: bool,
    pub touchpads: Vec<Touchpad>,
    pub gestures: GestureDiscovery,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Touchpad {
    pub name: String,
    pub settings: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct GestureDiscovery {
    pub bindings: Vec<GestureBinding>,
    pub status: DiscoveryStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiscoveryStatus {
    Complete,
    Incomplete,
    #[default]
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GestureBinding {
    pub fingers: u8,
    pub direction: String,
    pub modifiers: Modifiers,
    pub action: String,
    pub scale: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeripheralRow {
    pub label: String,
    pub action: String,
}

impl PeripheralRow {
    fn new(label: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
        }
    }
}

const SETTINGS: &[(&str, &str)] = &[
    ("tap_to_click", "input:touchpad:tap-to-click"),
    ("tap_button_map", "input:touchpad:tap_button_map"),
    (
        "clickfinger_behavior",
        "input:touchpad:clickfinger_behavior",
    ),
    ("natural_scroll", "input:touchpad:natural_scroll"),
    ("scroll_method", "input:scroll_method"),
    ("left_handed", "input:left_handed"),
];

pub fn discover_pointing_devices(registry: Option<&Value>) -> PointingDevices {
    let inputs = discover_inputs(registry);
    let parsed = config::load();
    let global_left = query_option("input:left_handed").and_then(|v| boolean(&v));
    let mice: Vec<_> = inputs
        .iter()
        .filter(|input| input.kind == InputKind::Mouse)
        .collect();
    let mouse_left = mouse_side(
        global_left,
        &mice
            .iter()
            .map(|input| {
                parsed
                    .devices
                    .get(&normalize_device_name(&input.name))
                    .and_then(|s| s.get("left_handed"))
                    .and_then(|v| boolean(v))
                    .or(global_left)
            })
            .collect::<Vec<_>>(),
    );
    let mouse_names = mice.iter().map(|input| input.name.clone()).collect();
    let mut globals = BTreeMap::new();
    if inputs.iter().any(|input| input.kind == InputKind::Touchpad) {
        for &(name, option) in SETTINGS {
            if let Some(value) = if name == "left_handed" {
                global_left.map(|v| v.to_string())
            } else {
                query_option(option)
            } {
                globals.insert(name.to_string(), value);
            }
        }
    }
    let touchpads = inputs
        .into_iter()
        .filter(|input| input.kind == InputKind::Touchpad)
        .map(|input| {
            let mut settings = globals.clone();
            if let Some(overrides) = parsed.devices.get(&normalize_device_name(&input.name)) {
                settings.extend(overrides.clone());
            }
            Touchpad {
                name: input.name,
                settings,
            }
        })
        .collect();
    PointingDevices {
        mice: mouse_names,
        mouse_left,
        touchpads,
        gestures: parsed.gestures,
    }
}

fn query_option(option: &str) -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["-j", "getoption", option])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    option_value(&serde_json::from_slice::<Value>(&output.stdout).ok()?)
}

fn option_value(value: &Value) -> Option<String> {
    for field in ["bool", "int", "str"] {
        if let Some(value) = value.get(field) {
            return match value {
                Value::Bool(v) => Some(v.to_string()),
                Value::Number(v) => Some(v.to_string()),
                Value::String(v) => Some(v.clone()),
                _ => None,
            };
        }
    }
    None
}

pub(super) fn boolean(value: &str) -> Option<bool> {
    match value {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn mouse_side(global: Option<bool>, mice: &[Option<bool>]) -> bool {
    match mice.first().copied().flatten() {
        Some(first) if mice.iter().all(|value| *value == Some(first)) => first,
        _ => global.unwrap_or(false),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    Mouse,
    Touchpad,
}
struct Input {
    name: String,
    kind: InputKind,
}

fn classify_input(
    name: &str,
    properties: &str,
    keys: &BTreeSet<u16>,
    abs: &BTreeSet<u16>,
    rel: &BTreeSet<u16>,
    props: &BTreeSet<u16>,
) -> Option<InputKind> {
    let flag = |key: &str| properties.lines().any(|line| line == format!("E:{key}=1"));
    if name.to_ascii_lowercase().contains("virtual")
        || flag("ID_INPUT_TOUCHSCREEN")
        || props.contains(&1)
    {
        return None;
    }
    if flag("ID_INPUT_TOUCHPAD")
        || (keys.contains(&325)
            && abs.contains(&0)
            && abs.contains(&1)
            && (props.contains(&0) || !keys.contains(&320)))
    {
        return Some(InputKind::Touchpad);
    }
    if flag("ID_INPUT_MOUSE") || (keys.contains(&272) && rel.contains(&0) && rel.contains(&1)) {
        return Some(InputKind::Mouse);
    }
    None
}

fn discover_inputs(registry: Option<&Value>) -> Vec<Input> {
    let names: Option<BTreeSet<_>> = registry
        .and_then(|r| r.get("mice"))
        .and_then(Value::as_array)
        .map(|mice| {
            mice.iter()
                .filter_map(|m| m.get("name").and_then(Value::as_str))
                .map(normalize_device_name)
                .collect()
        });
    let Ok(entries) = fs::read_dir("/sys/class/input") else {
        return Vec::new();
    };
    let mut inputs = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        let path = entry.path();
        let device = path.join("device");
        let Ok(name) = fs::read_to_string(device.join("name")) else {
            continue;
        };
        let name = name.trim().to_string();
        if names
            .as_ref()
            .is_some_and(|names| !names.contains(&normalize_device_name(&name)))
        {
            continue;
        }
        // Ignore software-created input nodes even when their names look physical.
        if fs::canonicalize(&device)
            .ok()
            .is_some_and(|p| p.starts_with("/sys/devices/virtual/input"))
        {
            continue;
        }
        let properties = fs::read_to_string(path.join("dev"))
            .ok()
            .and_then(|dev| {
                fs::read_to_string(Path::new("/run/udev/data").join(format!("c{}", dev.trim())))
                    .ok()
            })
            .unwrap_or_default();
        let bitmap = |field: &str| {
            parse_key_bitmap(&fs::read_to_string(device.join(field)).unwrap_or_default())
        };
        if let Some(kind) = classify_input(
            &name,
            &properties,
            &bitmap("capabilities/key"),
            &bitmap("capabilities/abs"),
            &bitmap("capabilities/rel"),
            &bitmap("properties"),
        ) {
            inputs.push(Input { name, kind });
        }
    }
    inputs.sort_by(|a, b| a.name.cmp(&b.name));
    inputs.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);
    inputs
}

pub fn touchpad_rows(devices: &PointingDevices, bindings: &[Binding]) -> Vec<PeripheralRow> {
    let mut rows = Vec::new();
    let mut groups: Vec<(Vec<&str>, &BTreeMap<String, String>)> = Vec::new();
    for pad in &devices.touchpads {
        if let Some((names, _)) = groups
            .iter_mut()
            .find(|(_, settings)| **settings == pad.settings)
        {
            names.push(&pad.name);
        } else {
            groups.push((vec![&pad.name], &pad.settings));
        }
    }
    for (names, settings) in groups {
        rows.push(PeripheralRow::new(names.join(" · "), "Touchpad controls"));
        let mut confirmed = false;
        for key in [
            "tap_to_click",
            "clickfinger_behavior",
            "natural_scroll",
            "left_handed",
        ] {
            let Some(value) = settings.get(key).and_then(|v| boolean(v)) else {
                continue;
            };
            confirmed = true;
            let (label, action) = match key {
                "tap_to_click" => (
                    "Tap",
                    if value {
                        match settings.get("tap_button_map").map(String::as_str) {
                            Some("lmr") => "1 / 2 / 3 fingers: left / middle / right click",
                            Some("lrm" | "" | "[[EMPTY]]") => {
                                "1 / 2 / 3 fingers: left / right / middle click"
                            }
                            _ => "Tap to click enabled; mapping unavailable",
                        }
                    } else {
                        "Tap to click disabled"
                    },
                ),
                "clickfinger_behavior" => (
                    "Press",
                    if value {
                        "1 / 2 / 3 fingers: left / right / middle click"
                    } else {
                        "Click position selects the button"
                    },
                ),
                "natural_scroll" => (
                    "Scrolling direction",
                    if value {
                        "Natural scrolling"
                    } else {
                        "Standard scrolling"
                    },
                ),
                _ => (
                    "Button handedness",
                    if value {
                        "Left and right buttons swapped"
                    } else {
                        "Standard button mapping"
                    },
                ),
            };
            rows.push(PeripheralRow::new(label, action));
        }
        if let Some(method) = settings
            .get("scroll_method")
            .filter(|v| !v.is_empty() && *v != "[[EMPTY]]")
        {
            confirmed = true;
            rows.push(PeripheralRow::new(
                "Scroll",
                match method.as_str() {
                    "2fg" | "two_finger" => "Two-finger scrolling",
                    "edge" => "Edge scrolling",
                    "on_button_down" => "Scroll while holding the scroll button",
                    "no_scroll" => "Scrolling disabled",
                    other => other,
                },
            ));
        }
        if !confirmed {
            rows.push(PeripheralRow::new(
                "Settings unavailable",
                "Could not read touchpad behavior",
            ));
        }
    }
    let controls = std::mem::take(&mut rows);
    let mut shared: Vec<_> = bindings
        .iter()
        .filter_map(|b| {
            let label = match b.key.as_str() {
                "MOUSE_LEFT" => "Left click",
                "MOUSE_RIGHT" => "Right click",
                "MOUSE_MIDDLE" => "Middle click",
                "MOUSE_UP" => "Scroll up",
                "MOUSE_DOWN" => "Scroll down",
                "MOUSE_SCROLL_LEFT" => "Scroll left",
                "MOUSE_SCROLL_RIGHT" => "Scroll right",
                _ => return None,
            };
            Some(PeripheralRow::new(
                chord_label(b.modifiers, label),
                b.action.clone(),
            ))
        })
        .collect();
    shared.sort_by(|a, b| a.label.cmp(&b.label).then(a.action.cmp(&b.action)));
    shared.dedup();
    for gesture in &devices.gestures.bindings {
        rows.push(PeripheralRow::new(
            chord_label(
                gesture.modifiers,
                &format!("{} fingers · {}", gesture.fingers, gesture.direction),
            ),
            &gesture.action,
        ));
    }
    match devices.gestures.status {
        DiscoveryStatus::Unavailable => rows.push(PeripheralRow::new(
            "Gestures unavailable",
            "Could not read the active configuration",
        )),
        DiscoveryStatus::Incomplete => rows.push(PeripheralRow::new(
            "Some gestures could not be read",
            "Dynamic configuration is not shown",
        )),
        DiscoveryStatus::Complete if devices.gestures.bindings.is_empty() => rows.push(
            PeripheralRow::new("No configured gestures", "No gesture declarations found"),
        ),
        DiscoveryStatus::Complete => {}
    }
    rows.extend(shared);
    rows.extend(controls);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handedness_uses_consensus_then_global_then_right() {
        assert!(mouse_side(Some(false), &[Some(true), Some(true)]));
        assert!(!mouse_side(Some(false), &[Some(true), Some(false)]));
        assert!(mouse_side(Some(true), &[None, Some(false)]));
        assert!(!mouse_side(None, &[None]));
        assert!(!mouse_side(None, &[]));
    }
    #[test]
    fn touchpad_detection_excludes_touchscreens_and_virtual_inputs() {
        let empty = BTreeSet::new();
        assert_eq!(
            classify_input(
                "pad",
                "E:ID_INPUT_TOUCHPAD=1",
                &empty,
                &empty,
                &empty,
                &empty
            ),
            Some(InputKind::Touchpad)
        );
        assert_eq!(
            classify_input(
                "pad",
                "",
                &BTreeSet::from([325, 330]),
                &BTreeSet::from([0, 1]),
                &empty,
                &BTreeSet::from([0])
            ),
            Some(InputKind::Touchpad)
        );
        assert_eq!(
            classify_input(
                "screen",
                "E:ID_INPUT_TOUCHSCREEN=1",
                &BTreeSet::from([325]),
                &BTreeSet::from([0, 1]),
                &empty,
                &empty
            ),
            None
        );
        assert_eq!(
            classify_input(
                "virtual pad",
                "E:ID_INPUT_TOUCHPAD=1",
                &empty,
                &empty,
                &empty,
                &empty
            ),
            None
        );
        assert_eq!(
            classify_input("unknown", "", &empty, &empty, &empty, &empty),
            None
        );
        assert_eq!(
            classify_input(
                "mouse",
                "",
                &BTreeSet::from([272]),
                &empty,
                &BTreeSet::from([0, 1]),
                &empty
            ),
            Some(InputKind::Mouse)
        );
    }
    #[test]
    fn options_support_legacy_and_lua_without_inventing_defaults() {
        assert_eq!(
            option_value(&serde_json::json!({"bool": false})),
            Some("false".into())
        );
        assert_eq!(
            option_value(&serde_json::json!({"int": 1})),
            Some("1".into())
        );
        assert_eq!(option_value(&serde_json::json!({"error": "unknown"})), None);
    }
    #[test]
    fn touchpad_rows_group_settings_keep_chords_and_exclude_side_buttons() {
        let devices = PointingDevices {
            touchpads: vec![
                Touchpad {
                    name: "a".into(),
                    ..Default::default()
                },
                Touchpad {
                    name: "b".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let bindings = vec![
            Binding {
                modifiers: Modifiers::SUPER,
                key: "MOUSE_UP".into(),
                action: "Previous workspace".into(),
            },
            Binding {
                modifiers: Modifiers::SUPER,
                key: "MOUSE:275".into(),
                action: "Extra".into(),
            },
        ];
        let rows = touchpad_rows(&devices, &bindings);
        assert!(rows.iter().any(|r| r.label == "a · b"));
        assert!(rows.iter().any(|r| r.label == "Settings unavailable"));
        assert!(rows.iter().any(|r| r.label == "SUPER + Scroll up"));
        assert!(!rows.iter().any(|r| r.action == "Extra"));
        assert!(rows.iter().any(|r| r.label == "Gestures unavailable"));
    }
}
