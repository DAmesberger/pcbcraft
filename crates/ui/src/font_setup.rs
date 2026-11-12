//! The host part of the interface type: the bundled IBM Plex faces, user overrides, and
//! language-specific system fallbacks (CJK, Hangul, Thai, Hebrew), installed under the names the
//! kit's type API gives (see [`crate::kit::type_face`]).
//! The same definitions are used on desktop and web; the web shell adds Japanese and Chinese
//! faces on demand.

use std::sync::Arc;

use egui::{FontDefinitions, FontFamily};

use crate::kit::type_face::{
    BOLD_FAMILY, CHINESE_FALLBACK, CJK_FALLBACK, HANGUL_FALLBACK, MEDIUM_FAMILY, SEMIBOLD_FAMILY, SYMBOL_FALLBACK, UI_FONT, fallback_order,
    needs_cjk_font, sans_families, semibold,
};
// The Thai, Hebrew and Arabic faces come only from the desktop's system font search (and the tests).
#[cfg(not(target_arch = "wasm32"))]
use crate::kit::type_face::{ARABIC_FALLBACK, HEBREW_FALLBACK, THAI_FALLBACK};

const PLEX_SANS: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf"));
const PLEX_MONO: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf"));
const WEIGHTS: [(&str, f32); 4] = [("plex-sans", 400.0), (MEDIUM_FAMILY, 500.0), (SEMIBOLD_FAMILY, 600.0), (BOLD_FAMILY, 700.0)];

fn all_families() -> impl Iterator<Item = FontFamily> {
    sans_families().into_iter().chain([FontFamily::Monospace])
}

/// IBM Plex Sans at 400/500/600/700 and Plex Mono Regular, followed by egui's symbol/emoji
/// fallbacks. Each variable instance shares the embedded bytes and selects a real weight.
pub fn base_font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for ((name, weight), family) in WEIGHTS.into_iter().zip(sans_families()) {
        let mut data = egui::FontData::from_static(PLEX_SANS);
        data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", weight)]);
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
        fonts.families.insert(family, std::iter::once(name.to_owned()).chain(fallbacks.iter().cloned()).collect());
    }
    fonts.font_data.insert("plex-mono".to_owned(), Arc::new(egui::FontData::from_static(PLEX_MONO)));
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "plex-mono".to_owned());
    with_symbol_fallback(fonts)
}

// Keep pending definitions too: set_fonts takes effect on the next frame, while the web
// fetch and preference update can both finish before that frame starts.
fn installed_fonts_id() -> egui::Id {
    egui::Id::unique("solidcraft_interface_fonts")
}

fn install(ctx: &egui::Context, fonts: FontDefinitions) {
    let fonts = with_symbol_fallback(fonts);
    ctx.data_mut(|d| d.insert_temp(installed_fonts_id(), fonts.clone()));
    ctx.set_fonts(fonts);
    ctx.global_style_mut(|style| {
        if let Some(heading) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
            heading.family = semibold(heading.size).family;
        }
    });
}

/// Install only the bundled definitions (also used by the offscreen scenario shell).
pub fn setup_bundled_fonts(ctx: &egui::Context) {
    install(ctx, base_font_definitions());
}

/// The language fallbacks: system fonts, or the web app's download.
const LANGUAGE_FALLBACKS: [&str; 6] = [
    CJK_FALLBACK,
    CHINESE_FALLBACK,
    HANGUL_FALLBACK,
    crate::kit::type_face::THAI_FALLBACK,
    crate::kit::type_face::HEBREW_FALLBACK,
    crate::kit::type_face::ARABIC_FALLBACK,
];

