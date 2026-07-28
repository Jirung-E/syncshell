use eframe::egui;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

/// 시스템 폰트 전체 열거(`load_system_fonts()`)는 디스크에서 수백 개 폰트
/// 파일의 메타데이터를 읽는 무거운 작업이다. `TerminalWidget`(따라서
/// `FontFallback`) 인스턴스마다 따로 하면 낭비이자(테스트에서 여러 인스턴스가
/// 동시에 스캔하면 서로 경합해 실제 셸 응답까지 느려지는 것을 확인함) 애초에
/// 시스템 폰트 목록은 프로세스당 하나면 충분한 자원이라, 프로세스 전체에서
/// 한 번만 로드해 공유한다.
fn shared_db() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// 시스템 폰트에서 실제 글리프를 가져오는 코드포인트별 폴백 체인(DEV-011).
///
/// 방침: 특정 언어(한글 등)를 겨냥한 폰트를 프로젝트에 번들하지 않는다 —
/// 한글만 넣으면 일본어·중국어·아랍어 사용자에게 무의미하고 임의적이다.
/// 대신 `fontdb`로 지금 실행 중인 시스템에 실제로 설치된 폰트를 열거해서,
/// 필요한 코드포인트가 나올 때마다 그걸 담고 있는 폰트를 찾아 그때그때
/// egui에 등록한다. 번들 폰트는 전혀 없다 — 이 프로젝트가 지금 지원하는
/// 유일한 OS(Windows)는 Courier New(순정 monospace)를 항상 갖고 있어서,
/// fontdb의 내장 기본값(`Family::Monospace` → "Courier New")이 사실상
/// "시스템에서 아무것도 못 찾았을 때의 최후 폴백" 역할을 대신한다.
pub struct FontFallback {
    db: Arc<fontdb::Database>,
    /// 이미 처리(성공이든 실패든)를 시도한 글자 — 매 프레임 fontdb를 다시
    /// 뒤지지 않기 위한 캐시. 시스템에 정말 없는 글자(예: 존재하지 않는
    /// 코드포인트)라도 한 번만 찾아보고 그 뒤로는 조용히 두부로 남는다.
    tried: HashSet<char>,
    loaded_families: HashSet<String>,
    fonts: egui::FontDefinitions,
}

impl FontFallback {
    pub fn new() -> Self {
        Self {
            db: shared_db(),
            tried: HashSet::new(),
            loaded_families: HashSet::new(),
            fonts: egui::FontDefinitions::default(),
        }
    }

    /// 터미널의 주 폰트(라틴 고정폭)를 설치한다. 특정 폰트 파일 경로를
    /// 하드코딩하지 않고, 잘 알려진 monospace 폰트 이름 후보를 fontdb로
    /// 조회해서 시스템에 실제로 깔린 첫 번째 것을 쓴다 — 하나도 없으면
    /// fontdb 기본값(Courier New)으로 자연히 떨어진다.
    ///
    /// 탐색기 라벨 등 일반 UI 텍스트(Proportional)용 폰트도 같이 설치한다 —
    /// DEV-011의 대상(터미널 코드포인트 폴백)은 아니지만, "폰트 파일 경로를
    /// 코드에 하드코딩하지 않고 이름으로 시스템에 질의한다"는 같은 원칙을
    /// 일관되게 적용한다. UI 라벨은 개발자가 미리 정한 고정 문자열(한글+영문)만
    /// 쓰므로, 터미널처럼 매 프레임 동적 코드포인트 폴백까지는 필요 없다.
    pub fn install_primary(&mut self, ctx: &egui::Context) {
        self.install_family(&["Cascadia Mono", "Consolas", "Cascadia Code", "Courier New"], egui::FontFamily::Monospace);
        self.install_family(&["Malgun Gothic", "Segoe UI"], egui::FontFamily::Proportional);
        ctx.set_fonts(self.fonts.clone());
    }

    fn install_family(&mut self, candidates: &[&str], target: egui::FontFamily) {
        for name in candidates {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                ..Default::default()
            };
            if let Some(id) = self.db.query(&query) {
                if let Some(bytes) = self.db.with_face_data(id, |data, _| data.to_vec()) {
                    self.register_font(name, bytes, &target, true);
                    return;
                }
            }
        }
    }

    fn register_font(&mut self, family: &str, bytes: Vec<u8>, target: &egui::FontFamily, primary: bool) {
        if self.loaded_families.insert(family.to_owned()) {
            self.fonts
                .font_data
                .insert(family.to_owned(), egui::FontData::from_owned(bytes).into());
        }
        if let Some(list) = self.fonts.families.get_mut(target) {
            if primary {
                list.insert(0, family.to_owned());
            } else if !list.contains(&family.to_owned()) {
                list.push(family.to_owned());
            }
        }
    }

    /// 주어진 텍스트에 지금 로드된 폰트로 못 그리는 글자가 있으면, 시스템
    /// 폰트 중 그 글자를 가진 걸 찾아 동적으로 추가한다. 새로 추가된 게
    /// 있으면 true를 돌려준다 — 호출부는 이 프레임 안에서 다시 그리거나
    /// (같은 프레임의 뒤이은 draw 호출부터 바로 정상 렌더링됨) repaint를
    /// 요청해야 한다.
    pub fn ensure_covers(&mut self, ctx: &egui::Context, font_id: &egui::FontId, text: &str) -> bool {
        let mut added = false;
        for c in text.chars() {
            if c == ' ' || c == '\0' || self.tried.contains(&c) {
                continue;
            }
            self.tried.insert(c);

            let covered = ctx.fonts_mut(|f| f.has_glyph(font_id, c));
            if covered {
                continue;
            }

            if let Some((family, bytes)) = self.find_system_font_for(c) {
                self.register_font(&family, bytes, &egui::FontFamily::Monospace, false);
                added = true;
            }
        }
        if added {
            ctx.set_fonts(self.fonts.clone());
        }
        added
    }

    /// 이 코드포인트를 담고 있는 시스템 폰트를 찾는다. 고정폭 폰트를
    /// 우선하고(터미널 셀 그리드와 어울림), 없으면 비고정폭이라도 있는 걸
    /// 쓴다 — 두부보다는 비율이 안 맞더라도 글자가 보이는 쪽이 낫다.
    fn find_system_font_for(&self, c: char) -> Option<(String, Vec<u8>)> {
        let mut proportional_fallback: Option<fontdb::ID> = None;
        for face in self.db.faces() {
            if !face_has_glyph(&self.db, face.id, c) {
                continue;
            }
            if face.monospaced {
                let name = face.families.first()?.0.clone();
                let bytes = self.db.with_face_data(face.id, |data, _| data.to_vec())?;
                return Some((name, bytes));
            }
            if proportional_fallback.is_none() {
                proportional_fallback = Some(face.id);
            }
        }

        let id = proportional_fallback?;
        let info = self.db.face(id)?;
        let name = info.families.first()?.0.clone();
        let bytes = self.db.with_face_data(id, |data, _| data.to_vec())?;
        Some((name, bytes))
    }
}

