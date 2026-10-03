//! 앱 크롬용 작은 위젯 — painter로 직접 그리는 아이콘과 세그먼트 버튼(DEV-021~).
//!
//! 아이콘을 글리프(이모지·기호 문자)가 아니라 선으로 그리는 이유: 폰트에 없는
//! 글리프가 두부(□)로 나오는 실사용 버그가 여러 번 있었다("✕"(U+2715) 등,
//! file_panel.rs 참고). 직접 그리면 폰트와 무관하게 항상 같은 모양이 나온다.

use crate::theme::Palette;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Stroke};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Sun,
    Moon,
}

/// `rect` 안에 아이콘을 그린다. 좌표는 16×16 격자 기준으로 잡고 rect 크기에
/// 맞춰 늘린다(시안의 SVG viewBox와 같은 좌표라 시안과 대조하기 쉽다).
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - vec2(8.0 * s, 8.0 * s);
    let p = |x: f32, y: f32| -> Pos2 { o + vec2(x * s, y * s) };
    let stroke = Stroke::new(1.4 * s.max(0.9), color);

    match icon {
        Icon::Sun => {
            painter.circle_stroke(p(8.0, 8.0), 3.0 * s, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = a.sin_cos();
                painter.line_segment([p(8.0 + cos * 5.0, 8.0 + sin * 5.0), p(8.0 + cos * 6.5, 8.0 + sin * 6.5)], stroke);
            }
        }
        Icon::Moon => {
            // 초승달 = 큰 원에서 오른쪽 위로 비켜난 작은 원을 뺀 모양. 두 원의
            // 교차점 둘을 구하고, 큰 원의 "작은 원 밖" 호와 작은 원의 "큰 원 안"
            // 호를 이어 닫힌 선으로 그린다.
            let (c0, r0) = (p(7.5, 8.5), 5.5 * s);
            let (c1, r1) = (p(10.5, 5.5), 4.5 * s);
            let Some((i0, i1)) = circle_intersections(c0, r0, c1, r1) else { return };
            let mut points = arc_points(c0, r0, i0, i1, |q| (q - c1).length() >= r1);
            let inner = arc_points(c1, r1, i1, i0, |q| (q - c0).length() <= r0);
            points.extend(inner.into_iter().skip(1));
            painter.add(egui::Shape::closed_line(points, stroke));
        }
    }
}

/// 두 원의 교차점(없으면 None).
fn circle_intersections(c0: Pos2, r0: f32, c1: Pos2, r1: f32) -> Option<(Pos2, Pos2)> {
    let d = (c1 - c0).length();
    if d == 0.0 || d > r0 + r1 || d < (r0 - r1).abs() {
        return None;
    }
    let a = (r0 * r0 - r1 * r1 + d * d) / (2.0 * d);
    let h = (r0 * r0 - a * a).max(0.0).sqrt();
    let dir = (c1 - c0) / d;
    let m = c0 + dir * a;
    let perp = egui::vec2(-dir.y, dir.x) * h;
    Some((m + perp, m - perp))
}

/// 원 위에서 `from`→`to`로 가는 두 방향의 호 중, 가운데 점이 `keep`을 만족하는
/// 쪽을 점 목록으로 돌려준다(양 끝 포함).
fn arc_points(c: Pos2, r: f32, from: Pos2, to: Pos2, keep: impl Fn(Pos2) -> bool) -> Vec<Pos2> {
    use std::f32::consts::TAU;
    let a0 = (from - c).angle();
    let mut ccw = (to - c).angle() - a0;
    while ccw <= 0.0 {
        ccw += TAU;
    }
    let mid = c + egui::Vec2::angled(a0 + ccw / 2.0) * r;
    let sweep = if keep(mid) { ccw } else { ccw - TAU };
    const STEPS: usize = 20;
    (0..=STEPS).map(|i| c + egui::Vec2::angled(a0 + sweep * i as f32 / STEPS as f32) * r).collect()
}

/// 아이콘 세그먼트 버튼(시안의 "좌우/상하", "라이트/다크" 묶음). 눌린 항목의
/// 인덱스를 돌려준다. 선택된 칸은 살짝 떠 보이는 바탕으로 구분한다.
pub fn segmented(ui: &mut egui::Ui, pal: &Palette, items: &[(Icon, &str, bool)]) -> Option<usize> {
    const BTN: egui::Vec2 = vec2(28.0, 22.0);
    const PAD: f32 = 2.0;
    let size = vec2(BTN.x * items.len() as f32 + PAD * 2.0, BTN.y + PAD * 2.0);
    let (outer, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(outer.expand(1.0));
    painter.rect(outer, 7.0, pal.bg, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);

    let mut clicked = None;
    for (i, (icon, tip, selected)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(outer.min.x + PAD + BTN.x * i as f32, outer.min.y + PAD), BTN);
        let resp = ui
            .interact(r, ui.id().with(("segment", i, *tip)), egui::Sense::click())
            .on_hover_text(*tip);
        let fill = if *selected {
            pal.segment_on
        } else if resp.hovered() {
            pal.hover
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(r, 5.0, fill);
        let fg = if *selected || resp.hovered() { pal.text } else { pal.muted };
        paint_icon(&painter, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), *icon, fg);
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}