/// `fonts` with egui's built-in Hack after the other bundled faces and before the language
/// fallbacks in every sans family. Ubuntu-Light, NotoEmoji and emoji-icon-font lack common symbols
/// such as ▲ ▼ ↑ ↓ → ⁻, which Hack has, so they no longer show as empty boxes where no system font
/// fills in (#2955). Text the other faces draw keeps them. A symbol Hack has no longer comes from a
/// language face, where it may be far larger than the text around it: Hiragino's ⇧ stands half as
/// tall again as a capital, in a full em. Hack has no letter, mark or punctuation of those
/// languages, so their text keeps its face, and a CJK UI's promoted face stays first.
pub fn with_symbol_fallback(mut fonts: egui::FontDefinitions) -> egui::FontDefinitions {
    if fonts.font_data.contains_key(SYMBOL_FALLBACK) {
        for family in sans_families() {
            let list = fonts.families.entry(family).or_default();
            list.retain(|name| name != SYMBOL_FALLBACK);
            let after_bundled = list.iter().rposition(|name| !LANGUAGE_FALLBACKS.contains(&name.as_str())).map_or(0, |last| last + 1);
            list.insert(after_bundled, SYMBOL_FALLBACK.to_owned());
        }
    }
    fonts
}

/// `fonts` with `bytes` appended under `name` as the last fallback of every interface
/// family. `None` when the bytes are not a font.
fn with_fallback(mut fonts: egui::FontDefinitions, name: &str, bytes: Vec<u8>) -> Option<egui::FontDefinitions> {
    ttf_parser::Face::parse(&bytes, 0).ok()?;
    fonts.font_data.insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
    for family in all_families() {
        let list = fonts.families.entry(family).or_default();
        list.retain(|entry| entry != name);
        list.push(name.to_owned());
    }
    Some(fonts)
}

/// Use one primary CJK face for mixed-script labels in every sans weight family.
/// An explicit interface font stays first; Plex and the other fallbacks remain available.
fn promote_font_to_front(fonts: &mut FontDefinitions, name: &str) {
    for family in sans_families() {
        if let Some(list) = fonts.families.get_mut(&family)
            && let Some(pos) = list.iter().position(|entry| entry == name)
        {
            let item = list.remove(pos);
            let first = usize::from(list.first().is_some_and(|entry| entry == UI_FONT));
            list.insert(first, item);
        }
    }
}

/// Lower-case file-name fragments that identify CJK fonts. Deliberately specific: generic
/// words such as "noto" or "gothic" also match Latin-only fonts (NotoSans-Regular, Century Gothic).
pub const CJK_FONT_KEYWORDS: &[&str] = &[
    "cjk",
    "notosansjp",
    "notoserifjp",
    "bizud",
    "biz-ud",
    "yugoth",
    "meiryo",
    "msgothic",
    "hiragino",
    "pingfang",
    "songti",
    "arial unicode",
    "ipag",
    "ipaexg",
    "takao",
    "vlgothic",
    "droidsansfallback",
];

/// Whether a font file name looks like a CJK font.
pub fn is_cjk_font_name(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    CJK_FONT_KEYWORDS.iter().any(|k| name.contains(k))
}

/// Lower-case file-name fragments that identify fonts covering simplified Chinese. Deliberately
/// separate from [`CJK_FONT_KEYWORDS`]: many Japanese fonts (Hiragino Gothic, Meiryo, Yu Gothic)
/// match those but lack simplified characters, so a Chinese fallback searches these instead.
pub const CHINESE_FONT_KEYWORDS: &[&str] = &[
    "pingfang",
    "hiragino sans gb",
    "stheiti",
    "songti",
    "arial unicode",
    "msyh",
    "yahei",
    "simhei",
    "simsun",
    "wqy",
    "wenquanyi",
    "notosanscjk",
    "notosanssc",
    "notoserifcjk",
    "notoserifsc",
    "droidsansfallback",
    "sourcehansans",
    "sourcehanserif",
    "siyuan",
];

/// Whether a font file name looks like a font covering simplified Chinese.
pub fn is_chinese_font_name(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    CHINESE_FONT_KEYWORDS.iter().any(|k| name.contains(k))
}

