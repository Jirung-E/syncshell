// 색 테마 시스템은 예광탄 범위 밖 — 고정된 기본 팔레트 하나만 사용한다.
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use eframe::egui::Color32;

pub const DEFAULT_BG: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
pub const DEFAULT_FG: Color32 = Color32::from_rgb(0xcc, 0xcc, 0xcc);

const ANSI_16: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00), // Black
    (0xcd, 0x31, 0x31), // Red
    (0x0d, 0xbc, 0x79), // Green
    (0xe5, 0xe5, 0x10), // Yellow
    (0x24, 0x72, 0xc8), // Blue
    (0xbc, 0x3f, 0xbc), // Magenta
    (0x11, 0xa8, 0xcd), // Cyan
    (0xe5, 0xe5, 0xe5), // White
    (0x80, 0x80, 0x80), // BrightBlack
    (0xf1, 0x4c, 0x4c), // BrightRed
    (0x23, 0xd1, 0x8b), // BrightGreen
    (0xf5, 0xf5, 0x43), // BrightYellow
    (0x3b, 0x8e, 0xea), // BrightBlue
    (0xd6, 0x70, 0xd6), // BrightMagenta
    (0x29, 0xb8, 0xdb), // BrightCyan
    (0xff, 0xff, 0xff), // BrightWhite
];

fn rgb_to_color32(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

fn indexed_to_color32(idx: u8) -> Color32 {
    if let Some(&(r, g, b)) = ANSI_16.get(idx as usize) {
        return Color32::from_rgb(r, g, b);
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

pub fn resolve_fg(color: Color) -> Color32 {
    match color {
        Color::Named(NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground) => {
            DEFAULT_FG
        }
        Color::Named(NamedColor::Background) => DEFAULT_BG,
        Color::Named(named) => named_to_color32(named),
        Color::Spec(rgb) => rgb_to_color32(rgb),
        Color::Indexed(idx) => indexed_to_color32(idx),
    }
}

pub fn resolve_bg(color: Color) -> Color32 {
    match color {
        Color::Named(NamedColor::Background) => DEFAULT_BG,
        Color::Named(NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground) => {
            DEFAULT_FG
        }
        Color::Named(named) => named_to_color32(named),
        Color::Spec(rgb) => rgb_to_color32(rgb),
        Color::Indexed(idx) => indexed_to_color32(idx),
    }
}

fn named_to_color32(named: NamedColor) -> Color32 {
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
        _ => return DEFAULT_FG,
    };
    let (r, g, b) = ANSI_16[idx];
    Color32::from_rgb(r, g, b)
}
