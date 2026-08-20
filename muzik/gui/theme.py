"""Shared visual theme and color tokens for the desktop interface.

One coherent dark theme keeps the interface consistent. It applies four
craft rules:

- Concentric rounding: outer surfaces (windows) use a larger radius than the
  frames (inputs, buttons) nested inside them.
- Consistent spacing: one padding and item-spacing scale everywhere.
- Elevation through color: windows, child panels, and frames step up a neutral
  ramp so nested surfaces read as raised, without extra borders.
- One accent: a single blue token marks section headers, primary actions, and
  selection, instead of scattered literal colors.
"""

from __future__ import annotations

import dearpygui.dearpygui as dpg


# Accent token. Import this instead of writing a literal color for headers.
ACCENT: tuple[int, int, int] = (122, 178, 255)

# Status tokens, shared by the settings and validation surfaces.
OK_COLOR: tuple[int, int, int] = (120, 200, 120)
FAIL_COLOR: tuple[int, int, int] = (230, 120, 120)
NA_COLOR: tuple[int, int, int] = (198, 198, 128)

# Neutral elevation ramp, dark to light.
_WINDOW_BG = (24, 26, 30)
_CHILD_BG = (30, 33, 38)
_FRAME_BG = (38, 42, 48)
_FRAME_BG_HOVERED = (47, 52, 60)
_FRAME_BG_ACTIVE = (55, 61, 70)
_BORDER = (56, 61, 69)
_TEXT = (228, 230, 234)
_TEXT_DISABLED = (128, 132, 140)

# Neutral button, the calm default so primary actions can stand out.
_BUTTON = (52, 57, 66)
_BUTTON_HOVERED = (64, 70, 80)
_BUTTON_ACTIVE = (72, 79, 90)

# Accent shades for the primary action and selection.
_ACCENT_BUTTON = (52, 96, 168)
_ACCENT_HOVERED = (66, 118, 200)
_ACCENT_ACTIVE = (44, 82, 148)
_HEADER = (52, 96, 168, 120)
_HEADER_HOVERED = (66, 118, 200, 160)
_HEADER_ACTIVE = (66, 118, 200, 210)

# Table surfaces, with subtle zebra striping.
_TABLE_HEADER_BG = (38, 42, 48)
_TABLE_BORDER_LIGHT = (48, 52, 60)
_ROW_BG = (0, 0, 0, 0)
_ROW_BG_ALT = (255, 255, 255, 10)


def build_theme() -> str | int:
    """Create the global theme and return its tag for ``bind_theme``."""
    with dpg.theme() as theme:
        with dpg.theme_component(dpg.mvAll):
            _apply_colors()
            _apply_styles()
    return theme


def apply_global_theme() -> None:
    """Bind the shared theme as the application default."""
    dpg.bind_theme(build_theme())


def bind_primary_button(item: str | int) -> None:
    """Mark one button as the primary action, in the accent color.

    A fresh theme is built each call. Theme item ids belong to the live
    DearPyGui context, so caching across ``create_context`` cycles would leave
    a stale id; the per-call cost is trivial for the few primary buttons.
    """
    core = dpg.mvThemeCat_Core
    with dpg.theme() as theme:
        with dpg.theme_component(dpg.mvButton):
            dpg.add_theme_color(dpg.mvThemeCol_Button, _ACCENT_BUTTON, category=core)
            dpg.add_theme_color(
                dpg.mvThemeCol_ButtonHovered, _ACCENT_HOVERED, category=core
            )
            dpg.add_theme_color(
                dpg.mvThemeCol_ButtonActive, _ACCENT_ACTIVE, category=core
            )
    dpg.bind_item_theme(item, theme)