/// Candidate font paths to look for on different operating systems.
#[cfg(not(target_arch = "wasm32"))]
const FONT_CANDIDATES: &[&str] = &[
    // Windows
    "C:\\Windows\\Fonts\\meiryo.ttc",
    "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\YuGothR.ttc",
    "C:\\Windows\\Fonts\\msgothic.ttc",
    "C:\\Windows\\Fonts\\BIZ-UDGothicR.ttc",
    "C:\\Windows\\Fonts\\BIZ-UDMinchoM.ttc",
    "C:\\Windows\\Fonts\\yumin.ttf",
    // macOS
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "/System/Library/Fonts/Hiragino Sans W3.ttc",
    "/System/Library/Fonts/Hiragino Sans W4.ttc",
    "/System/Library/Fonts/Hiragino Sans W6.ttc",
    "/System/Library/Fonts/Supplemental/Hiragino Sans W3.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
    "/Library/Fonts/Hiragino Sans W3.ttc",
    // Linux
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Regular.otf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/opentype/ipafont-gothic/ipag.ttf",
    "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    "/usr/share/fonts/truetype/takao-gothic/TakaoPGothic.ttf",
    "/usr/share/fonts/truetype/takao-gothic/TakaoGothic.ttf",
    "/usr/share/fonts/truetype/vlgothic/VL-PGothic-Regular.ttf",
    "/usr/share/fonts/truetype/ipafont/ipag.ttf",
    "/usr/share/fonts/truetype/ipaexfont/ipaexg.ttf",
    "/usr/share/fonts/opentype/ipafont/ipag.ttf",
    "/usr/share/fonts/opentype/ipaexfont/ipaexg.ttf",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/cjkuni-uming/uming.ttc",
];

/// Hangul fonts, appended after the CJK fallback so Korean renders even when the CJK font found
/// is a Japanese one without Hangul (Meiryo, Hiragino).
#[cfg(not(target_arch = "wasm32"))]
const HANGUL_FONT_CANDIDATES: &[&str] = &[
    // Windows: Malgun Gothic
    "C:\\Windows\\Fonts\\malgun.ttf",
    // macOS: Apple SD Gothic Neo
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
    // Linux: Nanum Gothic (Noto Sans CJK already covers Hangul)
    "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    "/usr/share/fonts/nanum/NanumGothic.ttf",
];

/// Thai fonts, appended last: none of the Latin or CJK faces draw Thai, so without one Thai text
/// shows as empty boxes (#1902).
#[cfg(not(target_arch = "wasm32"))]
const THAI_FONT_CANDIDATES: &[&str] = &[
    // Windows: Leelawadee UI (Windows 8 and later), Leelawadee, Tahoma
    "C:\\Windows\\Fonts\\LeelawUI.ttf",
    "C:\\Windows\\Fonts\\leelawad.ttf",
    "C:\\Windows\\Fonts\\tahoma.ttf",
    // macOS: Thonburi, Ayuthaya
    "/System/Library/Fonts/Thonburi.ttc",
    "/System/Library/Fonts/Supplemental/Thonburi.ttc",
    "/System/Library/Fonts/Supplemental/Ayuthaya.ttf",
    "/Library/Fonts/Thonburi.ttf",
    // Linux: Noto Sans Thai, TLWG Loma and Garuda
    "/usr/share/fonts/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSansThai[wght].ttf",
    "/usr/share/fonts/opentype/noto/NotoSansThai-Regular.ttf",
    "/usr/share/fonts/truetype/tlwg/Loma.ttf",
    "/usr/share/fonts/opentype/tlwg/Loma.otf",
    "/usr/share/fonts/truetype/tlwg/Garuda.ttf",
    "/usr/share/fonts/opentype/tlwg/Garuda.otf",
    "/usr/share/fonts/tlwg/Loma.otf",
];

