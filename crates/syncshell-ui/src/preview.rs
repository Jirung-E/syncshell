//! 파일 미리보기 화면(DEV-029). 같은 내용을 두 곳에 그린다:
//! - 훑어보기 창(시안 C, 기본 동작) — 탐색기에서 파일을 고르고 Space. Space·Esc로
//!   닫고 ↑↓(←→)로 이전·다음 파일. 떠 있는 동안 키 입력은 터미널로 가지 않는다.
//! - 탐색기 안 미리보기 칸(시안 A, 옵션) — 경로바 옆 눈 버튼으로 켜고 끈다.
//!
//! 파일 읽기는 배경 스레드에서 core의 `preview::load`로 한다 — UI 스레드는 결과만
//! 받는다(큰 파일·느린 드라이브에서 화면이 멈추지 않게).

use crate::chrome::{self, Icon};
use crate::theme::Palette;
use eframe::egui::{self, vec2, Color32, Rect};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use syncshell_core::preview::{InfoReason, Preview, PreviewContent};

/// 미리보기 칸이 탐색기에서 차지하는 비율의 기본값·범위.
const PANE_FRAC_DEFAULT: f32 = 0.45;
const PANE_FRAC_RANGE: std::ops::RangeInclusive<f32> = 0.2..=0.8;

pub struct PreviewState {
    shown: Option<Shown>,
    /// 배경 스레드가 읽는 중인 경로
    pending: Option<PathBuf>,
    tx: Sender<Preview>,
    rx: Receiver<Preview>,
    /// 훑어보기 창이 떠 있는지
    pub quick_look: bool,
    /// 미리보기 칸이 탐색기 안에서 차지하는 비율(나누는 선을 끌어 바꾼다)
    pub pane_frac: f32,
}

/// 다 읽어서 그릴 준비가 된 미리보기. 이미지는 텍스처로 올린 뒤 RGBA는 버린다.
struct Shown {
    preview: Preview,
    texture: Option<egui::TextureHandle>,
}

impl Default for PreviewState {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { shown: None, pending: None, tx, rx, quick_look: false, pane_frac: PANE_FRAC_DEFAULT }
    }
}

impl PreviewState {
    /// `path`의 미리보기를 준비한다. 이미 보여 주거나 읽는 중이면 아무것도 안 한다.
    /// `fresh`면 보여 주던 것이라도 다시 읽는다(훑어보기 창을 새로 열 때 — 그새
    /// 파일이 바뀌었을 수 있다). 다시 읽는 동안에는 이전 내용을 그대로 보여 준다.
    pub fn request(&mut self, ctx: &egui::Context, path: &Path, fresh: bool) {
        if self.pending.as_deref() == Some(path) {
            return;
        }
        if !fresh && self.pending.is_none() && self.shown_path() == Some(path) {
            return;
        }
        self.pending = Some(path.to_path_buf());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(syncshell_core::preview::load(&path));
            ctx.request_repaint();
        });
    }

    /// 배경 스레드 결과를 받는다. 그새 다른 파일을 고른 경우의 늦은 결과는 버린다.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(mut preview) = self.rx.try_recv() {
            if self.pending.as_deref() != Some(preview.path.as_path()) {
                continue;
            }
            self.pending = None;
            let mut texture = None;
            match &mut preview.content {
                PreviewContent::Image { width, height, rgba, .. } => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([*width as usize, *height as usize], rgba);
                    texture = Some(ctx.load_texture(
                        format!("preview:{}", preview.path.display()),
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    *rgba = Vec::new();
                }
                // egui는 탭을 폭 없는 글자로, \r을 두부로 그린다.
                PreviewContent::Text { text, .. } if text.contains(['\t', '\r']) => {
                    *text = text.replace('\t', "    ").replace('\r', "");
                }
                _ => {}
            }
            self.shown = Some(Shown { preview, texture });
        }
    }

    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn shown_path(&self) -> Option<&Path> {
        self.shown.as_ref().map(|s| s.preview.path.as_path())
    }

    #[cfg(test)]
    pub fn shown_content(&self) -> Option<&PreviewContent> {
        self.shown.as_ref().map(|s| &s.preview.content)
    }
}

