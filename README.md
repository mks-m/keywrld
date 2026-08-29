# keywrld

`keywrld` is a keyboard-shaped shortcut overlay for Omarchy. It reads the live
shortcut descriptions from `omarchy menu keybindings --print`, follows the
current Omarchy color palette, and updates when Shift, Ctrl, or Alt is held
alongside the implicit Super modifier.

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

