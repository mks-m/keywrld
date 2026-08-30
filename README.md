# keywrld

`keywrld` is a keyboard-shaped system shortcut overlay for Hyprland and
Omarchy. It reads Hyprland's live binding registry, using Omarchy only to
recover keys omitted from Lua-backed registry entries. App-local shortcuts and
unmodified dedicated keys whose function is already printed on the keyboard
are left out.

The view follows the modifiers that are actually held, so bindings without
Super are visible too. Shift, Ctrl, and Alt can also be clicked to keep them
selected; Super is never latched. Mouse bindings remain visible in their own
card beside the keyboard.

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
