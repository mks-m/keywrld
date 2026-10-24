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

## Shortcut search

The separate search panel initially lists all registered keyboard and mouse
bindings across every modifier combination, including keys absent from the
detected layout. Type immediately to fuzzy-search action descriptions, key
names, and modifier labels. Each word narrows the results; exact and contiguous
matches appear before scattered matches.

Holding or latching modifiers filters the list to that exact combination.
Releasing held modifiers and clearing latched modifiers restores all combinations.
Clear the search field to remove the text filter. Scroll over the results
independently of the touchpad panel, and hover a row for its full chord and
description. Results provide shortcut help; clicking them does not run actions.
In landscape layouts, search stays outermost left when the mouse diagram is also on the left.

### Search appearance

Search styling refreshes whenever the overlay opens. It reads Omarchy's current
`colors.toml` and `shell.toml` under `$XDG_STATE_HOME/omarchy/current/theme`
(default `~/.local/state/omarchy/current/theme`). Menu and control colors, font
sizes, and input spacing take priority; canonical `foreground` wins over
`bright_foreground`. Omarchy's `monospace` fontconfig alias supplies the font.

Live Hyprland rounding and border width fill the geometry settings; its active
border color fills a missing theme border. Remaining colors and fonts come from
GTK's active desktop theme, then built-in defaults. Border gradients use their
first color. The reader supports the scalar theme values used by search,
including color references and alpha values; unsupported or malformed values
fall back without preventing the overlay from opening. It does not modify any
system settings. Keyboard, mouse, and touchpad styling is unchanged.

## Responsive layout

The overlay uses the screen's available size after display scaling. Wide screens
keep search, keyboard and mouse side by side. When this would shrink keyboard
units below 32 logical pixels, the keyboard moves above a row of search, touchpad
and mouse panels, with a shorter header and footer. Search stays outermost left;
a left-handed mouse moves ahead of the touchpad.

Portrait screens put the keyboard across the top, with search beside a column of
mouse and touchpad panels below. That lower arrangement is mirrored for a
left-handed mouse. Missing devices release their space to the remaining panels.
Layouts adapt on resize while preserving the search and selected modifiers;
search and touchpad lists continue to scroll independently.

The readability target is 1280×720 landscape and 720×1280 portrait logical pixels
or larger. Smaller areas receive best-effort fitting. Keyboard diagrams grow up
to 78 logical pixels per unit; display scaling handles high-density screens.

## Pointing-device panels

Mouse bindings appear on a symbolic mouse in a separate panel, with 24-pixel
gaps between panels. On wide screens it sits beside the keyboard. Left/right buttons, wheel click and scroll directions
show action icons (or short text) for the exact selected modifier chord. Hover a
control for its full chord and actions. Thumb buttons and horizontal scrolling
appear when configured, and the diagram stays in place when no bindings match.
The panel moves to the left when the detected mice agree on
left-handed operation. Conflicting or unavailable device settings fall back to
Hyprland's global `input.left_handed` setting, then to the right.

A detected touchpad adds a panel, centered below the keyboard on wide screens. It lists configured
gestures, shared click/scroll bindings, and confirmed tap, clickfinger, button
mapping and scrolling settings. Devices with identical settings share a controls
group. The touchpad list keeps all chords visible; scroll over it to see remaining
rows and hover a row for its complete text. The mouse diagram follows held and
latched modifiers together with the keyboard. Clicking the backdrop, including
the gaps, closes the overlay.

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
