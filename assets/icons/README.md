# Action icon atlas

`action-icons.png` is a monochrome alpha atlas with 64 px cells and eight
columns. Tile order is defined by `sources.txt` and mirrored by `ActionIcon` in
`src/icons.rs`. Lucide glyphs are rasterized to 48 px and brand glyphs to 42 px,
then centered in their cells. The application uses the atlas as a Cairo mask,
so every icon follows the active Omarchy palette.

Sources are pinned to:

- Lucide 1.37.0: <https://github.com/lucide-icons/lucide/releases/tag/1.37.0>
- Simple Icons 16.28.0: <https://github.com/simple-icons/simple-icons/releases/tag/16.28.0>

The atlas is generated from the named SVG files in those releases. See
`THIRD_PARTY_NOTICES.md` for licensing and trademark information.
