//! 다크/라이트 테마(DEV-021). 앱 크롬 색과 터미널 색을 한 곳에서 정의한다.
//!
//! 색 값은 UI 시안(통합안, 2026-10-03 확정)에서 가져왔다 — 바꿀 때는 시안과
//! 같이 바꾼다. 테마 선택은 앱 전역 하나이고, 종료 시 `state.toml`에 저장된다
//! (앱이 쓰는 값이라 사람이 편집하는 `config.toml`이 아님, architecture 규칙 3).

use eframe::egui::{self, Color32};

/// 사용자가 고르는 테마. `state.toml`에는 소문자 문자열("dark"/"light")로 저장한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// `state.toml`에서 읽은 문자열을 해석한다. 모르는 값(손으로 고쳤거나 이후
    /// 버전에서 생긴 값)은 기본값(다크)으로 — 이것 때문에 앱이 안 뜨면 안 된다.
    pub fn parse(s: &str) -> Self {
        match s {
            "light" => Self::Light,
            _ => Self::Dark,
        }
    }

    pub fn palette(self) -> &'static Palette {
        match self {
            Self::Dark => &DARK,
            Self::Light => &LIGHT,
        }
    }
}

/// 앱 크롬 색 + 터미널 색.
#[derive(Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// 창 바탕(패널 사이 빈 곳, 세그먼트 버튼 바탕 등)
    pub bg: Color32,
    /// 탭 바·상태바
    pub chrome: Color32,
    /// 탐색기 패널, 활성 탭
    pub panel: Color32,
    pub border: Color32,
    pub text: Color32,
    /// 보조 글자(크기·수정일, 비활성 탭 등) — 패널 위에서 4.5:1 이상
    pub muted: Color32,
    /// 구분자 같은 아주 옅은 글자
    pub faint: Color32,
    pub accent: Color32,
    /// 선택된 항목 바탕
    pub selection: Color32,
    pub selection_text: Color32,
    /// 마우스를 올린 항목 바탕
    pub hover: Color32,
    /// 세그먼트 버튼에서 선택된 칸 바탕
    pub segment_on: Color32,
    pub ok: Color32,
    pub error: Color32,
    pub term: TermPalette,
}

