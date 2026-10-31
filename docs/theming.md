# Themes

Every colour and size the UI draws with comes from a theme, and a theme is a
file. Pick one in *Options → Appearance*; after editing a theme file, press
*Reload themes* there to see the change without restarting.

Three themes are built in: **Glossa Dark**, **Glossa Light** and **Glossa
Midnight**. Themes from a classic QTranslate installation appear alongside
them (see [modules/classic-scripts.md](modules/classic-scripts.md)).

## Where theme files go

`<data folder>/themes/` (for example `%APPDATA%\Glossa\themes`), or `themes/`
next to the executable. A file's name, without the extension, is the theme's
name. `.toml` and `.json` are read.

## Writing one

Start from an existing theme and change only what you want:

```toml
# themes/Ocean.toml
extends = "Glossa Dark"
accent = "#1fa2a6"
selection = "#1fa2a670"

[window]
back = "#0f1c24"
client_back = "#0a141a"

[popup]
back = "#0f1c24"
frame = "#1fa2a6"
opacity = 0.9

[metrics]
button_radius = 6.0
window_radius = 8.0
```

`extends` names any theme the program knows (built-in, classic, or another
file). To see every field with its current value:

```bash
glossa-cli theme show "Glossa Dark"          # the full theme as TOML
glossa-cli theme show "Glossa Dark" > themes/Mine.toml
```

### Colours

`#rrggbb` or `#rrggbbaa` (alpha last: `#2a8de470` is the accent at 44 %).
The classic shorthand is accepted too: `"b"` is `#bbbbbb`, `"57"` is
`#575757`, `"39f"` is `#3399ff`.

### Sections

| section | what it styles |
|---|---|
| top level | `dark` (start from dark or light defaults), `accent`, `accent_text`, `link`, `selection`, `error`, `warning` |
| `[window]` | window background, text, borders, the text panes' background (`client_back`), group frames |
| `[button.normal]`, `.hover`, `.pressed`, `.disabled` | buttons: `back_top` / `back_bottom` (classic themes' gradient ends; drawn as their mix), `border`, `text` |
| `[edit.normal]`, `.hover`, `.focused`, `.disabled` | text fields |
| `[window_button.*]` | minimise / maximise / close in the title bar: `main` (the glyph), `back` |
| `[icon.*]` | toolbar and pane icons |
| `[popup]` | the selection popup: `back`, `text`, `frame`, `frame_thickness`, `opacity` |
| `[metrics]` | `button_radius`, `window_radius`, `spacing`, `padding`, `border_width`, `service_tab_height`, `icon_size` |
| `[fonts]` | `family`, `size` (see below) |

A state that is not given falls back to `normal`.

### Fonts

`family` is the interface font. On Windows it is looked up by file name in
the Windows fonts folder, spaces ignored: `"Segoe UI"` finds `segoeui.ttf`,
and for a font whose file is named differently, give the file's stem
(`"consola"` for Consolas). On Linux and macOS it goes through fontconfig, so
any installed family name works. Empty means Segoe UI on Windows and egui's
built-in face elsewhere. `size` is the interface size; the text panes' size
is a setting (*Options → Appearance*), not part of the theme.

Scripts the chosen font lacks (CJK, Arabic, Indic, Thai…) are filled from
system fonts automatically, each loaded only when such text first appears.
`text_family`, `text_size` and `fallbacks` are read but not used yet.

## Transparency

Two settings in *Options → Appearance*, independent of the theme:

- **Window transparency** for the main window and the other windows;
- **Popup transparency** for the selection popup (the theme's
  `popup.opacity` is the starting point).

## Classic themes

Classic theme files (`Common`, `Window`, `Button`, `Edit`, `WindowButton`,
`Icon` sections, with `Normal` / `Hover` / `Pressed` / `Disabled` states and
`//` comments) load unchanged. To turn one into an editable native theme:

```bash
glossa-cli theme convert "C:\…\QTranslate\Themes\Flat Dark.json" > themes/Flat Dark.toml
```
