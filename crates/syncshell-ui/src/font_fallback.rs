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
    /// 굵게(BOLD) 표시용 별도 폰트 weight를 실제로 찾아 설치했으면 그 family.
    /// 못 찾았으면(시스템에 진짜 굵은 face가 없음) `None` — 이때는 색만
    /// 밝게(`ansi_color::resolve_fg_bold_aware`) 표현하는 걸로 대체한다.
    bold_family: Option<egui::FontFamily>,
}

impl FontFallback {
    pub fn new() -> Self {
        Self {
            db: shared_db(),
            tried: HashSet::new(),
            loaded_families: HashSet::new(),
            fonts: egui::FontDefinitions::default(),
            bold_family: None,
        }
    }

    /// 굵게 표시용으로 실제 로드된 폰트 family(있으면). 없으면 호출부가 일반
    /// 폰트로 그리고 색만 밝게 표현하는 기존 방식으로 대체해야 한다.
    pub fn bold_monospace_family(&self) -> Option<egui::FontFamily> {
        self.bold_family.clone()
    }

    /// 터미널의 주 폰트(라틴 고정폭)를 설치한다. 특정 폰트 파일 경로를
    /// 하드코딩하지 않고, 잘 알려진 monospace 폰트 이름 후보를 fontdb로
    /// 조회해서 시스템에 실제로 깔린 첫 번째 것을 쓴다 — 하나도 없으면
    /// fontdb 기본값(Courier New)으로 자연히 떨어진다.
    ///
    /// macOS 후보로 "SF Mono"를 먼저 넣었었는데, 실측해보니 fontdb로는
    /// 조회가 안 된다(Apple이 이름 질의로는 노출하지 않음 — 이름은 있어도
    /// `fontdb::Query`가 못 찾음). 그래서 이전 후보들(Windows용 Cascadia/
    /// Consolas)이 전부 없는 macOS에서는 곧장 "Courier New"(오래된 타자기
    /// 느낌의 폰트, macOS 터미널 기본값이 아님)까지 떨어져버렸다 — 사용자가
    /// "원하던 시스템 폰트가 아니다"라고 지적한 지점. "Menlo"(macOS
    /// Terminal.app 기본 monospace, fontdb로 조회 확인됨)를 macOS 후보로
    /// 추가해서 Courier New 이전에 걸리게 한다.
    ///
    /// 탐색기 라벨 등 일반 UI 텍스트(Proportional)용 폰트도 같이 설치한다 —
    /// DEV-011의 대상(터미널 코드포인트 폴백)은 아니지만, "폰트 파일 경로를
    /// 코드에 하드코딩하지 않고 이름으로 시스템에 질의한다"는 같은 원칙을
    /// 일관되게 적용한다. UI 라벨은 개발자가 미리 정한 고정 문자열(한글+영문)만
    /// 쓰므로, 터미널처럼 매 프레임 동적 코드포인트 폴백까지는 필요 없다.
    pub fn install_primary(&mut self, ctx: &egui::Context) {
        let primary_mono = self.install_family(
            &["Cascadia Mono", "Consolas", "Cascadia Code", "Menlo", "Monaco", "Andale Mono", "Courier New"],
            egui::FontFamily::Monospace,
        );
        // 한글+영문을 같이 커버하는 후보를 OS별로 하나씩 둔다 — "Malgun Gothic"/
        // "Segoe UI"는 Windows에만 있어서, macOS에서는 그동안 하나도 안 걸려
        // Proportional이 egui 기본 번들 폰트(한글 미지원)로 남아 있었다. 결과로
        // 탐색기의 한글 폴더/파일명이 두부(□)로 나오는 문제였다 — fontdb로 실측
        // 확인(macOS: "Apple SD Gothic Neo" 있음, 한글+라틴 전체 커버).
        self.install_family(
            &["Malgun Gothic", "Apple SD Gothic Neo", "Noto Sans CJK KR", "NanumGothic", "Segoe UI"],
            egui::FontFamily::Proportional,
        );
        // 실사용 피드백: "굵은 글씨가 제대로 표시되지 않음" — 예전엔 BOLD를
        // 밝은 색으로만 표현했는데(ansi_color::resolve_fg_bold_aware), 진짜
        // 굵은 획으로 보이길 기대하는 게 자연스럽다(대부분의 터미널 에뮬레이터
        // 관례). 주 폰트와 "같은 family"의 진짜 굵은 weight를 찾아서 쓴다 —
        // 엉뚱한 다른 폰트를 굵게 대신 쓰면 자모 폭·모양이 안 맞아 오히려
        // 어색해진다.
        if let Some(name) = primary_mono {
            self.install_bold_variant(&name);
        }
        ctx.set_fonts(self.fonts.clone());
    }

