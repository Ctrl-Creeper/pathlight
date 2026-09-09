//! What it takes for one window to look like it belongs on the system it is on.
//!
//! egui draws every pixel itself, so the platform's own UI font is the one
//! thing that has to be fetched by hand. Until it is, the window is legible but
//! foreign — and a file named in Chinese renders as empty boxes, because none of
//! the bundled fonts carry a CJK glyph.
//!
//! Dark and light mode need nothing here: following the system is egui's
//! default `ThemePreference`, and eframe forwards the system's preference. The
//! title bar and DPI are winit's, so they are already the platform's own.
//!
//! ponytail: no Mica or acrylic backdrop. egui paints its panels opaque, so a
//! translucent window would show through nothing; it needs a transparent
//! viewport and every panel frame reworked. Add it if the window has to blend
//! into a Windows 11 desktop, not before.

use std::path::Path;

use eframe::egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use eframe::egui::{Context, FontData, FontFamily};

/// A font file to look for.
pub struct SystemFont {
    pub path: &'static str,
    /// True for the platform's own UI font, which should be preferred over the
    /// bundled one. False for a fallback consulted only for glyphs no earlier
    /// font has — a CJK face, usually.
    pub leads: bool,
}

/// The files worth trying on this platform, in order.
///
/// Windows 11 ships Segoe UI Variable and every Windows before it Segoe UI;
/// Microsoft YaHei is the CJK face. A Linux distribution installs whatever it
/// likes, so those are candidates rather than a promise, and the bundled font
/// stays in place when none of them is there. macOS is not a target of this
/// shell — it has an app of its own — but the tests run there.
pub fn system_fonts() -> Vec<SystemFont> {
    let candidates: &[(&'static str, bool)] = if cfg!(windows) {
        &[
            ("C:/Windows/Fonts/SegUIVar.ttf", true),
            ("C:/Windows/Fonts/segoeui.ttf", true),
            ("C:/Windows/Fonts/msyh.ttc", false),
        ]
    } else if cfg!(target_os = "macos") {
        &[
            ("/System/Library/Fonts/SFNS.ttf", true),
            ("/System/Library/Fonts/Helvetica.ttc", true),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", false),
        ]
    } else {
        &[
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", true),
            (
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                false,
            ),
            (
                "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
                false,
            ),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", false),
        ]
    };
    candidates
        .iter()
        .map(|&(path, leads)| SystemFont { path, leads })
        .collect()
}

/// Installs every font in the list that is actually on this machine.
///
/// Only the first leading font is taken: two of them would fight over the same
/// glyphs, and the list is ordered by preference. Everything else is a fallback,
/// so an install with no CJK face still gets one if a later entry has it.
pub fn install(ctx: &Context, fonts: &[SystemFont]) {
    let mut led = false;
    for font in fonts {
        if font.leads && led {
            continue;
        }
        // A path that is not there is the normal case, not an error: this is a
        // list of what a platform might have installed.
        let Ok(data) = std::fs::read(font.path) else {
            continue;
        };
        let Some(name) = Path::new(font.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
        else {
            continue;
        };
        led |= font.leads;
        ctx.add_font(FontInsert {
            name: name.to_owned(),
            data: FontData::from_owned(data),
            families: vec![InsertFontFamily {
                family: FontFamily::Proportional,
                priority: if font.leads {
                    FontPriority::Highest
                } else {
                    FontPriority::Lowest
                },
            }],
        });
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::FontId;

    use super::*;

    fn rendered(ctx: &Context) {
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.label("下载/安装包.dmg");
        });
        // Nothing here uploads textures to a GPU, and epaint panics on a
        // delta that is dropped rather than handled.
        output.textures_delta.clear();
    }

    /// A font is loaded by path, and a path is a thing that moves between
    /// releases of an operating system. Linux is exempt: a distribution ships
    /// the fonts it wants to, and this shell has to survive either way.
    #[test]
    #[cfg_attr(target_os = "linux", ignore = "distribution-dependent")]
    fn the_platform_ui_font_is_where_this_says_it_is() {
        let fonts = system_fonts();
        assert!(
            fonts
                .iter()
                .any(|font| font.leads && Path::new(font.path).exists()),
            "no UI font candidate exists: {:?}",
            fonts.iter().map(|font| font.path).collect::<Vec<_>>()
        );
    }

    /// The point of loading a system font at all: a file named in Chinese has
    /// to read as its name and not as a row of boxes. A machine with no CJK
    /// font installed cannot render one either way — a stripped Windows Server
    /// image is the usual case — so that is a skip, not a failure.
    #[test]
    fn a_chinese_file_name_has_glyphs_to_render_with() {
        let fonts = system_fonts();
        if !fonts
            .iter()
            .any(|font| !font.leads && Path::new(font.path).exists())
        {
            eprintln!("skipped: no CJK font on this machine");
            return;
        }

        let ctx = Context::default();
        install(&ctx, &system_fonts());
        rendered(&ctx);

        let covered =
            ctx.fonts_mut(|fonts| fonts.has_glyphs(&FontId::proportional(14.0), "下载安装包"));
        assert!(covered, "CJK file names would render as boxes");
    }

    /// Most of this list is absent on any given machine, including all of it.
    #[test]
    fn a_missing_font_file_leaves_the_bundled_ones_in_place() {
        let ctx = Context::default();
        install(
            &ctx,
            &[SystemFont {
                path: "/nowhere/no-such-ui-font.ttf",
                leads: true,
            }],
        );
        rendered(&ctx);
    }
}