/// Hebrew fonts, appended last: egui's built-in faces have no Hebrew, so without one the Hebrew
/// interface and Hebrew names show as empty boxes (#3457).
#[cfg(not(target_arch = "wasm32"))]
const HEBREW_FONT_CANDIDATES: &[&str] = &[
    // Windows: Segoe UI, Arial
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
    // macOS: Arial Hebrew, SF Hebrew
    "/System/Library/Fonts/ArialHB.ttc",
    "/System/Library/Fonts/SFHebrew.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    // Linux: Noto Sans Hebrew, DejaVu Sans
    "/usr/share/fonts/noto/NotoSansHebrew-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansHebrew-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSansHebrew[wght].ttf",
    "/usr/share/fonts/opentype/noto/NotoSansHebrew-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
];

/// Arabic fonts with the joining forms the shaper uses. egui's built-in faces have no Arabic.
#[cfg(not(target_arch = "wasm32"))]
const ARABIC_FONT_CANDIDATES: &[&str] = &[
    // Windows: Segoe UI, Tahoma, Arial
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\tahoma.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
    // macOS: SF Arabic (11 and later), Geeza Pro, Damascus, Tahoma
    "/System/Library/Fonts/SFArabic.ttf",
    "/System/Library/Fonts/GeezaPro.ttc",
    "/System/Library/Fonts/Supplemental/GeezaPro.ttc",
    "/System/Library/Fonts/Supplemental/Damascus.ttc",
    "/System/Library/Fonts/Supplemental/Tahoma.ttf",
    // Linux: Noto Sans Arabic, Noto Naskh Arabic, KACST One, DejaVu Sans
    "/usr/share/fonts/noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSansArabic[wght].ttf",
    "/usr/share/fonts/opentype/noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoNaskhArabic-Regular.ttf",
    "/usr/share/fonts/noto/NotoNaskhArabic-Regular.ttf",
    "/usr/share/fonts/truetype/kacst-one/KacstOne.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
];

/// Fonts covering simplified Chinese, appended after the CJK fallback so Chinese renders even
/// when the CJK font found is a Japanese one without simplified characters (Hiragino Gothic,
/// Meiryo, Yu Gothic).
#[cfg(not(target_arch = "wasm32"))]
const CHINESE_FONT_CANDIDATES: &[&str] = &[
    // Windows: Microsoft YaHei
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\msyh.ttf",
    // macOS: PingFang SC, Hiragino Sans GB, Songti SC, STHeiti, Arial Unicode MS
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    // Linux: Noto Sans CJK SC
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
];

/// Whether the first face in `bytes` draws `c`: some candidates (an older Arial, a trimmed
/// DejaVu) may lack the script they are listed for.
#[cfg(not(target_arch = "wasm32"))]
fn covers(bytes: &[u8], c: char) -> bool {
    ttf_parser::Face::parse(bytes, 0).is_ok_and(|face| face.glyph_index(c).is_some())
}

#[cfg(not(target_arch = "wasm32"))]
fn try_read_font(path: &std::path::Path) -> Option<Vec<u8>> {
    if let Ok(bytes) = std::fs::read(path)
        && ttf_parser::Face::parse(&bytes, 0).is_ok()
    {
        Some(bytes)
    } else {
        None
    }
}

/// Recursively search for matching font files up to a bounded depth, testing names with `matches`.
#[cfg(not(target_arch = "wasm32"))]
fn scan_dir_for_font(dir: &std::path::Path, depth_remaining: usize, matches: fn(&str) -> bool) -> Option<Vec<u8>> {
    scan_dir_for_font_where(dir, depth_remaining, matches, |_| true)
}

/// [`scan_dir_for_font`] that also skips a font whose bytes `accept` refuses.
#[cfg(not(target_arch = "wasm32"))]
fn scan_dir_for_font_where(dir: &std::path::Path, depth_remaining: usize, matches: fn(&str) -> bool, accept: fn(&[u8]) -> bool) -> Option<Vec<u8>> {
    if depth_remaining == 0 {
        return None;
    }
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();

    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_file() {
            let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let is_font = name.ends_with(".ttc") || name.ends_with(".ttf") || name.ends_with(".otf");
            if is_font
                && matches(&name)
                && let Some(bytes) = try_read_font(&p)
                && accept(&bytes)
            {
                return Some(bytes);
            }
        } else if p.is_dir() {
            subdirs.push(p);
        }
    }

    for sub in subdirs {
        if let Some(bytes) = scan_dir_for_font_where(&sub, depth_remaining.saturating_sub(1), matches, accept) {
            return Some(bytes);
        }
    }

    None
}