    /// `primary`와 같은 family 이름으로 진짜 굵은(weight>=700) face를 찾아서
    /// 있으면 설치한다. fontdb는 굵은 face가 없을 때 조용히 가장 가까운
    /// weight(대개 그냥 Regular)로 대체해서 돌려준다 — 실측으로 확인(macOS
    /// "Monaco"는 Bold 질의에도 weight 400짜리를 돌려줌, 진짜 Bold face 자체가
    /// 없는 폰트라서). 그런 "가짜 굵음"은 설치하지 않는다 — 어차피 Regular랑
    /// 똑같이 그려질 걸 별도 family로 등록해봐야 의미가 없고, 나중에 이
    /// 폰트가 진짜론 안 굵다는 걸 놓친 채로 남을 수 있어서 아예 스킵하는 쪽이
    /// 낫다(호출부는 `bold_monospace_family() == None`이면 알아서 기존 방식
    /// — 색만 밝게 — 으로 대체함).
    fn install_bold_variant(&mut self, primary: &str) {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(primary)],
            weight: fontdb::Weight::BOLD,
            ..Default::default()
        };
        let Some(id) = self.db.query(&query) else { return };
        let Some(face) = self.db.face(id) else { return };
        if face.weight.0 < fontdb::Weight::BOLD.0 {
            return;
        }
        let Some((bytes, index)) = self.db.with_face_data(id, |data, index| (data.to_vec(), index)) else { return };

        let bold_family = egui::FontFamily::Name("syncshell-monospace-bold".into());
        // font_data 키는 원래 family 이름과 겹치면 안 된다(`register_font`가
        // `loaded_families`로 중복 등록을 걸러내므로, 같은 이름이면 Regular
        // face가 이미 차지한 자리를 Bold가 덮어쓰지 못하고 조용히 무시됨).
        let bold_key = format!("{primary} Bold");
        self.register_font(&bold_key, bytes, index, &bold_family, true);
        self.bold_family = Some(bold_family);
    }

    /// 시스템에 실제로 있는 첫 후보를 설치하고, 그 family 이름을 돌려준다
    /// (호출부가 같은 family의 다른 weight를 이어서 찾을 때 씀 — 예: 굵게).
    fn install_family(&mut self, candidates: &[&str], target: egui::FontFamily) -> Option<String> {
        for name in candidates {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                ..Default::default()
            };
            if let Some(id) = self.db.query(&query) {
                // with_face_data의 두 번째 콜백 인자가 이 id가 가리키는 TTC
                // 파일 안에서의 face 인덱스다 — 버리면 안 된다(아래 register_font
                // 주석 참고).
                if let Some((bytes, index)) = self.db.with_face_data(id, |data, index| (data.to_vec(), index)) {
                    self.register_font(name, bytes, index, &target, true);
                    return Some((*name).to_string());
                }
            }
        }
        None
    }

    fn register_font(&mut self, family: &str, bytes: Vec<u8>, index: u32, target: &egui::FontFamily, primary: bool) {
        if self.loaded_families.insert(family.to_owned()) {
            // macOS 시스템 폰트는 여러 굵기/스타일이 한 .ttc(TrueType Collection)
            // 파일에 같이 담기는 경우가 흔하다(예: AppleSDGothicNeo.ttc). `index`
            // 없이 FontData::from_owned()(항상 face 0으로 고정)를 쓰면, 우리가
            // 찾은 게 그 파일의 face 0이 아닐 때 완전히 다른 스타일이 로드되거나
            // 최악의 경우 glyph_index 조회가 전부 실패한다 — 실측: "Apple SD
            // Gothic Neo"를 index 무시하고 등록했더니 한글 글리프를 하나도 못
            // 찾음(macOS 이식 중 발견, 탐색기 한글 파일명이 두부로 보이는 문제의
            // 원인이었음).
            self.fonts.font_data.insert(
                family.to_owned(),
                egui::FontData { font: bytes.into(), index, tweak: Default::default() }.into(),
            );
        }
        // `entry().or_default()` — `egui::FontDefinitions::default()`는
        // Proportional/Monospace만 미리 채워두고, 굵게용으로 새로 만드는
        // 커스텀 `FontFamily::Name`은 처음 보는 키라서 `get_mut`으로는
        // 아무 항목도 못 찾아 조용히 무시된다(굵게 등록이 아예 안 먹히는
        // 버그였음 — 등록 직후 테스트에서 발견).
        let list = self.fonts.families.entry(target.clone()).or_default();
        if primary {
            list.insert(0, family.to_owned());
        } else if !list.contains(&family.to_owned()) {
            list.push(family.to_owned());
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

            if let Some((family, bytes, index)) = self.find_system_font_for(c) {
                self.register_font(&family, bytes, index, &egui::FontFamily::Monospace, false);
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
    fn find_system_font_for(&self, c: char) -> Option<(String, Vec<u8>, u32)> {
        let mut proportional_fallback: Option<fontdb::ID> = None;
        for face in self.db.faces() {
            if !face_has_glyph(&self.db, face.id, c) {
                continue;
            }
            if face.monospaced {
                let name = face.families.first()?.0.clone();
                let (bytes, index) = self.db.with_face_data(face.id, |data, index| (data.to_vec(), index))?;
                return Some((name, bytes, index));
            }
            if proportional_fallback.is_none() {
                proportional_fallback = Some(face.id);
            }
        }

        let id = proportional_fallback?;
        let info = self.db.face(id)?;
        let name = info.families.first()?.0.clone();
        let (bytes, index) = self.db.with_face_data(id, |data, index| (data.to_vec(), index))?;
        Some((name, bytes, index))
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

    /// 실사용 피드백: "글자체가 원하던 시스템 폰트가 아님". macOS에서
    /// "SF Mono"는 fontdb 이름 질의로는 못 찾는다(Apple이 노출 안 함) —
    /// 그래서 Windows용 후보(Cascadia/Consolas)가 전부 없는 macOS에서는
    /// "Menlo"(macOS Terminal.app 기본 monospace) 없이 곧장 낡은 느낌의
    /// "Courier New"까지 떨어져버렸었다. 시스템에 Menlo가 있는 한(대부분의
    /// macOS) 그게 선택돼야 하고, Courier New까지 떨어지면 안 된다.
    #[test]
    #[cfg(target_os = "macos")]
    fn macos_primary_monospace_prefers_menlo_over_courier_new() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);

        let db = shared_db();
        let has_menlo = db
            .query(&fontdb::Query { families: &[fontdb::Family::Name("Menlo")], ..Default::default() })
            .is_some();
        if !has_menlo {
            eprintln!("이 머신엔 Menlo가 없음 — 테스트 전제가 다름, 스킵");
            return;
        }

        let chosen = fallback.fonts.families.get(&egui::FontFamily::Monospace).and_then(|list| list.first());
        assert_eq!(
            chosen.map(String::as_str),
            Some("Menlo"),
            "Menlo가 시스템에 있는데도 주 monospace 폰트로 안 골라짐(실제로는: {chosen:?}) \
             — Courier New 같은 낡은 폴백까지 떨어졌을 가능성"
        );
    }

    /// 실사용 피드백: "굵은 글씨가 제대로 표시되지 않음" — 색만 밝게(bright)
    /// 표현하던 기존 방식에 더해, 시스템에 실제 굵은(weight>=700) face가
    /// 있으면 그걸 로드해서 진짜 굵은 획으로도 그려져야 한다. macOS의 Menlo는
    /// 진짜 Bold face를 갖고 있으므로(fontdb 실측 확인), 여기서는 그게 실제로
    /// 로드됐는지 확인한다.
    ///
    /// 글리프 가로 폭(`glyph_width`)으로 비교하려던 첫 시도는 틀렸다 — 진짜
    /// monospace 폰트는 Bold든 Regular든 셀 정렬을 위해 advance width가
    /// **의도적으로 같다**(실측: Menlo 'M' 둘 다 9.6328125px, 두꺼워지는 건
    /// 획 두께지 칸 너비가 아님). 원본 파일 바이트로 비교하려던 두 번째 시도도
    /// 틀렸다 — macOS는 Regular/Bold를 같은 `.ttc`(TrueType Collection) 파일
    /// 하나에 묶어 담으므로, 두 face가 정확히 같은 파일 바이트를 공유하는 게
    /// 정상이다(실측 확인: 둘 다 `ttcf` 매직으로 시작하는 동일 바이트).
    /// 그 파일 **안에서 어떤 face를 가리키는지**(`index`)가 실제로 다른지로
    /// 확인해야 한다 — 같은 index면 사실상 Regular를 중복 등록한 것.
    #[test]
    #[cfg(target_os = "macos")]
    fn bold_variant_is_loaded_as_a_distinct_face_when_available() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);

        let db = shared_db();
        let has_menlo_bold = db
            .query(&fontdb::Query {
                families: &[fontdb::Family::Name("Menlo")],
                weight: fontdb::Weight::BOLD,
                ..Default::default()
            })
            .is_some();
        if !has_menlo_bold {
            eprintln!("이 머신엔 Menlo Bold가 없음 — 테스트 전제가 다름, 스킵");
            return;
        }

        assert!(fallback.bold_monospace_family().is_some(), "Menlo Bold가 시스템에 있는데도 굵게용 폰트가 설치 안 됨");

        let regular_index = fallback.fonts.font_data.get("Menlo").map(|d| d.index);
        let bold_index = fallback.fonts.font_data.get("Menlo Bold").map(|d| d.index);
        assert!(regular_index.is_some(), "Regular Menlo가 font_data에 없음");
        assert!(bold_index.is_some(), "Menlo Bold가 font_data에 없음 — 굵게용 등록 자체가 안 됨");
        assert_ne!(
            regular_index, bold_index,
            "Regular와 Bold가 같은 face index를 가리킴 — 진짜 다른 face를 못 가져오고 \
             Regular를 그대로 중복 등록했을 가능성"
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

    /// DEV-011 Test plan: "한글·일본어·중국어·이모지가 모두 정상 표시". 위 한글
    /// 테스트와 같은 메커니즘(코드포인트별 시스템 폰트 조회)이 언어를 안 가리는지
    /// 다른 스크립트로도 확인한다 — 하나만 확인하면 우연히 한글 폰트가 일본어/
    /// 중국어까지 다 커버하는 시스템에서 다른 스크립트가 빠진 걸 놓칠 수 있다.
    #[test]
    fn ensure_covers_finds_system_font_for_japanese_chinese_and_emoji() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);
        step_frame(&ctx);
        let font_id = egui::FontId::monospace(16.0);

        // (표시 이름, 코드포인트) — 못 그린 상태에서 출발해야 하는(동적 폴백을
        // 실제로 타야 하는) 경우만.
        for (label, c) in [("일본어 히라가나", 'あ'), ("중국어 한자(간체)", '汉')] {
            let covered_before = ctx.fonts_mut(|f| f.has_glyph(&font_id, c));
            assert!(!covered_before, "[{label}] 주 폰트가 이미 커버함 — 테스트 전제가 깨짐");

            let added = fallback.ensure_covers(&ctx, &font_id, &c.to_string());
            assert!(added, "[{label}] 코드포인트에 대해 시스템 폰트를 새로 못 찾음");
            step_frame(&ctx);

            let covered_after = ctx.fonts_mut(|f| f.has_glyph(&font_id, c));
            assert!(covered_after, "[{label}] 폴백 등록 후에도 글리프가 없음");
        }

        // 이모지는 egui 기본 번들 폰트(NotoEmoji-Regular 등)가 이미 커버해서
        // ensure_covers가 새로 등록할 필요조차 없다 — "동적 폴백을 타야 함"이
        // 아니라 "최종적으로 화면에 나와야 함"만 확인한다.
        let emoji_covered = ctx.fonts_mut(|f| f.has_glyph(&font_id, '😀'));
        assert!(emoji_covered, "이모지가 표시되지 않음(기본 번들 폰트로도, 동적 폴백으로도 안 됨)");
    }

    /// macOS 이식 회귀 테스트: 탐색기 라벨(Proportional)이 한글을 커버하는지.
    /// "Malgun Gothic"/"Segoe UI"는 Windows 전용이라 macOS에서 후보가 하나도
    /// 안 걸리면 egui 기본 번들 폰트(한글 미지원)로 남아 한글 폴더/파일명이
    /// 두부(□)로 보이는 회귀가 생긴다.
    ///
    /// `has_glyph()`가 아니라 `glyph_width()`로 확인한다 — egui의 `has_glyph`는
    /// "이 문자가 replacement-face로 resolve됐는가"로 커버 여부를 판단하는데,
    /// 여기서 쓰는 "Apple SD Gothic Neo"처럼 macOS 시스템 폰트 자체가 U+FFFD
    /// 글리프를 갖고 있고 그게 fallback 체인 맨 앞(주 폰트)이면, egui가 그 폰트를
    /// replacement-face로 채택해버려서 **그 폰트로 resolve되는 모든 글자**가
    /// (실제로는 정상 글리프인데도) has_glyph에서 "커버 안 됨"으로 오판된다 —
    /// 직접 확인: `glyph_width`/`layout_no_wrap`으로는 '가'가 진짜 래스터라이즈된
    /// 글리프(폭 13.8pt, 진짜 사용하지 않는 코드포인트의 폭 0과 다름)로 나온다.
    /// 즉 실제 렌더링은 정상이고, `has_glyph`만 이 폰트에 한해 신뢰할 수 없다.
    #[test]
    fn proportional_font_covers_hangul() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);
        step_frame(&ctx); // set_fonts()는 다음 pass부터 적용됨 — 위 step_frame 주석 참고
        let font_id = egui::FontId::proportional(16.0);

        let width = ctx.fonts_mut(|f| f.glyph_width(&font_id, '가'));
        assert!(width > 0.0, "탐색기 라벨 폰트가 한글을 커버하지 못함 — 파일명이 두부로 보임");
    }

    /// 실사용 버그: 탭 닫기/상태 메시지 닫기 버튼에 "✕"(U+2715, Dingbats)를
    /// 썼는데 "Apple SD Gothic Neo"를 포함한 대부분의 일반 폰트가 이 코드포인트를
    /// 안 갖고 있어(fontdb로 실측 확인) 두부로 보였다. "×"(U+00D7, 곱셈 기호 —
    /// 기본 라틴 확장 범위라 사실상 모든 폰트가 가짐)로 바꿨다 — 회귀 테스트.
    #[test]
    fn proportional_font_covers_close_button_glyph() {
        let ctx = ctx_with_frame();
        let mut fallback = FontFallback::new();
        fallback.install_primary(&ctx);
        step_frame(&ctx);
        let font_id = egui::FontId::proportional(16.0);

        let width = ctx.fonts_mut(|f| f.glyph_width(&font_id, '×'));
        assert!(width > 0.0, "탭/상태 메시지 닫기 버튼에 쓰는 '×' 글리프가 커버되지 않음 — 두부로 보임");
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
