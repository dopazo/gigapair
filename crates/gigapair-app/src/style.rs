//! The look of the app: the bundled Archivo font, the light and dark themes
//! (`assets/theme.json`), the app's own glyphs, and the few colours the
//! component theme has no slot for.
//!
//! One colour carries meaning: signal yellow marks verified data and the
//! primary action. Everything else is ink on paper (light) or paper on
//! graphite (dark).

use std::borrow::Cow;
use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeRegistry};
use gpui_kit::{App, AssetSource, Hsla, SharedString, rgb};

/// Amounts and the wordmark: Archivo Expanded ExtraBold, so data reads
/// first. Renamed to a family of its own (see `assets/fonts/README.md`):
/// GPUI picks fonts by family and weight only, not width.
pub const FONT_WIDE: &str = "Archivo Display";
/// Page titles: Archivo SemiExpanded ExtraBold, renamed the same way.
pub const FONT_SEMIWIDE: &str = "Archivo Heading";

/// Archivo (SIL Open Font License 1.1, `assets/fonts/OFL.txt`), static cuts.
const FONTS: [&[u8]; 6] = [
    include_bytes!("../assets/fonts/Archivo-Regular.ttf"),
    include_bytes!("../assets/fonts/Archivo-Medium.ttf"),
    include_bytes!("../assets/fonts/Archivo-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Archivo-ExtraBold.ttf"),
    include_bytes!("../assets/fonts/ArchivoHeading-Regular.ttf"),
    include_bytes!("../assets/fonts/ArchivoDisplay-Regular.ttf"),
];

const THEMES: &str = include_str!("../assets/theme.json");
const LIGHT: &str = "Gigapair Light";
const DARK: &str = "Gigapair Dark";

/// The app's glyphs, drawn in one colour each (GPUI tints SVGs with the text
/// colour), so two-colour marks are two stacked SVGs.
pub mod icon {
    pub const MARK_G: &str = "gigapair/mark-g.svg";
    pub const MARK_BLOCK: &str = "gigapair/mark-block.svg";
    pub const DIR_UP: &str = "gigapair/dir-up.svg";
    pub const DIR_DOWN: &str = "gigapair/dir-down.svg";
    pub const DIR_BLOCK: &str = "gigapair/dir-block.svg";
    pub const LINK_DIRECT: &str = "gigapair/link-direct.svg";
    pub const LINK_RELAY: &str = "gigapair/link-relay.svg";
    pub const CELL: &str = "gigapair/cell.svg";
    pub const NAV_HISTORY: &str = "gigapair/nav-history.svg";
    pub const NAV_SETTINGS: &str = "gigapair/nav-settings.svg";
    pub const NAV_ADD: &str = "gigapair/nav-add.svg";
    pub const AWAKE: &str = "gigapair/awake.svg";
}

const GLYPHS: [(&str, &[u8]); 12] = [
    (icon::MARK_G, include_bytes!("../assets/icons/mark-g.svg")),
    (
        icon::MARK_BLOCK,
        include_bytes!("../assets/icons/mark-block.svg"),
    ),
    (icon::DIR_UP, include_bytes!("../assets/icons/dir-up.svg")),
    (
        icon::DIR_DOWN,
        include_bytes!("../assets/icons/dir-down.svg"),
    ),
    (
        icon::DIR_BLOCK,
        include_bytes!("../assets/icons/dir-block.svg"),
    ),
    (
        icon::LINK_DIRECT,
        include_bytes!("../assets/icons/link-direct.svg"),
    ),
    (
        icon::LINK_RELAY,
        include_bytes!("../assets/icons/link-relay.svg"),
    ),
    (icon::CELL, include_bytes!("../assets/icons/cell.svg")),
    (
        icon::NAV_HISTORY,
        include_bytes!("../assets/icons/nav-history.svg"),
    ),
    (
        icon::NAV_SETTINGS,
        include_bytes!("../assets/icons/nav-settings.svg"),
    ),
    (icon::NAV_ADD, include_bytes!("../assets/icons/nav-add.svg")),
    (icon::AWAKE, include_bytes!("../assets/icons/awake.svg")),
];

/// The component library's assets plus the app's glyphs.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = GLYPHS.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(
            GLYPHS
                .iter()
                .map(|(p, _)| *p)
                .filter(|p| p.starts_with(path))
                .map(SharedString::from),
        );
        Ok(paths)
    }
}

/// Registers the fonts and installs the themes. Call once, after
/// `gpui_kit::init`. On failure the app keeps the default look.
pub fn install(cx: &mut App) {
    let fonts = FONTS.iter().map(|f| Cow::Borrowed(*f)).collect();
    let fonts_ok = match cx.text_system().add_fonts(fonts) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("could not load the Archivo font: {e:#}");
            false
        }
    };
    let registry = ThemeRegistry::global_mut(cx);
    if let Err(e) = registry.load_themes_from_str(THEMES) {
        tracing::warn!("could not load the Gigapair theme: {e:#}");
        return;
    }
    let get = |name: &str| registry.themes().get(name).cloned();
    let (Some(light), Some(dark)) = (get(LIGHT), get(DARK)) else {
        tracing::warn!("the Gigapair theme file has no light or dark theme");
        return;
    };
    let strip_font = |t: Rc<ThemeConfig>| {
        if fonts_ok {
            t
        } else {
            let mut t = (*t).clone();
            t.font_family = None;
            Rc::new(t)
        }
    };
    let (light, dark) = (strip_font(light), strip_font(dark));
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    let appearance = cx.window_appearance();
    Theme::change(appearance, None, cx);
}

/// Colours the component theme has no slot for, per mode.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Paper: the window background.
    pub bg: Hsla,
    /// Raised: buttons, the drop zone, the selected contact.
    pub surface: Hsla,
    /// Below the page: tags, the title bar.
    pub sunken: Hsla,
    pub ink: Hsla,
    pub ink_2: Hsla,
    pub ink_3: Hsla,
    pub line: Hsla,
    pub signal: Hsla,
    pub signal_ink: Hsla,
    pub danger: Hsla,
    /// Danger text on the inverted band.
    pub danger_on_band: Hsla,
    pub cell_empty: Hsla,
    pub cell_flight: Hsla,
}

impl Palette {
    pub fn get(cx: &App) -> Self {
        let dark = Theme::global(cx).is_dark();
        let c = |v: u32| -> Hsla { rgb(v).into() };
        let (signal, signal_ink, danger) = (c(0xFFC929), c(0x1A1712), c(0xE5484D));
        if dark {
            Self {
                bg: c(0x1C1D21),
                surface: c(0x25262B),
                sunken: c(0x17181B),
                ink: c(0xF1F2F4),
                ink_2: c(0xA9ADB6),
                ink_3: c(0x737882),
                line: c(0x34363D),
                signal,
                signal_ink,
                danger,
                danger_on_band: c(0xC9282E),
                cell_empty: c(0x303238),
                cell_flight: c(0x6B5A1F),
            }
        } else {
            Self {
                bg: c(0xF3F4F6),
                surface: c(0xFFFFFF),
                sunken: c(0xE9EBEF),
                ink: c(0x17181C),
                ink_2: c(0x565A64),
                ink_3: c(0x8A8F99),
                line: c(0xDCDFE5),
                signal,
                signal_ink,
                danger,
                danger_on_band: c(0xFF8A8D),
                cell_empty: c(0xE2E5EA),
                cell_flight: c(0xFFE58F),
            }
        }
    }
}
