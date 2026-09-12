# keywrld

`keywrld` is a keyboard-shaped system shortcut overlay for Hyprland and
Omarchy. It reads Hyprland's live binding registry, using Omarchy only to
recover keys omitted from Lua-backed registry entries. App-local shortcuts and
unmodified dedicated keys whose function is already printed on the keyboard
are left out.

The view follows the modifiers that are actually held, so bindings without
Super are visible too. Shift, Ctrl, and Alt can also be clicked to keep them
selected; Super is never latched. A mouse diagram beside the keyboard follows
the same modifier selection, highlighting its bound buttons and scroll wheel.

## Runtime requirements

- Omarchy 4 / Hyprland on Wayland
- GTK 4
- GTK4 Layer Shell

## Build

```sh
cargo build --release
install -Dm755 target/release/keywrld ~/.local/bin/keywrld
```

The current machine uses this user keybinding override:

```lua
hl.unbind("SUPER + K")
o.bind("SUPER + K", "Keyboard shortcuts", { launch = "keywrld" })
```

Hyprland configuration changes should be checked with:

```sh
hyprctl reload
hyprctl configerrors
```

## Usage

```text
keywrld [--layout auto|full|100|tkl|80|75|65|60]
```

Device detection prefers an exact USB profile, then model-name hints, then
input capabilities. `--layout` is available when hardware metadata is
ambiguous. Escape, a backdrop click, or invoking the app again closes it.

## Pointing-device panels

Mouse bindings appear on a symbolic mouse in a separate panel, with a 24-pixel
gap beside the keyboard. Left/right buttons, wheel click and scroll directions
show action icons (or short text) for the exact selected modifier chord. Hover a
control for its full chord and actions. Thumb buttons and horizontal scrolling
appear when configured, and the diagram stays in place when no bindings match. The panel moves to the left when the detected mice agree on
left-handed operation. Conflicting or unavailable device settings fall back to
Hyprland's global `input.left_handed` setting, then to the right.

A detected touchpad adds a centered panel below the keyboard. It lists configured
gestures, shared click/scroll bindings, and confirmed tap, clickfinger, button
mapping and scrolling settings. Devices with identical settings share a controls
group. The touchpad list keeps all chords visible; scroll over it to see remaining
rows and hover a row for its complete text. The mouse diagram follows held and
latched modifiers together with the keyboard. Clicking the backdrop, including the gaps, closes the overlay.

Detection refreshes each time the overlay opens. Hyprland's device registry is
matched to Linux input metadata; touchpads use udev classification with a sysfs
capability fallback. Touchscreens and virtual input devices are excluded. Mouse
bindings remain available even when hardware metadata cannot be read.

Gesture discovery reads configuration files without executing them. Supported
forms are literal Lua `hl.gesture({ ... })` and `hl.device({ ... })` calls, plus
legacy `gesture = ...` and `device { ... }` declarations. Literal `require(...)`,
`dofile(...)` and legacy `source = ...` includes are followed in order; legacy
source paths support variables and `*`/`?` globs. Lua module lookup includes the
XDG state/config directories, `OMARCHY_PATH` (default `/usr/share/omarchy`), and the
main configuration's directory. `require` loads a module once. Gesture `unset`
entries match fingers, direction, modifiers and scale. Callback actions are
labeled “Custom action.”

Conditional registrations, computed declarations, unresolved includes and custom
Lua module-loading behavior cannot be reliably reconstructed. The panel flags
incomplete discovery separately from unavailable configuration and a successfully
read configuration with no gestures. Settings use live global options plus
resolvable static device overrides; changes made dynamically outside those files
may not be represented. App-local and third-party gesture-daemon bindings are
outside this reader's scope. No compositor settings are changed.

For tests and optional Cairo previews (no touchpad or graphical session needed):

```sh
cargo test
KEYWRLD_RENDER_DIR=/tmp/keywrld-panels cargo test render_panel_fixtures
```