/// 미리보기 내용(머리줄 + 본문)을 `ui`의 남은 영역에 그린다.
pub fn body(ui: &mut egui::Ui, pal: &Palette, state: &PreviewState, selected: Option<&Path>) {
    let Some(selected) = selected else {
        centered_note(ui, pal, "파일을 선택하면 내용이 여기 보입니다");
        return;
    };
    let name = selected.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let shown = state.shown.as_ref().filter(|s| s.preview.path == selected);

    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
        chrome::paint_icon(ui.painter(), r, Icon::File, pal.muted);
        ui.add(egui::Label::new(egui::RichText::new(&name).color(pal.text).strong()).truncate());
        if let Some(shown) = shown {
            ui.add(egui::Label::new(egui::RichText::new(meta_line(&shown.preview)).color(pal.muted).small()).truncate());
        }
    });
    ui.add_space(2.0);
    let line = ui.available_rect_before_wrap();
    ui.painter().hline(line.x_range(), line.top(), egui::Stroke::new(1.0, pal.border));
    ui.add_space(4.0);

    let Some(shown) = shown else {
        centered_note(ui, pal, "불러오는 중…");
        return;
    };
    match &shown.preview.content {
        PreviewContent::Text { text, .. } if text.is_empty() => centered_note(ui, pal, "빈 파일입니다"),
        PreviewContent::Text { text, .. } => {
            egui::ScrollArea::both()
                .id_salt(("preview_text", ui.id()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text.as_str()).monospace().size(12.0).color(pal.text))
                            .extend(),
                    );
                });
        }
        PreviewContent::Image { .. } => {
            if let Some(tex) = &shown.texture {
                let area = ui.available_rect_before_wrap();
                let size = tex.size_vec2();
                // 영역에 맞춰 줄이되 원래 크기보다 키우지는 않는다.
                let scale = (area.width() / size.x).min(area.height() / size.y).min(1.0);
                let rect = Rect::from_center_size(area.center(), size * scale);
                ui.painter().image(tex.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                ui.allocate_rect(area, egui::Sense::hover());
            }
        }
        PreviewContent::Info(reason) => {
            let msg = match reason {
                InfoReason::Binary => "미리 볼 수 없는 형식입니다".to_string(),
                InfoReason::ImageTooLarge => format!(
                    "이미지가 커서 미리 보지 않습니다 ({} 초과)",
                    crate::file_panel::format_size(syncshell_core::preview::IMAGE_FILE_LIMIT)
                ),
                InfoReason::ImageUnreadable => "이미지를 읽을 수 없습니다".to_string(),
                InfoReason::NotFile => "파일이 아닙니다".to_string(),
            };
            centered_note(ui, pal, &msg);
        }
        PreviewContent::Error(e) => {
            centered_note(ui, pal, &format!("읽을 수 없습니다: {e}"));
        }
    }
}

/// "1.2 KB · UTF-8 · 2026-10-11 14:20" 같은 머리줄 보조 정보.
fn meta_line(p: &Preview) -> String {
    let mut parts = vec![crate::file_panel::format_size(p.size)];
    match &p.content {
        PreviewContent::Text { encoding, truncated, .. } => {
            parts.push((*encoding).to_string());
            if *truncated {
                parts.push("앞부분만".to_string());
            }
        }
        PreviewContent::Image { original: (w, h), .. } => parts.push(format!("{w}×{h}")),
        _ => {}
    }
    if let Some(t) = p.modified {
        parts.push(crate::file_panel::format_modified(t));
    }
    parts.join(" · ")
}

fn centered_note(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    let area = ui.available_rect_before_wrap();
    ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)), |ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).color(pal.muted)).wrap());
    });
}