/// The font folders named by `SOLIDCRAFT_FONT_DIRS` (a path list), or else by the single folder
/// in `CRAFT_FONTS_DIR` (the craft-fonts pack).
#[cfg(not(target_arch = "wasm32"))]
fn font_override_dirs(dirs: Option<std::ffi::OsString>, pack: Option<std::ffi::OsString>) -> Vec<std::path::PathBuf> {
    match (dirs, pack) {
        (Some(dirs), _) => std::env::split_paths(&dirs).collect(),
        (None, Some(pack)) => vec![std::path::PathBuf::from(pack)],
        (None, None) => Vec::new(),
    }
}

/// Find a CJK fallback font on the local system or in SOLIDCRAFT_FONT_DIRS / CRAFT_FONTS_DIR.
#[cfg(not(target_arch = "wasm32"))]
fn find_system_cjk_font() -> Option<Vec<u8>> {
    // 1. Check the font folders the user named.
    for dir in font_override_dirs(std::env::var_os("SOLIDCRAFT_FONT_DIRS"), std::env::var_os("CRAFT_FONTS_DIR")) {
        let names = ["BIZUDPGothic-Regular.ttf", "BIZ-UDGothicR.ttc", "NotoSansCJK-Regular.ttc", "NotoSansJP-Regular.ttf"];
        for name in names {
            if let Some(bytes) = try_read_font(&dir.join(name)) {
                return Some(bytes);
            }
        }
    }

    // 2. Check well-known candidate paths.
    for path in FONT_CANDIDATES {
        if let Some(bytes) = try_read_font(std::path::Path::new(path)) {
            return Some(bytes);
        }
    }

    // 3. Platform-specific dynamic directory search.
    #[cfg(target_os = "windows")]
    {
        if let Ok(windir) = std::env::var("WINDIR").or_else(|_| std::env::var("SystemRoot")) {
            let fonts_dir = std::path::PathBuf::from(windir).join("Fonts");
            let names = ["meiryo.ttc", "YuGothM.ttc", "YuGothR.ttc", "msgothic.ttc", "BIZ-UDGothicR.ttc"];
            for name in names {
                if let Some(bytes) = try_read_font(&fonts_dir.join(name)) {
                    return Some(bytes);
                }
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let user_fonts = std::path::PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts");
            if let Some(bytes) = scan_dir_for_font(&user_fonts, 2, is_cjk_font_name) {
                return Some(bytes);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut dirs = vec![
            std::path::PathBuf::from("/System/Library/Fonts"),
            std::path::PathBuf::from("/System/Library/Fonts/Supplemental"),
            std::path::PathBuf::from("/Library/Fonts"),
        ];
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(std::path::PathBuf::from(home).join("Library/Fonts"));
        }
        for d in &dirs {
            if let Some(bytes) = scan_dir_for_font(d, 2, is_cjk_font_name) {
                return Some(bytes);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let search_dirs = [
            std::path::PathBuf::from("/usr/share/fonts"),
            std::path::PathBuf::from("/usr/local/share/fonts"),
            std::path::PathBuf::from("/run/host/fonts"),
            std::path::PathBuf::from("/run/host/user-fonts"),
        ];
        for dir in &search_dirs {
            if let Some(bytes) = scan_dir_for_font(dir, 3, is_cjk_font_name) {
                return Some(bytes);
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let user_fonts = std::path::PathBuf::from(home).join(".local/share/fonts");
            if let Some(bytes) = scan_dir_for_font(&user_fonts, 2, is_cjk_font_name) {
                return Some(bytes);
            }
        }
    }

    None
}

/// The system fallback faces, searched for once and kept: reading them takes long on a system
/// with large CJK fonts, so the app starts that search on a thread and draws its first frame
/// with the bundled faces (see [`load_system_fonts_in_background`]).
#[cfg(not(target_arch = "wasm32"))]
struct SystemFonts {
    cjk: Option<Vec<u8>>,
    chinese: Option<Vec<u8>>,
    hangul: Option<Vec<u8>>,
    thai: Option<Vec<u8>>,
    hebrew: Option<Vec<u8>>,
    arabic: Option<Vec<u8>>,
}

#[cfg(not(target_arch = "wasm32"))]
static SYSTEM_FONTS: std::sync::OnceLock<SystemFonts> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn system_fallbacks() -> &'static SystemFonts {
    SYSTEM_FONTS.get_or_init(|| {
        let cjk = find_system_cjk_font();
        let chinese = find_system_chinese_font().filter(|bytes| cjk.as_ref() != Some(bytes));
        let hangul = HANGUL_FONT_CANDIDATES.iter().find_map(|path| try_read_font(std::path::Path::new(path)));
        let thai = THAI_FONT_CANDIDATES.iter().find_map(|path| try_read_font(std::path::Path::new(path)));
        let hebrew = HEBREW_FONT_CANDIDATES.iter().find_map(|path| try_read_font(std::path::Path::new(path)).filter(|bytes| covers(bytes, 'א')));
        let arabic = arabic_font();
        SystemFonts { cjk, chinese, hangul, thai, hebrew, arabic }
    })
}

/// Whether the system fallback faces have been searched for.
#[cfg(not(target_arch = "wasm32"))]
pub fn system_fonts_ready() -> bool {
    SYSTEM_FONTS.get().is_some()
}

/// Search for the system fallback faces on a thread and repaint when they are found.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_system_fonts_in_background(ctx: &egui::Context) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    if SYSTEM_FONTS.get().is_some() {
        return;
    }
    let mut first = false;
    STARTED.call_once(|| first = true);
    if !first {
        return;
    }
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("system-fonts".into()).spawn(move || {
        system_fallbacks();
        ctx.request_repaint();
    });
    // Without a thread the first frame waits for them, as it always did.
    if spawned.is_err() {
        system_fallbacks();
    }
}

/// [`setup_fonts_for`] with the system fallbacks that are there already, never waiting for the search.
#[cfg(not(target_arch = "wasm32"))]
pub fn setup_fonts_without_waiting(ctx: &egui::Context, language: &str) {
    install_fonts(ctx, language, SYSTEM_FONTS.get());
}

#[cfg(not(target_arch = "wasm32"))]
fn install_fonts(ctx: &egui::Context, language: &str, system: Option<&SystemFonts>) {
    let (cjk, chinese, hangul, thai, hebrew, arabic) = match system {
        Some(f) => (f.cjk.clone(), f.chinese.clone(), f.hangul.clone(), f.thai.clone(), f.hebrew.clone(), f.arabic.clone()),
        None => (None, None, None, None, None, None),
    };
    let mut fonts = base_font_definitions();
    let order = fallback_order(language);
    for name in order {
        let bytes = match name {
            CJK_FALLBACK => cjk.clone(),
            CHINESE_FALLBACK => chinese.clone(),
            HANGUL_FALLBACK => hangul.clone(),
            THAI_FALLBACK => thai.clone(),
            HEBREW_FALLBACK => hebrew.clone(),
            _ => arabic.clone(),
        };
        if let Some(bytes) = bytes
            && let Some(with_fallback) = with_fallback(fonts.clone(), name, bytes)
        {
            fonts = with_fallback;
        }
    }
    if needs_cjk_font(language) {
        // Select the first installed face in language order (a duplicate Chinese face may
        // already be installed as CJK_FALLBACK). Keep explicit interface choices ahead of it.
        if let Some(primary) =
            order.into_iter().find(|name| ![THAI_FALLBACK, HEBREW_FALLBACK, ARABIC_FALLBACK].contains(name) && fonts.font_data.contains_key(*name))
        {
            promote_font_to_front(&mut fonts, primary);
        }
    }
    install(ctx, fonts);
}

/// Find a font covering simplified Chinese in the known candidate paths or by scanning the
/// system font directories for [`CHINESE_FONT_KEYWORDS`] names.
#[cfg(not(target_arch = "wasm32"))]
fn find_system_chinese_font() -> Option<Vec<u8>> {
    for path in CHINESE_FONT_CANDIDATES {
        if let Some(bytes) = try_read_font(std::path::Path::new(path)) {
            return Some(bytes);
        }
    }
    find_chinese_font_in(&chinese_font_dirs())
}

/// A character only fonts covering simplified Chinese draw: 设 (as in 设置, "settings"), boxed in
/// the report of #3376. Japanese fonts (Meiryo, Yu Gothic, BIZ UDPGothic, IPA) lack it.
#[cfg(not(target_arch = "wasm32"))]
const SIMPLIFIED_CHINESE_PROBE: char = '设';

/// Whether the font's first face draws simplified Chinese (see [`SIMPLIFIED_CHINESE_PROBE`]).
#[cfg(not(target_arch = "wasm32"))]
fn draws_simplified_chinese(bytes: &[u8]) -> bool {
    ttf_parser::Face::parse(bytes, 0).is_ok_and(|face| face.glyph_index(SIMPLIFIED_CHINESE_PROBE).is_some())
}

/// The system and per-user font folders searched for a Chinese font when no candidate path has
/// one, on every desktop system (#3376: Linux and Windows searched only the fixed paths, so a
/// Japanese CJK font drew the Chinese UI alone and its simplified characters showed as boxes).
#[cfg(not(target_arch = "wasm32"))]
fn chinese_font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Ok(windir) = std::env::var("WINDIR").or_else(|_| std::env::var("SystemRoot")) {
            dirs.push(std::path::PathBuf::from(windir).join("Fonts"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(std::path::PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        for d in ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"] {
            dirs.push(std::path::PathBuf::from(d));
        }
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(std::path::PathBuf::from(home).join("Library/Fonts"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push(std::path::PathBuf::from("/usr/share/fonts"));
        dirs.push(std::path::PathBuf::from("/usr/local/share/fonts"));
        dirs.push(std::path::PathBuf::from("/run/host/fonts"));
        dirs.push(std::path::PathBuf::from("/run/host/user-fonts"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(std::path::PathBuf::from(&home).join(".local/share/fonts"));
            dirs.push(std::path::PathBuf::from(home).join(".fonts"));
        }
    }
    dirs
}

/// The first font in `dirs` (searched three folders deep, which reaches
/// `/usr/share/fonts/truetype/wqy/`) with a Chinese font's name that draws simplified Chinese.
/// The drawing check keeps out a Japanese-only file whose name matches, such as a
/// `SourceHanSansJP` subset, and Windows' SimSun extension fonts.
#[cfg(not(target_arch = "wasm32"))]
fn find_chinese_font_in(dirs: &[std::path::PathBuf]) -> Option<Vec<u8>> {
    dirs.iter().find_map(|dir| scan_dir_for_font_where(dir, 3, is_chinese_font_name, draws_simplified_chinese))
}

/// The first Arabic font in `CRAFT_FONTS_DIR` (`NotoSansArabic-Regular.ttf`) or the candidate
/// paths that draws Arabic.
#[cfg(not(target_arch = "wasm32"))]
fn arabic_font() -> Option<Vec<u8>> {
    let pack = std::env::var("CRAFT_FONTS_DIR").ok().map(|dir| std::path::PathBuf::from(dir).join("NotoSansArabic-Regular.ttf"));
    pack.into_iter()
        .chain(ARABIC_FONT_CANDIDATES.iter().map(std::path::PathBuf::from))
        .find_map(|path| try_read_font(&path).filter(|bytes| covers(bytes, 'ب')))
}
