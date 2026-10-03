// 터미널 셀 색 해석. 기본 배경/전경과 ANSI 16색은 테마가 정한다(DEV-021,
// `theme::TermPalette`) — 256색 큐브/그레이스케일과 트루컬러는 테마와 무관.
use crate::theme::TermPalette;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use eframe::egui::Color32;

fn rgb_to_color32(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

fn indexed_to_color32(idx: u8, pal: &TermPalette) -> Color32 {
    if let Some(&c) = pal.ansi.get(idx as usize) {
        return c;
    }
    if (16..=231).contains(&idx) {
        // 6x6x6 색상 큐브 (xterm 표준 레벨)
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let i = idx - 16;
        let r = LEVELS[(i / 36) as usize];
        let g = LEVELS[((i / 6) % 6) as usize];
        let b = LEVELS[(i % 6) as usize];
        return Color32::from_rgb(r, g, b);
    }
    // 232..=255: 그레이스케일 램프
    let level = 8 + (idx.saturating_sub(232)) as u32 * 10;
    let level = level.min(255) as u8;
    Color32::from_rgb(level, level, level)
}

pub fn resolve_fg(color: Color, pal: &TermPalette) -> Color32 {
    match color {
        Color::Named(NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground) => {
            pal.fg
        }
        Color::Named(NamedColor::Background) => pal.bg,
        Color::Named(named) => named_to_color32(named, pal),
        Color::Spec(rgb) => rgb_to_color32(rgb),
        Color::Indexed(idx) => indexed_to_color32(idx, pal),
    }
}

/// 굵게(BOLD) 표시된 글자의 전경색. 이 렌더러는 굵은 폰트 weight를 따로 로드하지
/// 않으므로(DEV-011은 코드포인트 폴백 범위 밖 — 폰트마다 Bold variant를 또 찾아야
/// 해서 범위가 커짐), **일반(0-7) ANSI 색을 밝은(8-15) 계열로 승격**하는 것으로
/// 굵기를 표현한다 — iTerm2·Windows Terminal 등 대부분의 터미널이 쓰는 관례.
/// 이게 없으면 `ls --color`, git diff 등의 굵은 강조가 색·굵기 둘 다 안 보여서
/// "글자 굵기/색이 제대로 표현 안 됨"으로 체감된다(실사용 버그 리포트).
pub fn resolve_fg_bold_aware(color: Color, bold: bool, pal: &TermPalette) -> Color32 {
    if !bold {
        return resolve_fg(color, pal);
    }
    match color {
        Color::Named(named) => resolve_fg(Color::Named(brighten_named(named)), pal),
        Color::Indexed(idx) if idx < 8 => resolve_fg(Color::Indexed(idx + 8), pal),
        other => resolve_fg(other, pal),
    }
}

fn brighten_named(named: NamedColor) -> NamedColor {
    use NamedColor::*;
    match named {
        Black | DimBlack => BrightBlack,
        Red | DimRed => BrightRed,
        Green | DimGreen => BrightGreen,
        Yellow | DimYellow => BrightYellow,
        Blue | DimBlue => BrightBlue,
        Magenta | DimMagenta => BrightMagenta,
        Cyan | DimCyan => BrightCyan,
        White | DimWhite => BrightWhite,
        other => other, // 이미 Bright*이거나 Foreground/Background/Cursor 등 — 그대로.
    }
}

pub fn resolve_bg(color: Color, pal: &TermPalette) -> Color32 {
    match color {
        Color::Named(NamedColor::Background) => pal.bg,
        Color::Named(NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground) => {
            pal.fg
        }
        Color::Named(named) => named_to_color32(named, pal),
        Color::Spec(rgb) => rgb_to_color32(rgb),
        Color::Indexed(idx) => indexed_to_color32(idx, pal),
    }
}

fn named_to_color32(named: NamedColor, pal: &TermPalette) -> Color32 {
    use NamedColor::*;
    let idx = match named {
        Black | DimBlack => 0,
        Red | DimRed => 1,
        Green | DimGreen => 2,
        Yellow | DimYellow => 3,
        Blue | DimBlue => 4,
        Magenta | DimMagenta => 5,
        Cyan | DimCyan => 6,
        White | DimWhite => 7,
        BrightBlack => 8,
        BrightRed => 9,
        BrightGreen => 10,
        BrightYellow => 11,
        BrightBlue => 12,
        BrightMagenta => 13,
        BrightCyan => 14,
        BrightWhite => 15,
        // Foreground/Background/Cursor/BrightForeground/DimForeground은 위에서 먼저 처리됨
        _ => return pal.fg,
    };
    pal.ansi[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    const P: &TermPalette = &DARK.term;

    /// 실사용 버그: BOLD 텍스트의 색이 일반 텍스트와 구분이 안 됨. 별도 굵은
    /// 폰트 weight 없이도, 일반(0-7) ANSI 색은 밝은(8-15) 계열로 승격돼야 한다.
    #[test]
    fn bold_promotes_named_colors_to_bright_variants() {
        let normal_red = resolve_fg(Color::Named(NamedColor::Red), P);
        let bright_red = resolve_fg(Color::Named(NamedColor::BrightRed), P);
        let bold_red = resolve_fg_bold_aware(Color::Named(NamedColor::Red), true, P);

        assert_eq!(bold_red, bright_red, "굵은 빨강이 밝은 빨강으로 승격되지 않음");
        assert_ne!(bold_red, normal_red, "굵은 색이 일반 색과 구분이 안 됨");
    }

    #[test]
    fn bold_promotes_indexed_colors_0_to_7() {
        let bold = resolve_fg_bold_aware(Color::Indexed(2), true, P); // 2 = Green
        let bright_green = resolve_fg(Color::Indexed(10), P); // 10 = BrightGreen
        assert_eq!(bold, bright_green);
    }

    /// 이미 밝은 색이거나 팔레트 범위(8 이상) 밖이면 그대로 둔다 — 더 밝힐 데가 없다.
    #[test]
    fn bold_does_not_change_already_bright_or_indexed_colors() {
        let already_bright = resolve_fg_bold_aware(Color::Named(NamedColor::BrightBlue), true, P);
        assert_eq!(already_bright, resolve_fg(Color::Named(NamedColor::BrightBlue), P));

        let high_index = resolve_fg_bold_aware(Color::Indexed(200), true, P);
        assert_eq!(high_index, resolve_fg(Color::Indexed(200), P));
    }

    #[test]
    fn not_bold_leaves_color_unchanged() {
        let color = Color::Named(NamedColor::Cyan);
        assert_eq!(resolve_fg_bold_aware(color, false, P), resolve_fg(color, P));
    }

    #[test]
    fn bold_does_not_affect_true_color_rgb() {
        let rgb = Color::Spec(Rgb { r: 12, g: 34, b: 56 });
        assert_eq!(resolve_fg_bold_aware(rgb, true, P), resolve_fg(rgb, P), "트루컬러는 승격 대상이 아님");
    }
}