def _apply_colors() -> None:
    color = dpg.add_theme_color
    core = dpg.mvThemeCat_Core
    color(dpg.mvThemeCol_WindowBg, _WINDOW_BG, category=core)
    color(dpg.mvThemeCol_ChildBg, _CHILD_BG, category=core)
    color(dpg.mvThemeCol_PopupBg, _CHILD_BG, category=core)
    color(dpg.mvThemeCol_MenuBarBg, _CHILD_BG, category=core)
    color(dpg.mvThemeCol_Border, _BORDER, category=core)
    color(dpg.mvThemeCol_Text, _TEXT, category=core)
    color(dpg.mvThemeCol_TextDisabled, _TEXT_DISABLED, category=core)

    color(dpg.mvThemeCol_FrameBg, _FRAME_BG, category=core)
    color(dpg.mvThemeCol_FrameBgHovered, _FRAME_BG_HOVERED, category=core)
    color(dpg.mvThemeCol_FrameBgActive, _FRAME_BG_ACTIVE, category=core)

    color(dpg.mvThemeCol_TitleBg, _WINDOW_BG, category=core)
    color(dpg.mvThemeCol_TitleBgActive, _CHILD_BG, category=core)
    color(dpg.mvThemeCol_TitleBgCollapsed, _WINDOW_BG, category=core)

    color(dpg.mvThemeCol_Button, _BUTTON, category=core)
    color(dpg.mvThemeCol_ButtonHovered, _BUTTON_HOVERED, category=core)
    color(dpg.mvThemeCol_ButtonActive, _BUTTON_ACTIVE, category=core)

    color(dpg.mvThemeCol_Header, _HEADER, category=core)
    color(dpg.mvThemeCol_HeaderHovered, _HEADER_HOVERED, category=core)
    color(dpg.mvThemeCol_HeaderActive, _HEADER_ACTIVE, category=core)

    color(dpg.mvThemeCol_CheckMark, ACCENT, category=core)
    color(dpg.mvThemeCol_SliderGrab, _ACCENT_HOVERED, category=core)
    color(dpg.mvThemeCol_SliderGrabActive, ACCENT, category=core)
    # DearPyGui draws the progress-bar fill with the histogram color.
    color(dpg.mvThemeCol_PlotHistogram, _ACCENT_HOVERED, category=core)

    color(dpg.mvThemeCol_TableHeaderBg, _TABLE_HEADER_BG, category=core)
    color(dpg.mvThemeCol_TableBorderStrong, _BORDER, category=core)
    color(dpg.mvThemeCol_TableBorderLight, _TABLE_BORDER_LIGHT, category=core)
    color(dpg.mvThemeCol_TableRowBg, _ROW_BG, category=core)
    color(dpg.mvThemeCol_TableRowBgAlt, _ROW_BG_ALT, category=core)


def _apply_styles() -> None:
    style = dpg.add_theme_style
    core = dpg.mvThemeCat_Core
    # Concentric rounding: window > child > frame.
    style(dpg.mvStyleVar_WindowRounding, 8, category=core)
    style(dpg.mvStyleVar_ChildRounding, 6, category=core)
    style(dpg.mvStyleVar_PopupRounding, 6, category=core)
    style(dpg.mvStyleVar_FrameRounding, 5, category=core)
    style(dpg.mvStyleVar_TabRounding, 5, category=core)
    style(dpg.mvStyleVar_GrabRounding, 4, category=core)
    style(dpg.mvStyleVar_ScrollbarRounding, 8, category=core)

    # One spacing scale.
    style(dpg.mvStyleVar_WindowPadding, 14, 12, category=core)
    style(dpg.mvStyleVar_FramePadding, 10, 6, category=core)
    style(dpg.mvStyleVar_CellPadding, 8, 5, category=core)
    style(dpg.mvStyleVar_ItemSpacing, 8, 8, category=core)
    style(dpg.mvStyleVar_ItemInnerSpacing, 8, 6, category=core)
    style(dpg.mvStyleVar_ScrollbarSize, 13, category=core)
    style(dpg.mvStyleVar_GrabMinSize, 12, category=core)

    # Hairline borders; elevation carries the structure.
    style(dpg.mvStyleVar_WindowBorderSize, 0, category=core)
    style(dpg.mvStyleVar_ChildBorderSize, 1, category=core)
    style(dpg.mvStyleVar_FrameBorderSize, 0, category=core)