impl Default for FontFallback {
    fn default() -> Self {
        Self::new()
    }
}

fn face_has_glyph(db: &fontdb::Database, id: fontdb::ID, c: char) -> bool {
    db.with_face_data(id, |data, index| {
        ttf_parser::Face::parse(data, index)
            .ok()
            .and_then(|face| face.glyph_index(c))
            .is_some()
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_frame(ctx: &egui::Context) {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)));
        let _ = ctx.run_ui(input, |_ui| {});
    }

    fn ctx_with_frame() -> egui::Context {
        let ctx = egui::Context::default();
        step_frame(&ctx);
        ctx
    }

    /// DEV-011 회귀 테스트: 주 폰트(Consolas 등)가 실제 monospace인지 —
    /// 라틴 글리프 폭이 문자마다 다르면 셀 렌더러에서 글자가 겹쳐 보이는
    /// 버그가 재발한다(사용자 피드백: "터미널에서 글자가 겹쳐서 출력").
    #[test]
    fn primary_font_is_truly_monospace_for_latin() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);
        // set_fonts()는 egui 0.35 기준 "다음 pass 시작 시점"에야 적용된다 —
        // 프레임을 한 번 더 돌려야 방금 등록한 폰트가 실제로 활성화된다.
        // 이걸 빼먹으면 egui 기본 번들 폰트를 검사하게 되어(우연히 그것도
        // monospace라) 이 테스트가 Consolas를 전혀 안 쓰고도 통과해버린다.
        step_frame(&ctx);
        let font_id = egui::FontId::monospace(16.0);

        let widths: Vec<f32> = ctx.fonts_mut(|f| {
            ['M', 'W', 'A', 'i', 'l', '1', '.', '-', '>', 'P']
                .iter()
                .map(|&c| f.glyph_width(&font_id, c))
                .collect()
        });
        let first = widths[0];
        assert!(
            widths.iter().all(|w| (w - first).abs() < 0.01),
            "주 폰트의 라틴 글리프 폭이 문자마다 다름(겹침 버그 재발 위험): {widths:?}"
        );
    }

    /// DEV-011 핵심: 주 폰트에 없는 글자(한글)가 코드포인트 폴백으로 실제
    /// 그려지는지 — 두부(□)로 안 남는지 확인한다. 특정 언어 폰트를 코드에
    /// 미리 등록해두지 않고, ensure_covers가 그때그때 시스템에서 찾아야 한다.
    #[test]
    fn ensure_covers_finds_system_font_for_uncovered_hangul() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);
        step_frame(&ctx); // set_fonts()는 다음 pass부터 적용됨 — 위 step_frame 주석 참고
        let font_id = egui::FontId::monospace(16.0);

        let covered_before = ctx.fonts_mut(|f| f.has_glyph(&font_id, '가'));
        assert!(!covered_before, "주 폰트가 이미 한글을 커버함 — 테스트 전제가 깨짐(주 폰트 후보 확인 필요)");

        let added = fallback.ensure_covers(&ctx, &font_id, "안녕하세요");
        assert!(added, "한글 코드포인트에 대해 시스템 폰트를 새로 못 찾음");
        step_frame(&ctx);

        let covered_after = ctx.fonts_mut(|f| f.has_glyph(&font_id, '가'));
        assert!(covered_after, "폴백 등록 후에도 한글 글리프가 없음");
    }

    /// 같은 글자를 반복 요청해도 fontdb 스캔/폰트 등록이 한 번만 일어나는지
    /// (매 프레임 반복 호출되는 게 실사용 패턴이라 중요).
    #[test]
    fn ensure_covers_does_not_reload_already_resolved_char() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);
        let font_id = egui::FontId::monospace(16.0);

        assert!(fallback.ensure_covers(&ctx, &font_id, "가"));
        assert!(
            !fallback.ensure_covers(&ctx, &font_id, "가"),
            "이미 해결된 글자를 다시 새로 추가된 것처럼 보고함"
        );
    }
}
