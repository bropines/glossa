# Fluent (WinUI 3) numbers

What the **Fluent Dark** and **Fluent Light** themes take from Microsoft's
design guidance, and where each value comes from. Colours are WinUI's theme
resources (`Common_themeresources_any.xaml` in microsoft-ui-xaml, MIT); a
translucent token is given composited over the theme's base colour, which is
what the theme uses without a backdrop (with one, `[backdrop]` uses the
translucent token itself).

Sources:

- [Typography](https://learn.microsoft.com/windows/apps/design/signature-experiences/typography),
  [Geometry](https://learn.microsoft.com/windows/apps/design/signature-experiences/geometry),
  [Layering and elevation](https://learn.microsoft.com/windows/apps/design/signature-experiences/layering),
  [Materials](https://learn.microsoft.com/windows/apps/design/signature-experiences/materials),
  [Content layout and spacing](https://learn.microsoft.com/windows/apps/design/basics/content-basics),
  [Color](https://learn.microsoft.com/windows/apps/design/signature-experiences/color)
- [`Common_themeresources_any.xaml`](https://github.com/microsoft/microsoft-ui-xaml/blob/main/src/controls/dev/CommonStyles/Common_themeresources_any.xaml)
  and the per-control `*_themeresources.xaml` next to it (Button, ComboBox,
  CheckBox, Slider, ScrollBar, MenuFlyout)
- [Fluent 2 elevation](https://fluent2.microsoft.design/elevation) for shadow
  values

## Type

| ramp | size / line | Glossa |
|---|---|---|
| Caption | 12 / 16, Small | `TextStyle::Small` = body × 12/14 |
| Body | 14 / 20, Text | `fonts.size = 10.5` (× 1.33 = 14 px): `Body`, `Button` |
| Body Strong | 14 / 20, Text semibold | not used (egui has no per-text weight in `RichText`) |
| Subtitle | 20 / 28, Display semibold | `TextStyle::Heading` = body × 20/14 |
| Title | 28 / 36 | not used |

Font: **Segoe UI Variable** (`SegUIVar.ttf`, axes `wght` 300–700 default 400,
`opsz` 5–36 default 10.5: the default instance is the *Text* optical size at
regular weight, which is what 14 px body text uses), then **Segoe UI** where
it is missing (Windows 10). Looked up on the system, never bundled. Segoe UI
Variable covers Latin, Greek and Cyrillic only; Arabic, Hebrew, Armenian and
Georgian then come from Segoe UI through the per-script fallback.

## Geometry and sizes

| what | WinUI | Glossa |
|---|---|---|
| controls (Button, ComboBox, TextBox, CheckBox, list items) | `ControlCornerRadius` 4 | `metrics.button_radius = 4` |
| windows, flyouts, dialogs, menus | `OverlayCornerRadius` 8 | `window_radius = 8`, `overlay_radius = 8` (egui's menu radius) |
| standard control height | 32 (compact 24) | `metrics.control_height = 32` → `interact_size.y` |
| button padding | `ButtonPadding` 11,5,11,6; `ComboBoxPadding` 12,5,0,7 | 10 × 5 (one px less across, so the main window's language row fits its default width) |
| stroke | 1 px everywhere (`ButtonBorderThemeThickness`, `TextControlBorderThemeThickness`) | `border_width = 1` |
| spacing | 8 between controls, 12 between areas, 16 to a surface's edge | `item_spacing` 8 × 4 (rows of 32 px controls), `spacing = 8` |
| check box | `CheckBoxSize` 20, glyph 12 | 16 / 10: egui sizes combo box arrows from the same setting, and 20 makes them twice WinUI's chevron |
| slider | track `SliderTrackThemeHeight` 4, thumb 18 | `slider_rail_height = 4`, track before the thumb accent-filled |
| scroll bar | 12 px bar, collapsed 2 px indicator, 6 px thumb, 2 px from the edge, thumb ≥ 30 | floating bar: `floating_width` 2, `bar_width` 6, margins 2, `handle_min_length` 30 |
| menu | presenter padding 2, items 4 px from the edge | `menu_margin` 4 |
| states | fill changes only | `expansion = 0` (egui grows widgets 1 px under the pointer by default) |
| caret | 1 px, text colour | `text_cursor.stroke` 1 px, text colour |

## Elevation

Window and dialog 128, flyout 32, tooltip 16, card 8, control 2, all with a
1 px stroke. egui has two shadows; they take Fluent 2's key shadows:

| egui | Fluent 2 | value |
|---|---|---|
| `popup_shadow` (menus, combo lists, tooltips) | shadow16: `0 8 16` at 14 % (light), 28 % (dark) | offset 0,8, blur 16, black α 36 / 71 |
| `window_shadow` | shadow28: `0 14 28` at 14 % / 28 % | offset 0,14, blur 28, same alpha |

## Colour tokens

Dark over `SolidBackgroundFillColorBase` #202020, light over #F3F3F3.

| token | dark | light | used for |
|---|---|---|---|
| SolidBackgroundFillColorBase | #202020 | #F3F3F3 | `window.back` |
| LayerFillColorDefault | #4C3A3A3A → #282828 | #80FFFFFF → #F9F9F9 | `client_back` (panes) |
| ControlFillColorDefault | #0FFFFFFF → #2D2D2D | #B3FFFFFF → #FBFBFB | button / field rest |
| ControlFillColorSecondary | #15FFFFFF → #323232 | #80F9F9F9 → #F6F6F6 | hover |
| ControlFillColorTertiary | #08FFFFFF → #272727 | #4DF9F9F9 → #F5F5F5 | pressed |
| ControlFillColorDisabled | #0BFFFFFF → #2A2A2A | #4DF9F9F9 → #F5F5F5 | disabled |
| ControlFillColorInputActive | #B31E1E1E → #1F1F1F | #FFFFFF | focused text field |
| ControlStrokeColorDefault | #12FFFFFF → #3C3C3C | #0F000000 → #ECECEC | control stroke |
| ControlStrokeColorSecondary | #18FFFFFF → #414141 | #29000000 → #D3D3D3 | the bottom edge; light uses #E5E5E5 for one stroke standing for both |
| SurfaceStrokeColorDefault | #66757575 | #66757575 | window and popup frame |
| DividerStrokeColorDefault | #15FFFFFF → #323232 | #0F000000 → #E5E5E5 (the theme uses #E0E0E0, a touch stronger) | separators, group frames |
| SubtleFillColorSecondary / Tertiary | #0FFFFFFF / #0AFFFFFF | #09000000 / #06000000 | caption button hover / pressed |
| TextFillColorPrimary | #FFFFFF | #E4000000 → #1A1A1A | text |
| TextFillColorSecondary | #C5FFFFFF → #CCCCCC | #9E000000 → #5C5C5C | icons; weak text (hints) is base→text at 75 % |
| TextFillColorDisabled | #5DFFFFFF → #717171 | #5C000000 → #9B9B9B | disabled text |
| AcrylicInAppFillColorDefault (tint) | #2C2C2C | #FCFCFC | the selection popup |
| SystemFillColorCritical / Caution | #FF99A4 / #FCE100 | #C42B1C / #9D5D00 | error / warning |
| SmokeFillColorDefault | #4D000000 | #4D000000 | (not used: no modal dialogs) |
| accent fill / text | AccentPalette light 2 / light 3 | dark 1 / dark 2 | `accent` / `link`, from the system accent |

## Materials

Mica (long-lived windows; Mica Alt behind tabs), Acrylic (transient
surfaces: flyouts, the popup), Smoke (behind modal dialogs). Windows draws
Mica and the 22H2 Acrylic only while the window is active. How Glossa asks
for each is in [../theming.md](../theming.md#backdrops); with a backdrop the
Fluent themes draw panes with LayerFillColorDefault and fields with
ControlFillColorDefault as the translucent tokens, and the window body not
at all.

## What egui cannot match

- WinUI's control stroke is a gradient (`ControlElevationBorderBrush`: a
  darker bottom pixel); egui strokes are one colour.
- A focused WinUI text field keeps its grey stroke and adds a 2 px accent
  line along the bottom; egui draws the whole outline in the accent.
- Checked check boxes and slider thumbs are accent-filled in WinUI; egui
  draws them with the widget fill and a check mark or plain thumb.
- Keyboard focus is a 2 px outer / 1 px inner ring (FocusStrokeColorOuter /
  Inner); egui shows focus with the pressed style.
- Combo boxes show a chevron glyph (Segoe Fluent Icons, 12 px); egui draws a
  filled triangle.
