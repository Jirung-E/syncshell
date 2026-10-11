//! UI 배율과 터미널 글자 크기(DEV-026). 둘은 따로 조절한다 — UI 배율은 egui
//! zoom_factor(앱 전체 크기), 터미널 글자 크기는 터미널 셀 폰트. UI 배율을 바꿔도
//! 터미널 글자의 실제 화면 크기는 그대로가 되도록, 터미널 폰트는 "글자 크기 /
//! UI 배율" pt로 그린다(egui pt × zoom = 화면 크기).
//!
//! 단축키: 터미널 글자 Ctrl/Cmd + = − 0(일반 터미널 관례), UI 배율 Ctrl/Cmd +
//! Shift + = − 0. egui 기본 키보드 확대는 같은 키를 쓰므로 끈다.

use eframe::egui;

pub const UI_DEFAULT: f32 = 1.0;
const UI_MIN: f32 = 0.5;
const UI_MAX: f32 = 2.0;
pub const FONT_DEFAULT: f32 = 16.0;
const FONT_MIN: f32 = 8.0;
const FONT_MAX: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    /// UI 배율(1.0 = 100%)
    pub ui: f32,
    /// 터미널 글자 크기(UI 배율 100% 기준 pt)
    pub font: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleAction {
    UiIn,
    UiOut,
    UiReset,
    FontIn,
    FontOut,
    FontReset,
}

impl Default for Scale {
    fn default() -> Self {
        Self { ui: UI_DEFAULT, font: FONT_DEFAULT }
    }
}

impl Scale {
    /// `state.toml`에서 읽은 값. 없거나 이상한 값(손으로 고쳤거나 NaN)은 기본값,
    /// 범위를 벗어나면 범위 안으로.
    pub fn from_saved(ui: Option<f32>, font: Option<f32>) -> Self {
        let pick = |v: Option<f32>, default: f32, min: f32, max: f32| match v {
            Some(v) if v.is_finite() => v.clamp(min, max),
            _ => default,
        };
        Self { ui: pick(ui, UI_DEFAULT, UI_MIN, UI_MAX), font: pick(font, FONT_DEFAULT, FONT_MIN, FONT_MAX) }
    }

    pub fn apply(&mut self, action: ScaleAction) {
        // UI 배율은 10% 단위 정수로 계산한다 — 0.1을 계속 더하면 부동소수 오차로
        // 110%가 109.99999%가 되는 식으로 어긋난다.
        let ui_step = |ui: f32, d: f32| (((ui * 10.0).round() + d) / 10.0).clamp(UI_MIN, UI_MAX);
        match action {
            ScaleAction::UiIn => self.ui = ui_step(self.ui, 1.0),
            ScaleAction::UiOut => self.ui = ui_step(self.ui, -1.0),
            ScaleAction::UiReset => self.ui = UI_DEFAULT,
            ScaleAction::FontIn => self.font = (self.font.round() + 1.0).clamp(FONT_MIN, FONT_MAX),
            ScaleAction::FontOut => self.font = (self.font.round() - 1.0).clamp(FONT_MIN, FONT_MAX),
            ScaleAction::FontReset => self.font = FONT_DEFAULT,
        }
    }

    /// 터미널을 그릴 egui pt — UI 배율로 나눠서 실제 화면 크기가 UI 배율과 무관하게.
    pub fn terminal_points(&self) -> f32 {
        self.font / self.ui
    }

    pub fn ui_percent(&self) -> i32 {
        (self.ui * 100.0).round() as i32
    }
}

/// 이번 프레임의 배율 단축키를 꺼내고 입력에서 지운다(터미널로 새지 않게).
/// 물리 키로 본다 — Shift를 같이 누르면 `=` 키가 `+`로 들어오는 등 논리 키가
/// 자판 배열·Shift에 따라 달라지기 때문.
pub fn take_shortcuts(ctx: &egui::Context) -> Vec<ScaleAction> {
    let mut actions = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } = e else {
                return true;
            };
            if !modifiers.command || modifiers.alt {
                return true;
            }
            let action = match (physical_key.unwrap_or(*key), modifiers.shift) {
                (egui::Key::Equals | egui::Key::Plus, false) => ScaleAction::FontIn,
                (egui::Key::Minus, false) => ScaleAction::FontOut,
                (egui::Key::Num0, false) => ScaleAction::FontReset,
                (egui::Key::Equals | egui::Key::Plus, true) => ScaleAction::UiIn,
                (egui::Key::Minus, true) => ScaleAction::UiOut,
                (egui::Key::Num0, true) => ScaleAction::UiReset,
                _ => return true,
            };
            actions.push(action);
            false
        });
    });
    actions
}