/// 탐색기 영역을 목록과 미리보기 칸으로 나눈다. 긴 쪽을 나눈다 — 좌우 분할(세로로
/// 긴 탐색기)이면 목록 아래, 상하 분할(가로로 긴 탐색기)이면 목록 오른쪽.
/// 돌려주는 값: (목록 영역, 나누는 선 영역, 미리보기 영역).
pub fn split_pane(area: Rect, frac: f32) -> (Rect, Rect, Rect) {
    const GAP: f32 = 6.0;
    if area.height() >= area.width() {
        let pv_h = ((area.height() - GAP) * frac).round();
        let list = Rect::from_min_max(area.min, egui::pos2(area.right(), area.bottom() - pv_h - GAP));
        let sep = Rect::from_min_max(egui::pos2(area.left(), list.bottom()), egui::pos2(area.right(), list.bottom() + GAP));
        let pv = Rect::from_min_max(egui::pos2(area.left(), sep.bottom()), area.max);
        (list, sep, pv)
    } else {
        let pv_w = ((area.width() - GAP) * frac).round();
        let list = Rect::from_min_max(area.min, egui::pos2(area.right() - pv_w - GAP, area.bottom()));
        let sep = Rect::from_min_max(egui::pos2(list.right(), area.top()), egui::pos2(list.right() + GAP, area.bottom()));
        let pv = Rect::from_min_max(egui::pos2(sep.right(), area.top()), area.max);
        (list, sep, pv)
    }
}

/// 나누는 선: 끌면 비율이 바뀐다.
pub fn pane_divider(ui: &mut egui::Ui, pal: &Palette, area: Rect, sep: Rect, state: &mut PreviewState) {
    let vertical = area.height() >= area.width();
    let resp = ui.interact(sep, ui.id().with("preview_divider"), egui::Sense::drag());
    let hot = resp.hovered() || resp.dragged();
    if hot {
        ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
    }
    if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let frac = if vertical {
                (area.bottom() - pos.y) / area.height()
            } else {
                (area.right() - pos.x) / area.width()
            };
            state.pane_frac = frac.clamp(*PANE_FRAC_RANGE.start(), *PANE_FRAC_RANGE.end());
        }
    }
    let color = if hot { pal.accent } else { pal.border };
    if vertical {
        ui.painter().hline(sep.x_range(), sep.center().y, egui::Stroke::new(1.0, color));
    } else {
        ui.painter().vline(sep.center().x, sep.y_range(), egui::Stroke::new(1.0, color));
    }
}

/// 훑어보기 창. 닫아야 하면(바깥 클릭·닫기 버튼) true.
pub fn quick_look_window(ctx: &egui::Context, pal: &Palette, state: &PreviewState, selected: &Path) -> bool {
    let screen = ctx.content_rect();
    let size = vec2((screen.width() * 0.8).min(1100.0), (screen.height() * 0.8).min(800.0));
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new("quick_look"))
        .frame(
            egui::Frame::NONE
                .fill(pal.panel)
                .stroke(egui::Stroke::new(1.0, pal.border))
                .corner_radius(10)
                .inner_margin(egui::Margin::same(12)),
        )
        .show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Space·Esc 닫기   ↑↓ 이전·다음 파일").color(pal.muted).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::click());
                    let resp = resp.on_hover_text("닫기");
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 4.0, pal.hover);
                    }
                    chrome::paint_icon(ui.painter(), r.shrink(4.0), Icon::Close, if resp.hovered() { pal.text } else { pal.muted });
                    close |= resp.clicked();
                });
            });
            body(ui, pal, state, Some(selected));
        });
    #[cfg(test)]
    tests::record_quick_look_rect_for_test(modal.response.rect);
    close || modal.backdrop_response.clicked()
}

/// 이 프레임의 키 입력 중 Space(보조키 없음) 누름을 가져간다 — 뒤따르는 " " 글자도
/// 같이 지워 터미널에 공백이 들어가지 않게 한다.
pub fn take_space(ctx: &egui::Context) -> bool {
    let mut hit = false;
    ctx.input_mut(|i| {
        let mut drop_text = false;
        i.events.retain(|e| match e {
            egui::Event::Key { key: egui::Key::Space, pressed, modifiers, .. } if modifiers.is_none() => {
                hit |= *pressed;
                drop_text = *pressed;
                false
            }
            egui::Event::Text(t) if drop_text && t == " " => {
                drop_text = false;
                false
            }
            _ => {
                drop_text = false;
                true
            }
        });
    });
    hit
}