/// 터미널 셀 색. 라이트 모드는 밝은 배경용 ANSI 팔레트를 따로 쓴다 — 어두운
/// 배경용 팔레트를 그대로 쓰면 노랑·흰색 계열 글자가 흰 바탕에서 안 보인다.
#[derive(Debug, PartialEq)]
pub struct TermPalette {
    pub bg: Color32,
    pub fg: Color32,
    pub cursor: Color32,
    /// ANSI 0-15 (0-7 일반, 8-15 밝은 계열)
    pub ansi: [Color32; 16],
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub static DARK: Palette = Palette {
    dark: true,
    bg: rgb(0x17181B),
    chrome: rgb(0x121316),
    panel: rgb(0x1E1F23),
    border: rgb(0x2A2C31),
    text: rgb(0xD7D9DE),
    muted: rgb(0x8B8F98),
    faint: rgb(0x4A4D55),
    accent: rgb(0x6EA8FE),
    selection: rgb(0x2C3445),
    selection_text: rgb(0xFFFFFF),
    hover: rgb(0x26282D),
    segment_on: rgb(0x2C2E34),
    ok: rgb(0x6CC08B),
    error: rgb(0xE0787F),
    term: TermPalette {
        bg: rgb(0x121316),
        fg: rgb(0xC9CCD2),
        cursor: rgb(0xD7D9DE),
        ansi: [
            rgb(0x000000), // Black
            rgb(0xCD3131), // Red
            rgb(0x0DBC79), // Green
            rgb(0xE5E510), // Yellow
            rgb(0x2472C8), // Blue
            rgb(0xBC3FBC), // Magenta
            rgb(0x11A8CD), // Cyan
            rgb(0xE5E5E5), // White
            rgb(0x808080), // BrightBlack
            rgb(0xF14C4C), // BrightRed
            rgb(0x23D18B), // BrightGreen
            rgb(0xF5F543), // BrightYellow
            rgb(0x3B8EEA), // BrightBlue
            rgb(0xD670D6), // BrightMagenta
            rgb(0x29B8DB), // BrightCyan
            rgb(0xFFFFFF), // BrightWhite
        ],
    },
};

pub static LIGHT: Palette = Palette {
    dark: false,
    bg: rgb(0xF4F4F2),
    chrome: rgb(0xECECE9),
    panel: rgb(0xFFFFFF),
    border: rgb(0xDEDEDA),
    text: rgb(0x1F2023),
    muted: rgb(0x62666E),
    faint: rgb(0xB4B6BA),
    accent: rgb(0x2563D9),
    selection: rgb(0xDCE7FB),
    selection_text: rgb(0x0F2A5C),
    hover: rgb(0xF0F0EE),
    segment_on: rgb(0xFFFFFF),
    ok: rgb(0x1F7A43),
    error: rgb(0xC0303A),
    term: TermPalette {
        bg: rgb(0xFAFAF8),
        fg: rgb(0x2A2C31),
        cursor: rgb(0x1F2023),
        // 밝은 배경에서 읽히도록 전부 어둡게 잡았다. "White"(7, 15)도 흰 바탕
        // 위에서 보여야 하므로 회색 — 셸 프롬프트·`ls --color` 등이 흰색을
        // 강조로 쓰는 경우가 있어서, 진짜 흰색이면 글자가 사라진다.
        ansi: [
            rgb(0x24292F), // Black
            rgb(0xCF222E), // Red
            rgb(0x116329), // Green
            rgb(0x7D4E00), // Yellow
            rgb(0x0969DA), // Blue
            rgb(0x8250DF), // Magenta
            rgb(0x1B7C83), // Cyan
            rgb(0x6E7781), // White
            rgb(0x57606A), // BrightBlack
            rgb(0xA40E26), // BrightRed
            rgb(0x1A7F37), // BrightGreen
            rgb(0x633C01), // BrightYellow
            rgb(0x0550AE), // BrightBlue
            rgb(0x6639BA), // BrightMagenta
            rgb(0x136061), // BrightCyan
            rgb(0x4B535D), // BrightWhite
        ],
    },
};

/// egui 기본 위젯(버튼·메뉴·텍스트 칸·스크롤바 등)이 팔레트를 따르게 한다.
pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let p = mode.palette();
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };

    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent;
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.window_stroke = egui::Stroke::new(1.0, p.border);
    v.extreme_bg_color = p.bg;
    v.faint_bg_color = p.hover;
    v.code_bg_color = p.bg;
    v.error_fg_color = p.error;
    v.selection.bg_fill = p.selection;
    v.selection.stroke = egui::Stroke::new(1.0, p.selection_text);

    let radius = egui::CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    for (state, fill) in [
        (&mut w.inactive, Color32::TRANSPARENT),
        (&mut w.hovered, p.hover),
        (&mut w.active, p.selection),
        (&mut w.open, p.hover),
    ] {
        state.weak_bg_fill = fill;
        state.bg_fill = fill;
        state.bg_stroke = egui::Stroke::NONE;
        state.fg_stroke = egui::Stroke::new(1.0, p.text);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    // 체크박스·슬라이더 등 "칸"이 보여야 하는 위젯은 투명하면 안 보인다.
    w.inactive.bg_fill = p.hover;
    w.inactive.fg_stroke = egui::Stroke::new(1.0, p.muted);

    // egui는 기본으로 OS 다크/라이트 설정을 따라 스타일을 고른다(ThemePreference::
    // System) — 선호를 고정하지 않으면 OS가 라이트일 때 우리 다크 색을 넣어둔
    // 스타일이 아니라 egui 기본 라이트 스타일이 쓰인다.
    let egui_theme = if p.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(egui_theme);
    ctx.set_visuals_of(egui_theme, v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_mode_round_trips_through_string_and_defaults_to_dark() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            assert_eq!(ThemeMode::parse(mode.as_str()), mode);
        }
        assert_eq!(ThemeMode::parse("뭔가 이상한 값"), ThemeMode::Dark, "모르는 값은 기본(다크)으로 돌아가야 함");
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);
    }

    /// WCAG 상대 휘도 대비. 시안에서 정한 "본문 4.5:1" 기준을 코드로 지킨다 —
    /// 나중에 색을 손보다 보조 글자가 읽기 어려워지는 걸 테스트로 잡는다.
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn lum(c: Color32) -> f32 {
            let ch = |v: u8| {
                let s = v as f32 / 255.0;
                if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        }
        let (la, lb) = (lum(a), lum(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_colors_meet_contrast_minimum_on_both_themes() {
        for p in [&DARK, &LIGHT] {
            assert!(contrast(p.text, p.panel) >= 4.5, "본문 글자 대비 부족 (dark={})", p.dark);
            assert!(contrast(p.muted, p.panel) >= 4.5, "보조 글자 대비 부족 (dark={})", p.dark);
            assert!(contrast(p.selection_text, p.selection) >= 4.5, "선택 항목 글자 대비 부족 (dark={})", p.dark);
            assert!(contrast(p.term.fg, p.term.bg) >= 4.5, "터미널 기본 글자 대비 부족 (dark={})", p.dark);
        }
    }

    /// 라이트 모드에서 ANSI 색 글자가 흰 바탕에서 사라지지 않아야 한다(이 테마를
    /// 따로 만든 이유). 큰 글자 기준(3:1)으로 본다 — 터미널 강조색은 본문보다
    /// 채도가 높아도 되는 관례라.
    #[test]
    fn light_terminal_ansi_colors_stay_readable_on_light_background() {
        let p = &LIGHT.term;
        for (i, c) in p.ansi.iter().enumerate() {
            assert!(contrast(*c, p.bg) >= 3.0, "ANSI {i} 색이 라이트 배경에서 안 보임");
        }
    }
}