/// 상태바의 "UI 100% · 글자 16" 메뉴. 고른 동작을 돌려준다.
pub fn status_menu(ui: &mut egui::Ui, scale: &Scale, font: egui::FontId, color: egui::Color32) -> Option<ScaleAction> {
    let key = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
    let label = egui::RichText::new(format!("UI {}% · 글자 {}", scale.ui_percent(), scale.font.round() as i32))
        .font(font)
        .color(color);
    let mut picked = None;
    ui.menu_button(label, |ui| {
        let mut row = |ui: &mut egui::Ui, title: &str, value: String, out: ScaleAction, inn: ScaleAction, reset: ScaleAction, hint: String| {
            ui.label(egui::RichText::new(title).strong());
            ui.horizontal(|ui| {
                if ui.button("−").clicked() {
                    picked = Some(out);
                }
                ui.label(value);
                if ui.button("+").clicked() {
                    picked = Some(inn);
                }
                if ui.button("기본값").clicked() {
                    picked = Some(reset);
                }
            });
            ui.weak(hint);
        };
        row(
            ui,
            "UI 배율",
            format!("{}%", scale.ui_percent()),
            ScaleAction::UiOut,
            ScaleAction::UiIn,
            ScaleAction::UiReset,
            format!("{key}+Shift+= / − / 0"),
        );
        ui.separator();
        row(
            ui,
            "터미널 글자 크기",
            format!("{}", scale.font.round() as i32),
            ScaleAction::FontOut,
            ScaleAction::FontIn,
            ScaleAction::FontReset,
            format!("{key}+= / − / 0, {key}+휠"),
        );
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_clamp_and_stay_on_round_values() {
        let mut s = Scale::default();
        for _ in 0..3 {
            s.apply(ScaleAction::UiIn);
        }
        assert_eq!(s.ui_percent(), 130, "10% 단위로 정확히 올라가야 함(부동소수 오차 없이)");
        for _ in 0..30 {
            s.apply(ScaleAction::UiOut);
        }
        assert_eq!(s.ui_percent(), 50, "최소 50%에서 멈춰야 함");
        s.apply(ScaleAction::UiReset);
        assert_eq!(s.ui_percent(), 100);

        for _ in 0..100 {
            s.apply(ScaleAction::FontIn);
        }
        assert_eq!(s.font, 40.0, "최대 40에서 멈춰야 함");
        s.apply(ScaleAction::FontReset);
        assert_eq!(s.font, FONT_DEFAULT);
    }

    #[test]
    fn saved_values_are_validated() {
        assert_eq!(Scale::from_saved(None, None), Scale::default());
        assert_eq!(Scale::from_saved(Some(f32::NAN), Some(9999.0)), Scale { ui: UI_DEFAULT, font: 40.0 });
        assert_eq!(Scale::from_saved(Some(1.5), Some(12.0)), Scale { ui: 1.5, font: 12.0 });
    }

    /// UI 배율을 바꿔도 터미널 글자의 실제 화면 크기(pt × 배율)는 그대로여야 한다.
    #[test]
    fn terminal_size_on_screen_is_independent_of_ui_scale() {
        let mut s = Scale::default();
        let before = s.terminal_points() * s.ui;
        s.apply(ScaleAction::UiIn);
        s.apply(ScaleAction::UiIn);
        let after = s.terminal_points() * s.ui;
        assert!((before - after).abs() < 1e-4, "UI 배율에 따라 터미널 글자 화면 크기가 바뀜: {before} → {after}");
    }

    fn key(key: egui::Key, shift: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { shift, ..egui::Modifiers::COMMAND },
        }
    }

    /// 단축키가 터미널 글자/UI 배율로 갈리고, 처리한 키는 입력에서 지워져 터미널로 새지 않는지.
    #[test]
    fn shortcuts_are_split_and_consumed() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events = vec![
            key(egui::Key::Equals, false),
            key(egui::Key::Minus, true),
            key(egui::Key::Num0, false),
            key(egui::Key::A, false),
        ];
        let mut actions = Vec::new();
        let mut left = 0;
        let _ = ctx.run_ui(input, |ui| {
            actions = take_shortcuts(ui.ctx());
            left = ui.input(|i| i.events.len());
        });
        assert_eq!(actions, vec![ScaleAction::FontIn, ScaleAction::UiOut, ScaleAction::FontReset]);
        assert_eq!(left, 1, "처리한 단축키가 입력에 남아 있음(터미널로 샐 수 있음)");
    }
}