/// 훑어보기 창이 떠 있는 동안의 키 입력: 닫기 여부와 이동 칸 수(−1/+1)를 돌려주고,
/// 키·글자·붙여넣기·IME 입력은 모두 지운다 — 창이 떠 있는 동안 터미널에 아무것도
/// 들어가지 않게. 복사(Copy) 이벤트는 남겨 미리보기 글자 선택 복사는 된다.
pub fn take_quick_look_keys(ctx: &egui::Context) -> (bool, i32) {
    let mut close = false;
    let mut step = 0;
    ctx.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                if modifiers.is_none() {
                    match key {
                        egui::Key::Space | egui::Key::Escape => close = true,
                        egui::Key::ArrowUp | egui::Key::ArrowLeft => step -= 1,
                        egui::Key::ArrowDown | egui::Key::ArrowRight => step += 1,
                        _ => {}
                    }
                }
                false
            }
            egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Ime(_) => false,
            _ => true,
        });
    });
    (close, step)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static QUICK_LOOK_RECT: Cell<Option<Rect>> = const { Cell::new(None) };
    }

    pub fn record_quick_look_rect_for_test(r: Rect) {
        QUICK_LOOK_RECT.with(|c| c.set(Some(r)));
    }

    pub fn take_quick_look_rect() -> Option<Rect> {
        QUICK_LOOK_RECT.with(|c| c.take())
    }

    #[test]
    fn split_pane_divides_the_long_side() {
        let tall = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(300.0, 806.0));
        let (list, sep, pv) = split_pane(tall, 0.5);
        assert_eq!((list.width(), pv.width()), (300.0, 300.0), "세로로 긴 탐색기는 위아래로 나눔");
        assert_eq!(list.bottom(), sep.top());
        assert_eq!(pv.height(), 400.0);
        assert_eq!(pv.bottom(), 806.0);

        let wide = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1006.0, 300.0));
        let (list, _, pv) = split_pane(wide, 0.3);
        assert_eq!((list.height(), pv.height()), (300.0, 300.0), "가로로 긴 탐색기는 좌우로 나눔");
        assert_eq!(pv.width(), 300.0);
        assert!(list.right() < pv.left());
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn space_is_taken_with_its_text_and_other_input_left_alone() {
        let ctx = egui::Context::default();
        let mut hit = None;
        let input = egui::RawInput {
            events: vec![key(egui::Key::Space, egui::Modifiers::NONE), egui::Event::Text(" ".into()), egui::Event::Text("a".into())],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            hit = Some(take_space(ui.ctx()));
            let left: Vec<_> = ui.input(|i| i.events.clone());
            assert_eq!(left, vec![egui::Event::Text("a".into())]);
        });
        assert_eq!(hit, Some(true));

        // Shift+Space 등 보조키가 있으면 가져가지 않는다
        let input = egui::RawInput {
            events: vec![key(egui::Key::Space, egui::Modifiers::SHIFT), egui::Event::Text(" ".into())],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            assert!(!take_space(ui.ctx()));
            assert_eq!(ui.input(|i| i.events.len()), 2);
        });
    }

    #[test]
    fn quick_look_keys_close_step_and_swallow_typing() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![
                key(egui::Key::ArrowDown, egui::Modifiers::NONE),
                key(egui::Key::ArrowDown, egui::Modifiers::NONE),
                key(egui::Key::ArrowUp, egui::Modifiers::NONE),
                key(egui::Key::A, egui::Modifiers::NONE),
                egui::Event::Text("a".into()),
                key(egui::Key::C, egui::Modifiers::CTRL),
                egui::Event::Copy,
            ],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            assert_eq!(take_quick_look_keys(ui.ctx()), (false, 1));
            let left: Vec<_> = ui.input(|i| i.events.clone());
            assert_eq!(left, vec![egui::Event::Copy], "터미널로 갈 입력이 남음");
        });
        let input = egui::RawInput { events: vec![key(egui::Key::Escape, egui::Modifiers::NONE)], ..Default::default() };
        let _ = ctx.run_ui(input, |ui| assert_eq!(take_quick_look_keys(ui.ctx()), (true, 0)));
    }
}
