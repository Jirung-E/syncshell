use crate::ansi_color::{resolve_bg, resolve_fg_bold_aware};
use crate::theme::ThemeMode;
use crate::font_fallback::FontFallback;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::term::cell::Flags;
use eframe::egui::{self, FontId, Pos2, Rect, Vec2};
use syncshell_core::terminal::TerminalSession;

pub struct TerminalWidget {
    font_id: FontId,
    /// BOLD 셀을 그릴 실제 굵은 weight 폰트(있으면) — `install_fonts`가 채운다.
    /// 시스템에 진짜 굵은 face가 없으면 `None`으로 남고, 그때는 렌더링 루프가
    /// `font_id`(일반)로 그리고 색만 밝게 표현하는 걸로 대체한다
    /// (`ansi_color::resolve_fg_bold_aware`).
    /// **알려진 한계**: 코드포인트 폴백(`ensure_covers`)은 `font_id`(일반)
    /// family에만 동적으로 폰트를 추가한다. 이 `bold_font_id` family는 그
    /// 폴백 체인을 안 물려받으므로, 굵은 한글/이모지처럼 굵은 폰트 자체에는
    /// 없는 글자는 두부로 보일 수 있다(굵은 라틴 문자는 정상). 실사용에서
    /// 문제되면 별도로 다룰 것.
    bold_font_id: Option<FontId>,
    fallback: FontFallback,
    /// 폰트 폴백 사전 스캔을 마지막으로 돌렸을 때의 화면 상태
    /// (내용 버전, 스크롤 위치, 열, 행). 이게 그대로면 보이는 글자도 그대로라
    /// 스캔을 건너뛴다 — `show()` 안의 사용처 주석 참고.
    last_scan_key: Option<(u64, usize, u16, u16)>,
    /// 사전 스캔을 실제로 돌린 횟수(건너뛴 프레임은 세지 않음). 위 최적화가
    /// 정말 동작하는지 테스트에서 확인하는 용도.
    scans_run: u64,
    /// IME로 조합 중인 글자(DEV-012). **아직 확정되지 않았으므로 PTY로 보내지
    /// 않고 화면에만 그린다** — 확정(`ImeEvent::Commit`) 시점에만 전송한다.
    /// 비어 있으면 조합 중이 아니다.
    preedit: String,
    /// 셀 색을 정하는 테마(DEV-021). 앱이 테마를 바꾸면 `set_theme`으로 알려준다
    /// — `show()` 인자로 받지 않는 건, `show()`를 부르는 Windows 전용 테스트가
    /// 많아서(이 머신에서 컴파일 확인 불가) 시그니처를 건드리지 않기 위해서다.
    theme: ThemeMode,
}

impl TerminalWidget {
    pub fn new() -> Self {
        Self {
            font_id: FontId::monospace(16.0),
            bold_font_id: None,
            fallback: FontFallback::new(),
            last_scan_key: None,
            scans_run: 0,
            preedit: String::new(),
            theme: ThemeMode::default(),
        }
    }

    pub fn set_theme(&mut self, theme: ThemeMode) {
        self.theme = theme;
    }

    /// 사전 스캔을 실제로 돌린 누적 횟수 — 테스트에서 "안 바뀐 프레임은
    /// 건너뛰는지" 확인하는 용도.
    /// 지금 IME로 조합 중인 글자(없으면 빈 문자열).
    // 지금 이 값을 쓰는 테스트가 전부 실제 PowerShell 세션을 필요로 해서
    // windows-only로 게이팅돼 있다(cargo test --workspace 참고) — 그래서
    // 다른 플랫폼에서는 "쓰이지 않는 메서드"로 보인다.
    #[cfg(all(test, windows))]
    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    #[cfg(all(test, windows))]
    pub fn scans_run(&self) -> u64 {
        self.scans_run
    }

    /// 앱 시작 시 한 번 호출 — 주 폰트(라틴 고정폭)를 설치한다. 실제 코드포인트별
    /// 폴백(한글 등 못 커버하는 글자를 시스템에서 찾아오는 것)은 `show()`가 매
    /// 프레임 알아서 처리한다 — `font_fallback.rs` 참고.
    pub fn install_fonts(&mut self, ctx: &egui::Context) {
        self.fallback.install_primary(ctx);
        self.bold_font_id =
            self.fallback.bold_monospace_family().map(|family| FontId { size: self.font_id.size, family });
    }

    /// 셀 크기를 측정하고, 그리드가 몇 열x몇 행 들어가는지 계산해 반환한다
    /// (호출자가 session.resize에 넘기도록).
    ///
    /// 기준폭은 라틴 문자 'M' — 주 폰트로 고르는 후보(Cascadia Mono/Consolas 등,
    /// `font_fallback.rs` 참고)는 전부 실제 monospace(라틴 전부 동일 폭)라서
    /// 안전하다. 예전엔 맑은 고딕을 주 폰트로 하드코딩했었는데, 맑은 고딕은
    /// 완전한 monospace가 아니어서(라틴 글리프 폭이 문자마다 다름, 예:
    /// 16pt에서 'W'=15.26px, '.'=3.5px) 넓은 글자가 옆 칸을 침범해 화면에서
    /// 글자가 겹쳐 보이는 버그가 있었다(사용자 피드백: "터미널에서 글자가
    /// 겹쳐서 출력되는중").
    pub fn cell_size(&self, ctx: &egui::Context) -> Vec2 {
        let font_id = self.font_id.clone();
        ctx.fonts_mut(|f| {
            let w = f.glyph_width(&font_id, 'M');
            let h = f.row_height(&font_id);
            Vec2::new(w, h)
        })
    }

    /// 마우스로 텍스트를 선택한다(DEV-012). 선택 상태 자체는 alacritty_terminal의
    /// `Selection`이 들고 있고(스크롤·리사이즈 시 좌표 보정까지 해준다), 여기서는
    /// 화면 좌표를 셀 좌표로 바꿔 넘기기만 한다.
    fn handle_selection_mouse(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        session: &mut TerminalSession,
        rect: Rect,
        cell: Vec2,
    ) {
        use alacritty_terminal::selection::{Selection, SelectionType};

        let Some(pos) = response.interact_pointer_pos() else {
            return;
        };
        let (point, side) = pos_to_cell(pos, rect, cell, session);

        // 더블클릭=단어, 트리플클릭=줄. egui는 트리플클릭도 double_clicked()를
        // true로 보고하므로 트리플을 먼저 본다 — 순서를 바꾸면 줄 선택이 안 된다.
        if response.triple_clicked() {
            session.term.selection = Some(Selection::new(SelectionType::Lines, point, side));
        } else if response.double_clicked() {
            session.term.selection = Some(Selection::new(SelectionType::Semantic, point, side));
        } else if response.drag_started() {
            // 드래그의 시작점은 **버튼을 누른 위치**여야 한다. `drag_started()`가
            // true가 되는 시점엔 egui의 드래그 판정 문턱만큼 포인터가 이미 움직인
            // 뒤라, 지금 위치를 앵커로 쓰면 처음 몇 글자가 선택에서 빠진다
            // (실측: "SELECTME"를 끌었는데 "ECTME"만 선택됨).
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .unwrap_or(pos);
            let (start, start_side) = pos_to_cell(origin, rect, cell, session);
            let mut selection = Selection::new(SelectionType::Simple, start, start_side);
            // 앵커를 누른 자리에 두고, 현재 위치까지 즉시 확장한다.
            selection.update(point, side);
            session.term.selection = Some(selection);
        } else if response.dragged() {
            if let Some(selection) = session.term.selection.as_mut() {
                selection.update(point, side);
            }
        } else if response.clicked() {
            // 그냥 클릭하면 선택 해제(일반 터미널·에디터와 동일).
            session.term.selection = None;
        }

        // 선택 중 위/아래로 끌면 스크롤백이 따라 움직여야 화면 밖 내용도 선택할 수 있다.
        if response.dragged() {
            let above = rect.top() - pos.y;
            let below = pos.y - rect.bottom();
            let lines = if above > 0.0 {
                (above / cell.y).ceil() as i32
            } else if below > 0.0 {
                -((below / cell.y).ceil() as i32)
            } else {
                0
            };
            if lines != 0 {
                session
                    .term
                    .scroll_display(alacritty_terminal::grid::Scroll::Delta(lines));
                ui.ctx().request_repaint();
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, session: &mut TerminalSession) {
        let cell = self.cell_size(ui.ctx());
        // 드래그로 텍스트를 선택해야 하므로 클릭만이 아니라 드래그도 받는다(DEV-012).
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if !ui.is_rect_visible(rect) {
            return;
        }

        claim_terminal_focus(ui, &response);

        // 그리드 크기를 먼저 뷰포트에 맞춘다 — 아래 마우스 좌표 → 셀 좌표 변환이
        // 현재 프레임의 그리드 크기를 기준으로 계산되어야 한다.
        let cols = (rect.width() / cell.x).floor().max(1.0) as u16;
        let rows = (rect.height() / cell.y).floor().max(1.0) as u16;
        session.resize(cols, rows);

        // 키/텍스트 입력 — 예광탄 범위: 일반 문자, Enter, Backspace, 방향키, Ctrl+C.
        // PageUp/PageDown/휠 스크롤은 셸로 보내는 게 아니라 로컬 스크롤백 뷰포트만
        // 옮긴다(alacritty_terminal이 이미 들고 있는 스크롤백을 그대로 활용 —
        // architecture 규칙에 없던 새 기능, TR-002 "생략" 목록에 있던 항목을
        // 실사용 피드백으로 지금 채워 넣음).
        let mut input_bytes: Vec<u8> = Vec::new();
        let mut scroll_lines: i32 = 0;
        let mut page_scroll: Option<alacritty_terminal::grid::Scroll> = None;
        let mut copy_requested = false;
        let mut interrupt_requested = false;
        let mut new_preedit: Option<String> = None;
        // Ctrl+C를 복사로 볼지 인터럽트(SIGINT)로 볼지 가르는 기준 — 아래 참고.
        let has_selection = session
            .term
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty());
        ui.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Text(text) => input_bytes.extend_from_slice(text.as_bytes()),
                    // 붙여넣기(Ctrl+V, 마우스 중클릭 등)가 Event::Text가 아니라
                    // 별도의 Event::Paste로 온다는 걸 놓쳐서 여태 아예 무시되고
                    // 있었다 — 터미널에서 자주 쓰는 동작이라 우선 점검함(Tab/Home/End
                    // 처럼 하나씩 빠져있던 것과 같은 패턴). bracketed paste 모드는
                    // 여전히 범위 밖(TR-002 "생략").
                    // 여러 줄을 붙여넣으면 내부 개행이 \n으로 들어오는데, 이 코드베이스
                    // 전체에서 줄 끝은 \r 하나로 통일하기로 확정했다(초기화 스크립트에
                    // \r\n을 썼다가 PSReadLine이 ">>" 연속줄 프롬프트를 잘못 띄우던
                    // 버그가 있었음 — TR-006). 같은 문제 재발을 막기 위해 \n을 \r로 바꾼다.
                    egui::Event::Paste(text) => {
                        input_bytes.extend_from_slice(text.replace('\n', "\r").as_bytes())
                    }
                    // IME 조합 입력(DEV-012). 조합 중인 글자는 아직 확정된 입력이
                    // 아니므로 **PTY로 보내지 않고 화면에만 그린다** — 안 그러면
                    // 한글 자모가 하나씩 셸로 들어가 명령이 깨진다.
                    // 빈 문자열은 조합이 취소·종료됐다는 뜻이다.
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        new_preedit = Some(text.clone());
                    }
                    // 확정된 글자만 셸로 보낸다.
                    egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                        new_preedit = Some(String::new());
                        input_bytes.extend_from_slice(text.as_bytes());
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => match key {
                        egui::Key::Enter => input_bytes.extend_from_slice(b"\r"),
                        egui::Key::Backspace => input_bytes.push(0x7f),
                        // 탭 자동완성은 셸(PSReadLine)이 처리한다 — 여기선 그냥
                        // 탭 바이트를 그대로 전달하기만 하면 된다. TR-002 스코프
                        // 목록에 넣는 걸 빠뜨렸던 것(생략을 의도한 게 아니었음).
                        egui::Key::Tab if !modifiers.shift => input_bytes.push(b'\t'),
                        // Shift+Tab(CBT, 역방향 자동완성 순환)도 마찬가지로 빠뜨렸던
                        // 것 — 일반 Tab만 처리하고 조건에서 걸러내기만 하고 시퀀스를
                        // 안 넣어놨었다(TR-006 피드백: "shift+tab이 안 먹힘").
                        egui::Key::Tab if modifiers.shift => input_bytes.extend_from_slice(b"\x1b[Z"),
                        egui::Key::Escape => input_bytes.push(0x1b),
                        egui::Key::ArrowUp => input_bytes.extend_from_slice(b"\x1b[A"),
                        egui::Key::ArrowDown => input_bytes.extend_from_slice(b"\x1b[B"),
                        egui::Key::ArrowRight => input_bytes.extend_from_slice(b"\x1b[C"),
                        egui::Key::ArrowLeft => input_bytes.extend_from_slice(b"\x1b[D"),
                        egui::Key::Home => input_bytes.extend_from_slice(b"\x1b[H"),
                        egui::Key::End => input_bytes.extend_from_slice(b"\x1b[F"),
                        egui::Key::Delete => input_bytes.extend_from_slice(b"\x1b[3~"),
                        // Ctrl+Shift+C는 선택 여부와 무관하게 항상 복사(리눅스 터미널 관례).
                        egui::Key::C if modifiers.ctrl && modifiers.shift => copy_requested = true,
                        // Ctrl+C는 선택이 있으면 복사, 없으면 인터럽트(0x03) — Windows
                        // Terminal과 같은 규칙이다. 터미널에서 Ctrl+C는 원래 실행 중인
                        // 명령을 끊는 키라 무조건 복사로 바꿔버리면 그 기능을 잃는데,
                        // 선택이 있을 때만 복사로 해석하면 둘 다 자연스럽게 쓸 수 있다.
                        egui::Key::C if modifiers.ctrl && has_selection => copy_requested = true,
                        // 인터럽트는 0x03을 쓰는 것만으로는 부족하다 — 실행 중인
                        // 명령을 끊으려면 콘솔 CTRL_C_EVENT가 함께 필요하다(BUG-001).
                        // `TerminalSession::send_interrupt()`가 둘 다 처리한다.
                        egui::Key::C if modifiers.ctrl => interrupt_requested = true,
                        egui::Key::PageUp => page_scroll = Some(alacritty_terminal::grid::Scroll::PageUp),
                        egui::Key::PageDown => page_scroll = Some(alacritty_terminal::grid::Scroll::PageDown),
                        _ => {}
                    },
                    _ => {}
                }
            }
        });
        if let Some(preedit) = new_preedit {
            self.preedit = preedit;
        }

        if !input_bytes.is_empty() {
            let _ = session.write_keyboard_input(&input_bytes);
            // 타이핑하면 선택을 푼다 — 일반 터미널과 같은 동작. 선택해둔 채로
            // 명령을 계속 치면 화면이 반전된 채 남아 헷갈린다.
            session.term.selection = None;
        }

        if interrupt_requested {
            // 콘솔 이벤트 전송이 실패해도 0x03은 나가므로 프롬프트 줄 취소는 동작한다.
            let _ = session.send_interrupt();
        }

        if copy_requested {
            if let Some(text) = session.term.selection_to_string() {
                if !text.is_empty() {
                    ui.ctx().copy_text(text);
                }
            }
        }

        self.handle_selection_mouse(ui, &response, session, rect, cell);

        // 휠 스크롤 — 터미널 위에 마우스가 있을 때만(파일 패널 스크롤과 겹치지 않게).
        // smooth_scroll_delta.y가 양수(휠을 위로 밀어 화면이 아래로 내려가는 제스처)면
        // 더 옛날 내용을 보여줘야 하므로 스크롤백 쪽(양의 Scroll::Delta)으로 옮긴다.
        if response.hovered() {
            let wheel_y = ui.input(|i| i.smooth_scroll_delta.y);
            if wheel_y != 0.0 {
                scroll_lines += (wheel_y / cell.y).round() as i32;
            }
        }
        if let Some(scroll) = page_scroll {
            session.term.scroll_display(scroll);
        } else if scroll_lines != 0 {
            session
                .term
                .scroll_display(alacritty_terminal::grid::Scroll::Delta(scroll_lines));
        }

        // 코드포인트 폴백 사전 스캔(DEV-011) — 이번 프레임에 그릴 글자 중 지금
        // 로드된 폰트로 못 그리는 게 있으면 시스템에서 찾아 등록한다.
        // `egui::Context::set_fonts()`는 "다음 pass 시작 시점"에야 적용되므로
        // (egui 0.35 문서 그대로) 새로 등록한 폰트로 지금 이 프레임에 바로
        // 그릴 수는 없다 — request_repaint()로 한 프레임 더 돌려서 다음
        // 프레임부터 정상 렌더링되게 한다(그 사이엔 두부로 한 프레임 보일 수
        // 있지만 크래시나 레이아웃 깨짐은 없다).
        // renderable_content()는 grid를 복제하지 않는 가벼운 뷰라 두 번
        // 만들어도(스캔용 1회 + 아래 draw용 1회) 비용이 크지 않다.
        // 화면에 보이는 내용이 지난 프레임과 똑같으면 스캔 자체를 건너뛴다.
        // 이 스캔은 화면 전체 셀을 훑고 String을 새로 만드는 작업이라 8000셀
        // 기준 프레임당 0.5~1ms가 든다(실측) — 실사용에서는 대부분의 프레임이
        // "아무것도 안 바뀐" 프레임이라 그대로 두면 순수 낭비다.
        // 보이는 내용이 바뀌는 경우는 셋뿐이다: 셸이 새 출력을 냈거나
        // (content_version 증가), 스크롤 위치가 옮겨졌거나, 창 크기가 바뀌어
        // 보이는 셀 범위 자체가 달라졌거나.
        let scan_key = (
            session.content_version(),
            session.term.grid().display_offset(),
            cols,
            rows,
        );
        if self.last_scan_key != Some(scan_key) {
            self.last_scan_key = Some(scan_key);
            self.scans_run += 1;
            let scan_content = session.term.renderable_content();
            let mut visible_text = String::new();
            for indexed in scan_content.display_iter {
                let c = indexed.cell.c;
                if c != ' ' && c != '\0' {
                    visible_text.push(c);
                }
            }
            if self.fallback.ensure_covers(ui.ctx(), &self.font_id, &visible_text) {
                ui.ctx().request_repaint();
            }
        }

        let pal = &self.theme.palette().term;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, pal.bg);

        let content = session.term.renderable_content();
        // alacritty_terminal의 display_iter/cursor는 grid 좌표(스크롤 안 된 활성
        // 영역 기준 Line(0))를 그대로 준다 — display_offset만큼 떨어진 뷰포트
        // 좌표가 아니다(DEV-003 스크롤백 구현 중 발견: PageUp을 눌러 display_offset이
        // 바뀌면 display_iter가 내놓는 point.line이 전부 음수가 돼 기존의
        // `point.line.0 < 0` 가드에 전부 걸러져 화면이 통째로 빈 채로 그려졌었다).
        // alacritty_terminal이 제공하는 `term::point_to_viewport()`와 같은 공식
        // (line + display_offset)으로 뷰포트 상대 좌표로 변환해야 한다.
        let display_offset = content.display_offset as i32;
        let selection = content.selection;
        for indexed in content.display_iter {
            let point = indexed.point;
            let viewport_line = point.line.0 + display_offset;
            if viewport_line < 0 {
                continue;
            }
            let alacritty_terminal::term::cell::Cell { c, fg, bg, flags, .. } = *indexed.cell;
            if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }

            let x = rect.left() + point.column.0 as f32 * cell.x;
            let y = rect.top() + viewport_line as f32 * cell.y;
            let width = if flags.contains(Flags::WIDE_CHAR) { cell.x * 2.0 } else { cell.x };

            // 선택된 셀은 전경/배경을 뒤집어 그린다(DEV-012). 선택색을 따로 두면
            // 그 위의 글자색에 따라 안 보이는 조합이 생기는데, 반전은 원래 대비를
            // 그대로 유지해서 어떤 배색에서도 읽힌다.
            let selected = selection.is_some_and(|s| s.contains(point));
            // BOLD는 일반(0-7) ANSI 색을 밝은(8-15) 계열로 승격해서도 표현한다
            // (ansi_color.rs 참고, 대부분의 터미널 관례) — 배경색 자체(`bg`)에는
            // 적용하지 않는다. 색만으로는 부족하다는 실사용 피드백("굵은 글씨가
            // 제대로 표시되지 않음")을 받고, 시스템에서 실제 굵은 weight 폰트를
            // 찾았으면(`bold_font_id`) 그 폰트로 그려서 진짜 굵은 획으로도
            // 보이게 한다 — 없으면(시스템에 굵은 face 자체가 없는 폰트) 기존
            // 대로 색만 밝게.
            let bold = flags.contains(Flags::BOLD);
            let bright_fg = resolve_fg_bold_aware(fg, bold, pal);
            let (bg_color, fg_color) =
                if selected { (bright_fg, resolve_bg(bg, pal)) } else { (resolve_bg(bg, pal), bright_fg) };
            let glyph_font_id =
                if bold { self.bold_font_id.clone().unwrap_or_else(|| self.font_id.clone()) } else { self.font_id.clone() };

            if bg_color != pal.bg {
                painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, cell.y)), 0.0, bg_color);
            }

            if c != ' ' && c != '\0' {
                painter.text(Pos2::new(x, y), egui::Align2::LEFT_TOP, c, glyph_font_id, fg_color);
            }
        }

        // 커서 — 블링크 없이 고정 아웃라인만 (예광탄 범위). cursor.point는 항상
        // 활성 영역 좌표(0..rows)라서, 스크롤백을 보고 있을 때(display_offset != 0)
        // 뷰포트로 환산하면 화면 밖으로 밀려난다 — 그럴 땐 커서를 그리지 않는다
        // (스크롤백 열람 중에는 원래 실제 커서 위치가 안 보이는 게 자연스럽다).
        let cursor = content.cursor;
        let cursor_viewport_line = cursor.point.line.0 + display_offset;
        if cursor_viewport_line >= 0 && cursor_viewport_line < rows as i32 {
            let x = rect.left() + cursor.point.column.0 as f32 * cell.x;
            let y = rect.top() + cursor_viewport_line as f32 * cell.y;
            let cursor_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell.x, cell.y));
            painter.rect_stroke(
                cursor_rect,
                0.0,
                egui::Stroke::new(1.5, pal.cursor),
                egui::StrokeKind::Outside,
            );

            // IME 조합 중인 글자를 커서 자리에 겹쳐 그린다(DEV-012). 셸은 아직
            // 이 글자를 모르므로(확정 전엔 안 보냄) 우리가 직접 그려줘야 사용자가
            // 뭘 치고 있는지 볼 수 있다.
            if !self.preedit.is_empty() {
                self.draw_preedit(&painter, Pos2::new(x, y), cell, rect);
            }

            // IME 후보창이 커서를 따라오게 좌표를 알려준다. 이걸 설정해야
            // egui-winit이 창의 IME를 켜기도 한다(set_ime_allowed) — 없으면
            // 조합 이벤트 자체가 안 온다.
            ui.output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    rect,
                    cursor_rect,
                    should_interrupt_composition: false,
                });
            });
        }
    }

    /// 조합 중인 글자를 커서 위치부터 그린다. 셀 그리드에 맞춰야 하므로 글자마다
    /// 폭(한글·한자·가나는 2셀)을 계산해 진행한다 — 비례 배치로 그리면 확정된
    /// 뒤 셸이 그리는 위치와 어긋난다.
    ///
    /// 조합 중임을 알리기 위해 밑줄을 긋는다(터미널·에디터 공통 관례).
    fn draw_preedit(&self, painter: &egui::Painter, start: Pos2, cell: Vec2, clip: Rect) {
        use unicode_width::UnicodeWidthChar;

        let pal = self.theme.palette();
        let (bg, fg) = (pal.selection, pal.selection_text);
        let mut x = start.x;
        for c in self.preedit.chars() {
            let cols = c.width().unwrap_or(1).max(1) as f32;
            let w = cell.x * cols;
            if x + w > clip.right() {
                break; // 오른쪽 끝을 넘으면 자른다(줄바꿈 처리는 범위 밖)
            }
            let r = Rect::from_min_size(Pos2::new(x, start.y), Vec2::new(w, cell.y));
            painter.rect_filled(r, 0.0, bg);
            painter.text(
                Pos2::new(x, start.y),
                egui::Align2::LEFT_TOP,
                c,
                self.font_id.clone(),
                fg,
            );
            painter.line_segment(
                [
                    Pos2::new(x, start.y + cell.y - 1.0),
                    Pos2::new(x + w, start.y + cell.y - 1.0),
                ],
                egui::Stroke::new(1.5, fg),
            );
            x += w;
        }
    }
}

impl Default for TerminalWidget {
    fn default() -> Self {
        Self::new()
    }
}

/// 터미널이 항상 키보드 포커스를 갖게 하고, Tab/화살표/Esc를 egui의 전역 포커스
/// 이동용으로 뺏기지 않게 한다. 포커스를 가진 위젯이 하나도 없으면 egui는
/// Tab/화살표를 "다음 위젯으로 포커스 이동" 명령으로 소비해버린다(egui 소스
/// memory/mod.rs 확인) — 그 결과 Tab을 누르면 포커스가 왼쪽 탐색기의
/// selectable_label로 옮겨가고, 뒤이은 Enter가 "포커스된 위젯 활성화"로
/// 해석돼 그 폴더가 클릭된 것처럼 cd가 주입되는 버그가 있었다(TR-006 피드백:
/// "탭 치면서 탐색기쪽 폴더가 선택되고 엔터 치면 cd가 입력되어버림" — 이전에
/// PTY 타이밍 문제로 오판했던 것과는 다른 원인). 아래 테스트가 이 함수를
/// 그대로 재사용해 회귀를 잡는다.
/// 화면 좌표를 터미널 셀 좌표로 바꾼다. 돌려주는 `Point.line`은 **그리드 좌표**라
/// 스크롤백을 보고 있으면 음수가 된다 — `display_offset`을 빼는 게 그 변환이다
/// (렌더 루프가 반대로 더하는 것과 짝을 이룬다, DEV-003 참고).
///
/// `Side`는 셀의 왼쪽 절반인지 오른쪽 절반인지로, 선택 경계가 글자 앞에서 끊기는지
/// 뒤에서 끊기는지를 정한다 — 이게 없으면 드래그 시작점의 글자가 통째로 빠지거나
/// 하나 더 붙는다.
fn pos_to_cell(pos: Pos2, rect: Rect, cell: Vec2, session: &TerminalSession) -> (Point, Side) {
    use alacritty_terminal::grid::Dimensions;

    let cols = session.term.columns();
    let rows = session.term.screen_lines();

    let rel_x = ((pos.x - rect.left()) / cell.x).max(0.0);
    let col = (rel_x.floor() as usize).min(cols.saturating_sub(1));
    let side = if rel_x.fract() < 0.5 { Side::Left } else { Side::Right };

    let rel_y = ((pos.y - rect.top()) / cell.y).max(0.0);
    let viewport_row = (rel_y.floor() as usize).min(rows.saturating_sub(1));

    let display_offset = session.term.grid().display_offset() as i32;
    let line = viewport_row as i32 - display_offset;

    (Point::new(Line(line), Column(col)), side)
}

fn claim_terminal_focus(ui: &mut egui::Ui, response: &egui::Response) {
    response.request_focus();
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn screen_input(event: egui::Event) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)));
        input.events.push(event);
        input
    }

    fn layout_frame(ctx: &egui::Context, input: egui::RawInput, sidepanel_clicked: &mut bool) {
        let _ = ctx.run_ui(input, |ui| {
            egui::Panel::left("side_test").show(ui, |ui| {
                let resp = ui.selectable_label(false, "TestFolder");
                if resp.clicked() {
                    *sidepanel_clicked = true;
                }
            });
            egui::CentralPanel::default().show(ui, |ui| {
                let (_, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
                claim_terminal_focus(ui, &response);
            });
        });
    }

    /// TR-006 회귀 테스트: 터미널에서 Tab을 누른 뒤 Enter를 눌러도, 탐색기 쪽
    /// selectable_label이 "클릭됐다"고 잘못 보고되면 안 된다. `claim_terminal_focus`가
    /// 없으면 이 테스트는 실패한다(포커스가 탐색기 항목으로 옮겨가 Enter가
    /// 그 항목을 활성화하기 때문) — 직접 재현해서 확인 후 이 테스트를 작성함.
    #[test]
    fn tab_then_enter_does_not_activate_sidepanel_item() {
        let ctx = egui::Context::default();
        let mut sidepanel_clicked = false;

        // 프레임 1: 터미널에 포커스가 아직 없는 첫 프레임 상태를 흉내내고, Tab을 누른다.
        layout_frame(&ctx, screen_input(key_event(egui::Key::Tab)), &mut sidepanel_clicked);
        assert!(!sidepanel_clicked, "1프레임차에 이미 클릭된 것으로 나오면 테스트 설계 자체가 잘못됨");

        // 프레임 2: Enter — claim_terminal_focus가 제대로 동작하면 탐색기 쪽은
        // 포커스가 없으므로 Enter가 그 항목을 활성화하지 않는다.
        layout_frame(&ctx, screen_input(key_event(egui::Key::Enter)), &mut sidepanel_clicked);
        assert!(!sidepanel_clicked, "Enter가 탐색기 항목을 잘못 활성화함 — 포커스 고정이 깨짐");
    }

    /// TR-006 피드백: "화면 밖으로 나가는 출력이 아랫줄로 안 가고 짤림"을 진짜
    /// 프로덕션 경로(TerminalWidget::show가 매 프레임 session.resize()를 부르는 것
    /// 포함)로 재현한다. 별도 헤드리스 예제(wrap_check)에서는 alacritty_terminal
    /// 모델 자체는 wrap이 정상 동작함을 이미 확인했다 — 이 테스트는 "매 프레임
    /// 실제 위젯 코드가 resize를 반복 호출하는 것"이 문제를 만드는지를 본다.
    #[cfg(windows)]
    #[test]
    fn realistic_frame_loop_keeps_stable_cols_and_wraps_wide_output() {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point};

        let ctx = egui::Context::default();
        let mut widget = TerminalWidget::new();
        widget.install_fonts(&ctx);
        let mut session = TerminalSession::spawn("powershell.exe", 80, 24, || {}).expect("spawn powershell");

        let mut cols_history: Vec<usize> = Vec::new();

        let mut run_frames = |session: &mut TerminalSession, n: usize| {
            for _ in 0..n {
                session.pump();
                let mut input = egui::RawInput::default();
                input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 500.0)));
                let _ = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                        widget.show(ui, session);
                    });
                });
                cols_history.push(session.term.columns());
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };

        run_frames(&mut session, 50); // 스폰 정착
        // 강제로 컬럼 수를 지정하지 않는다 — PowerShell이 실제 콘솔 폭에 맞춰
        // 알아서 몇 칸으로 나눌지 정하게 둔다(-Column N을 강제하면 PowerShell
        // 자체가 폭에 안 맞는 칸 수를 억지로 맞추려다 파일명을 "..."로 잘라버리는데,
        // 이건 syncshell의 wrap 문제가 아니라 PowerShell 포맷터의 정상 동작임).
        session.write_input(b"Get-ChildItem C:\\Windows\r").expect("write");
        run_frames(&mut session, 80); // 출력 스트리밍 동안

        let distinct: std::collections::BTreeSet<usize> = cols_history.iter().copied().collect();
        println!("관찰된 cols 값들: {distinct:?} (마지막 cols={})", cols_history.last().unwrap());
        assert!(
            distinct.len() <= 2,
            "cols가 프레임마다 불안정하게 흔들림(리사이즈 스래싱 의심): {distinct:?}"
        );

        let cols = *cols_history.last().unwrap();
        let rows = session.term.screen_lines();
        println!("\n=== 최종 화면 (cols={cols}, rows={rows}) ===");
        let grid = session.term.grid();
        let mut truncated_names = 0;
        for l in 0..rows as i32 {
            let mut text = String::new();
            let mut wrapped_last = false;
            for c in 0..cols {
                let cell = &grid[Point::new(Line(l), Column(c))];
                text.push(cell.c);
                if c == cols - 1 {
                    wrapped_last = cell.flags.contains(Flags::WRAPLINE);
                }
            }
            let trimmed = text.trim_end();
            if !trimmed.is_empty() {
                println!("[{l:>2}] wrapped={wrapped_last:5} {trimmed:?}");
            }
            if trimmed.contains("...") {
                truncated_names += 1;
            }
        }
        println!("\n'...' 로 잘린 항목 수: {truncated_names} (0이어야 정상 — 강제 컬럼수 없이 PowerShell 기본 포맷팅)");
        assert_eq!(truncated_names, 0);

        // 새 기본 창 크기(1400x800)에서 파일 패널을 뺀 폭이면, Get-ChildItem
        // 기본 뷰의 Name 컬럼이 드롭되지 않고 실제로 보여야 한다 — 좁은 폭(47컬럼)
        // 에서는 이 컬럼이 통째로 사라지는 것을 확인했었다(PowerShell 자체의
        // 폭 적응 동작). 파일명 확장자 패턴으로 Name 컬럼 존재를 확인.
        let full_screen: String = (0..rows as i32)
            .flat_map(|l| (0..cols).map(move |c| Point::new(Line(l), Column(c))))
            .map(|p| grid[p].c)
            .collect();
        assert!(
            full_screen.contains(".exe") || full_screen.contains(".dll") || full_screen.contains(".log"),
            "이 폭에서는 Name 컬럼이 보여야 하는데 파일 확장자를 못 찾음 — 창이 다시 좁아졌는지 확인"
        );
    }

    /// TR-006 회귀 테스트: 붙여넣기(Event::Paste)가 Event::Text/Event::Key 처리에만
    /// 신경 쓰다 아예 빠져서 무시되고 있었다 — 실제 프로덕션 위젯 코드로 재현.
    /// 여러 줄(내부 개행 포함)을 붙여넣었을 때 \n이 아니라 \r로 정규화되어 각 줄이
    /// 제대로 명령으로 실행되는지까지 실제 PowerShell로 확인한다.
    #[cfg(windows)]
    #[test]
    fn pasted_multiline_text_executes_each_line() {
        let ctx = egui::Context::default();
        let mut widget = TerminalWidget::new();
        widget.install_fonts(&ctx);
        let mut session = TerminalSession::spawn("powershell.exe", 100, 24, || {}).expect("spawn powershell");

        let mut run_frames = |session: &mut TerminalSession, n: usize, paste: Option<&str>| {
            for i in 0..n {
                session.pump();
                let mut input = egui::RawInput::default();
                input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 500.0)));
                if i == 0 {
                    if let Some(text) = paste {
                        input.events.push(egui::Event::Paste(text.to_string()));
                    }
                }
                let _ = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                        widget.show(ui, session);
                    });
                });
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };

        run_frames(&mut session, 50, None); // 스폰 정착
        println!("정착 후 cwd: {:?}", session.cwd);

        // 두 줄을 한 번에 붙여넣는다 — 내부 개행이 \n으로 옴을 시뮬레이션.
        // 첫 줄에서 C:\Windows로 이동하고, 둘째 줄에서 그 안의 System32로 이동.
        let pasted = "cd C:\\Windows\ncd System32\n";
        run_frames(&mut session, 1, Some(pasted));
        println!("붙여넣기 직후 cwd: {:?}", session.cwd);
        run_frames(&mut session, 60, None); // 두 cd가 순서대로 처리되는 동안

        println!("붙여넣기 처리 후 cwd: {:?}", session.cwd);
        assert_eq!(
            session.cwd.as_deref(),
            Some(std::path::Path::new(r"C:\Windows\System32")),
            "붙여넣은 두 줄이 순서대로 실행되지 않음 — Event::Paste 처리 또는 \\n→\\r 정규화가 깨짐"
        );
    }

    #[cfg(windows)]
    fn full_screen_text(session: &TerminalSession) -> String {
        use alacritty_terminal::grid::Dimensions;
        let content = session.term.renderable_content();
        let rows = session.term.screen_lines();
        let display_offset = content.display_offset as i32;
        let mut lines: Vec<String> = vec![String::new(); rows];
        for indexed in content.display_iter {
            // show()와 동일하게 grid 좌표를 뷰포트 좌표로 변환해야 한다 — 그대로
            // 쓰면 display_offset != 0일 때 전부 음수가 돼 아무 것도 안 잡힌다.
            let l = indexed.point.line.0 + display_offset;
            if l >= 0 && (l as usize) < rows {
                lines[l as usize].push(indexed.cell.c);
            }
        }
        lines.join("\n")
    }

    /// DEV-003 회귀 테스트: 뷰포트보다 훨씬 많은 줄을 출력한 뒤, 실제 프로덕션
    /// 위젯 코드(PageUp 처리 포함)로 스크롤백을 위로 넘겨서 옛날 내용이 실제로
    /// 보이는지, 방향이 올바른지(PageUp이 더 오래된/작은 번호 쪽을 보여주는지)
    /// 확인한다.
    #[cfg(windows)]
    #[test]
    fn pageup_reveals_older_scrollback_content() {
        let ctx = egui::Context::default();
        let mut widget = TerminalWidget::new();
        widget.install_fonts(&ctx);
        // 행 수를 작게(뷰포트 250px 높이) 만들어 50줄 출력이 화면보다 훨씬 많아지게 한다.
        let mut session = TerminalSession::spawn("powershell.exe", 120, 24, || {}).expect("spawn powershell");

        let mut run_frames = |session: &mut TerminalSession, n: usize, height: f32| {
            for _ in 0..n {
                session.pump();
                let mut input = egui::RawInput::default();
                input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, height)));
                let _ = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                        widget.show(ui, session);
                    });
                });
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };

        run_frames(&mut session, 50, 500.0); // 스폰 정착 (넉넉한 높이로)

        // 화면보다 훨씬 많은 줄을 생성 — 뷰포트를 일부러 작게(150px, ~6줄)
        // 잡아서 50줄이 스크롤백으로 확실히 밀려나게 한다.
        session
            .write_input(b"1..50 | ForEach-Object { Write-Host $_ }\r")
            .expect("write");
        run_frames(&mut session, 60, 150.0);

        let bottom_view = full_screen_text(&session);
        println!("=== 스크롤 전(최신) ===\n{bottom_view}");
        assert!(bottom_view.contains("50"), "최신 뷰에 마지막 줄(50)이 안 보임 — 테스트 시나리오 자체가 잘못됨");
        assert!(
            !bottom_view.lines().any(|l| l.trim_end() == "1"),
            "최신 뷰에 이미 첫 줄(1)이 보임 — 스크롤백이 충분히 안 쌓임(뷰포트를 더 줄이거나 줄 수를 늘릴 것)"
        );

        // PageUp을 반복 — 실제 프로덕션 키 처리 경로로. 뷰포트가 ~7줄이고
        // 스크롤백이 약 49줄이라 맨 위(줄 "1")까지 가려면 산수로 7번은 필요하다
        // (처음엔 2번이면 충분하다고 잘못 가정해서 실패했었다 — PageUp이 한 번에
        // 옮기는 양은 현재 뷰포트 줄 수만큼이라 원래도 그 이상은 못 감).
        // 여유 있게 10번 눌러서 맨 위에 확실히 도달하게 한다.
        for _ in 0..10 {
            session.pump();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 150.0)));
            input.events.push(key_event(egui::Key::PageUp));
            let _ = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    widget.show(ui, &mut session);
                });
            });
        }

        let scrolled_view = full_screen_text(&session);
        println!("\n=== PageUp x10 후 ===\n{scrolled_view}");
        // full_screen_text()는 각 줄을 전체 컬럼 폭만큼(공백 포함) 그대로 담으므로
        // 줄 끝 공백을 잘라내고 비교해야 한다.
        assert!(
            scrolled_view.lines().any(|l| l.trim_end() == "1"),
            "PageUp 후에도 옛날 내용(1)이 안 보임 — 스크롤 방향/메커니즘이 깨짐"
        );
        assert_ne!(scrolled_view, bottom_view, "PageUp을 눌렀는데 화면이 전혀 안 바뀜");

        // PageDown으로 다시 내려가면 최신 내용(바닥)으로 돌아와야 한다.
        for _ in 0..10 {
            session.pump();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 150.0)));
            input.events.push(key_event(egui::Key::PageDown));
            let _ = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    widget.show(ui, &mut session);
                });
            });
        }
        let back_to_bottom = full_screen_text(&session);
        println!("\n=== PageDown으로 복귀 후 ===\n{back_to_bottom}");
        assert_eq!(
            back_to_bottom, bottom_view,
            "PageDown으로 끝까지 내려도 원래 최신 화면으로 복귀하지 않음"
        );
    }

    /// 유휴 상태(입력도 없고 셸 출력도 없음)에서 앱이 계속 다시 그리라고
    /// 요청하는지 확인한다 — 요청하면 배터리를 계속 먹는다(DEV-003 퀘스트의
    /// "유휴 시 CPU 0에 가깝게" 요구사항). egui는 `repaint_delay`로 다음
    /// 다시 그리기 시점을 알려주는데, `Duration::MAX`면 "입력이 올 때까지
    /// 다시 그릴 필요 없음"이라는 뜻이다.
    ///
    /// 폰트 폴백 사전 스캔이 매 프레임 도는 게 여기 영향을 주는지도 같이 본다 —
    /// 이미 해결된 글자만 있으면 `ensure_covers`가 false를 돌려줘서
    /// `request_repaint()`를 부르지 않아야 한다.
    #[cfg(windows)]
    #[test]
    fn idle_frames_do_not_request_continuous_repaint() {
        let ctx = egui::Context::default();
        let mut widget = TerminalWidget::new();
        widget.install_fonts(&ctx);
        let mut session = TerminalSession::spawn("powershell.exe", 120, 30, || {}).expect("spawn powershell");

        let mut run_frame = |session: &mut TerminalSession| -> std::time::Duration {
            session.pump();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 700.0)));
            let output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    widget.show(ui, session);
                });
            });
            output
                .viewport_output
                .values()
                .map(|v| v.repaint_delay)
                .min()
                .unwrap_or(std::time::Duration::MAX)
        };

        // 스폰 정착 — 이 동안엔 셸 출력이 계속 오므로 다시 그리기 요청이 정상이다.
        for _ in 0..60 {
            run_frame(&mut session);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // 한글까지 화면에 띄워서 폰트 폴백이 확실히 한 번 발동하게 한다.
        session.write_input("Write-Host '한글 출력 테스트 가나다'\r".as_bytes()).expect("write");
        for _ in 0..60 {
            run_frame(&mut session);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // 이제 완전히 조용한 상태 — 입력도 없고 새 출력도 없다.
        // 마지막 몇 프레임이 "다시 그릴 필요 없음"으로 수렴해야 한다.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let delays: Vec<std::time::Duration> = (0..5).map(|_| run_frame(&mut session)).collect();
        println!("유휴 상태 repaint_delay: {delays:?}");

        assert!(
            delays.iter().all(|d| *d == std::time::Duration::MAX),
            "유휴 상태인데 다시 그리기를 계속 요청함(배터리 소모) — {delays:?}"
        );
    }

    /// 폰트 폴백 사전 스캔은 화면 전체 셀을 훑고 String을 만드는 작업이라
    /// 8000셀 기준 프레임당 0.5~1ms가 든다(실측). 화면 내용이 안 바뀐
    /// 프레임에서는 그 비용을 통째로 건너뛰는지 확인한다 — 실사용에서는
    /// 대부분의 프레임이 "아무것도 안 바뀐" 프레임이라 이게 안 되면 순수 낭비다.
    ///
    /// 벽시계 시간이 아니라 "스캔을 실제로 몇 번 돌렸는지"로 검증한다
    /// (디버그/릴리스 빌드나 머신 부하에 흔들리지 않게).
    #[cfg(windows)]
    #[test]
    fn font_scan_is_skipped_when_screen_content_unchanged() {
        let ctx = egui::Context::default();
        let mut widget = TerminalWidget::new();
        widget.install_fonts(&ctx);
        let mut session = TerminalSession::spawn("powershell.exe", 200, 50, || {}).expect("spawn powershell");

        let run_frame = |session: &mut TerminalSession, widget: &mut TerminalWidget| {
            session.pump();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0)));
            let _ = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    widget.show(ui, session);
                });
            });
        };

        for _ in 0..40 {
            run_frame(&mut session, &mut widget);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // 화면을 글자로 가득 채운다(한글 포함 — 폴백까지 한 번 태운다).
        session
            .write_input("Get-ChildItem C:\\Windows; Write-Host '한글 가나다라마바사'\r".as_bytes())
            .expect("write");
        for _ in 0..60 {
            run_frame(&mut session, &mut widget);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // 이제 조용한 상태 — 새 출력도 없고 스크롤/리사이즈도 없다.
        std::thread::sleep(std::time::Duration::from_millis(300));
        run_frame(&mut session, &mut widget); // 마지막 잔여 출력까지 반영
        let before = widget.scans_run();
        for _ in 0..100 {
            run_frame(&mut session, &mut widget);
        }
        let scans_during_idle = widget.scans_run() - before;
        println!("조용한 100프레임 동안 실제로 돈 스캔 횟수: {scans_during_idle}");
        assert_eq!(
            scans_during_idle, 0,
            "화면이 안 바뀌었는데 스캔이 {scans_during_idle}번 돌았음 — 건너뛰기 최적화가 깨짐"
        );

        // 반대로, 새 출력이 오면 반드시 다시 스캔해야 한다(안 하면 새 글자가
        // 영영 두부로 남는다).
        let before = widget.scans_run();
        session.write_input("Write-Host '새출력 テスト'\r".as_bytes()).expect("write");
        for _ in 0..60 {
            run_frame(&mut session, &mut widget);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            widget.scans_run() > before,
            "새 출력이 왔는데도 스캔을 안 돌림 — 새 글자가 두부로 남게 됨"
        );
    }

    // ---- DEV-012: 선택·복사 ----

    /// 선택·복사 테스트용 하네스. 실제 PowerShell 세션 위에서 진짜 위젯 코드를
    /// 돌리고, 합성 마우스/키 이벤트를 넣어 결과를 읽는다.
    #[cfg(windows)]
    struct SelectionHarness {
        ctx: egui::Context,
        widget: TerminalWidget,
        session: TerminalSession,
        rect: egui::Rect,
    }

    #[cfg(windows)]
    impl SelectionHarness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let mut widget = TerminalWidget::new();
            widget.install_fonts(&ctx);
            let session =
                TerminalSession::spawn("powershell.exe", 120, 30, || {}).expect("spawn powershell");
            let mut h = Self {
                ctx,
                widget,
                session,
                rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 600.0)),
            };
            h.run_frames(50, |_| {}); // 스폰 정착
            h
        }

        /// `n` 프레임을 돌린다. `fill`은 프레임마다 RawInput을 꾸미는 훅.
        /// 복사 명령이 나왔으면 마지막으로 복사된 문자열을 돌려준다.
        fn run_frames(&mut self, n: usize, mut fill: impl FnMut(&mut egui::RawInput)) -> Option<String> {
            let mut copied = None;
            for _ in 0..n {
                self.session.pump();
                let mut input = egui::RawInput::default();
                input.screen_rect = Some(self.rect);
                fill(&mut input);
                let widget = &mut self.widget;
                let session = &mut self.session;
                let out = self.ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| widget.show(ui, session));
                });
                for cmd in &out.platform_output.commands {
                    if let egui::OutputCommand::CopyText(text) = cmd {
                        copied = Some(text.clone());
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            copied
        }

        fn cell_size(&self) -> Vec2 {
            self.widget.cell_size(&self.ctx)
        }

        /// 셀 안의 좌표. `frac_x`는 셀 안에서의 가로 위치(0.0=왼쪽 끝, 1.0=오른쪽 끝).
        /// 선택 경계는 셀의 어느 쪽 절반을 눌렀는지로 정해지므로(Side::Left/Right)
        /// 이 값이 어느 글자까지 포함되는지를 좌우한다.
        fn cell_pos(&self, col: usize, viewport_row: usize, frac_x: f32) -> egui::Pos2 {
            let cell = self.cell_size();
            egui::Pos2::new(
                self.rect.left() + (col as f32 + frac_x) * cell.x,
                self.rect.top() + (viewport_row as f32 + 0.5) * cell.y,
            )
        }

        /// 화면 각 행의 텍스트. 렌더러와 똑같이 **와이드 문자 뒤의 스페이서 셀을
        /// 건너뛴다** — 안 그러면 한글이 "한 글"처럼 사이에 공백이 낀 채로 나와서
        /// 실제 화면과 다른 것을 검사하게 된다(실측으로 겪음).
        fn screen_lines(&self) -> Vec<String> {
            use alacritty_terminal::grid::Dimensions;
            let content = self.session.term.renderable_content();
            let offset = content.display_offset as i32;
            let rows = self.session.term.screen_lines();
            let mut lines = vec![String::new(); rows];
            for indexed in content.display_iter {
                let flags = indexed.cell.flags;
                if flags.contains(Flags::WIDE_CHAR_SPACER)
                    || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let l = indexed.point.line.0 + offset;
                if l >= 0 && (l as usize) < rows {
                    lines[l as usize].push(indexed.cell.c);
                }
            }
            lines
        }

        /// 화면에서 주어진 텍스트가 있는 뷰포트 행을 찾는다.
        fn find_row(&self, needle: &str) -> Option<usize> {
            self.screen_lines().iter().position(|l| l.contains(needle))
        }

        /// 화면에 `needle`이 나타날 때까지 프레임을 돌리며 기다린다(최대 ~4초).
        /// 고정 프레임 수로 기다리면 테스트를 병렬로 돌릴 때(각자 PowerShell을
        /// 띄우므로 부하가 크다) 셸이 느려져 간헐적으로 실패한다.
        fn wait_for_row(&mut self, needle: &str) -> bool {
            for _ in 0..200 {
                if self.find_row(needle).is_some() {
                    return true;
                }
                self.run_frames(1, |_| {});
            }
            false
        }

        fn dump_screen(&self) -> String {
            self.screen_lines()
                .iter()
                .map(|l| l.trim_end())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        }

        /// `col_from`부터 `col_to`까지(양끝 포함) 드래그로 선택한다.
        /// 시작은 첫 글자의 왼쪽 부분(0.25), 끝은 마지막 글자의 오른쪽 부분(0.75)을
        /// 눌러 두 글자가 모두 선택에 들어가게 한다 — 실제 사용자가 "이 글자부터
        /// 이 글자까지" 끌 때의 손 위치와 같다.
        fn drag_select(&mut self, row: usize, col_from: usize, col_to: usize) {
            let from = self.cell_pos(col_from, row, 0.25);
            let to = self.cell_pos(col_to, row, 0.75);
            // 누르고 → 끌고 → 뗀다. egui가 드래그로 인식하려면 누른 상태로
            // 위치가 바뀐 프레임이 있어야 한다.
            self.run_frames(1, |i| {
                i.events.push(egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
            });
            for step in 1..=3 {
                let t = step as f32 / 3.0;
                let pos = egui::Pos2::new(from.x + (to.x - from.x) * t, from.y);
                self.run_frames(1, |i| i.events.push(egui::Event::PointerMoved(pos)));
            }
            self.run_frames(1, |i| {
                i.events.push(egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            });
        }

        /// IME 조합 이벤트를 한 프레임에 넣는다.
        fn send_ime(&mut self, event: egui::ImeEvent) {
            self.run_frames(1, |i| i.events.push(egui::Event::Ime(event.clone())));
        }

        /// 이번 프레임의 `platform_output.ime`(IME 후보창 위치 통보)를 돌려준다.
        fn ime_output(&mut self) -> Option<egui::output::IMEOutput> {
            self.session.pump();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(self.rect);
            let widget = &mut self.widget;
            let session = &mut self.session;
            let out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| widget.show(ui, session));
            });
            out.platform_output.ime
        }

        fn press_ctrl_c(&mut self) -> Option<String> {
            self.run_frames(1, |i| {
                i.events.push(egui::Event::Key {
                    key: egui::Key::C,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL,
                });
            })
        }
    }

    /// DEV-012 핵심: 드래그로 고른 영역이 실제로 선택되고, Ctrl+C로 그 텍스트가
    /// 클립보드로 나가는지 — 실제 PowerShell 출력 위에서 프로덕션 경로로 확인한다.
    #[cfg(windows)]
    #[test]
    fn drag_selection_then_ctrl_c_copies_selected_text() {
        let mut h = SelectionHarness::new();
        h.session
            .write_input(b"Write-Host 'SELECTME-ABCDEFGH'\r")
            .expect("write");
        assert!(
            h.wait_for_row("SELECTME-ABCDEFGH"),
            "출력한 문자열이 화면에 안 나타남"
        );

        let row = h
            .find_row("SELECTME-ABCDEFGH")
            .expect("출력한 문자열이 화면에 없음 — 테스트 시나리오가 잘못됨");
        // 'SELECTME-ABCDEFGH'가 시작하는 열을 찾는다.
        let col = {
            use alacritty_terminal::grid::Dimensions;
            let content = h.session.term.renderable_content();
            let offset = content.display_offset as i32;
            let cols = h.session.term.columns();
            let mut line = vec![' '; cols];
            for indexed in content.display_iter {
                let l = indexed.point.line.0 + offset;
                if l == row as i32 {
                    let c = indexed.point.column.0;
                    if c < cols {
                        line[c] = indexed.cell.c;
                    }
                }
            }
            let text: String = line.iter().collect();
            text.find("SELECTME").expect("행 안에서 문자열을 못 찾음")
        };

        // "SELECTME" 8글자만 선택한다.
        h.drag_select(row, col, col + 7);

        let selected = h.session.term.selection_to_string().unwrap_or_default();
        println!("선택된 텍스트: {selected:?}");
        assert!(
            selected.contains("SELECTME"),
            "드래그했는데 선택 내용이 예상과 다름: {selected:?}"
        );

        let copied = h.press_ctrl_c().expect("Ctrl+C를 눌렀는데 복사 명령이 안 나옴");
        println!("복사된 텍스트: {copied:?}");
        assert!(
            copied.contains("SELECTME"),
            "복사된 내용이 선택과 다름: {copied:?}"
        );
    }

    /// 선택이 없을 때의 Ctrl+C는 복사가 아니라 인터럽트 바이트(0x03)로 가야 한다.
    /// 셸에 실제로 전달됐는지는 **프롬프트에서 치던 줄이 취소되는지**로 확인한다
    /// (PSReadLine이 `^C`를 찍고 줄을 버린 뒤 새 프롬프트를 낸다).
    ///
    /// **실행 중인 명령을 끊는 것**은 이걸로 검증하지 않는다 —
    /// `examples/ctrlc_matrix.rs`(별도 프로세스)가 그쪽을 담당한다.
    ///
    /// # 왜 `#[ignore]`인가
    /// 이 테스트는 `send_interrupt()`를 태우는데, 그 안에서 콘솔을 잠깐 뗐다
    /// 붙인다(BUG-001). 그 조작이 **프로세스 전역**이라 같은 테스트 바이너리에서
    /// 병렬로 도는 다른 PTY 테스트들을 망가뜨린다 — 실측으로 무관한 테스트 3~4개가
    /// 매번 다르게 실패했고, 이 테스트를 빼면 안정적으로 통과했다.
    /// 단독 실행으로만 돌린다:
    ///
    /// ```text
    /// cargo test -p syncshell-ui -- --ignored --test-threads=1
    /// ```
    #[cfg(windows)]
    #[test]
    #[ignore = "콘솔 조작이 프로세스 전역이라 병렬 테스트를 깨뜨림 — 단독 실행할 것"]
    fn ctrl_c_without_selection_sends_interrupt_instead_of_copying() {
        let mut h = SelectionHarness::new();
        assert!(
            h.session.term.selection.is_none(),
            "시작부터 선택이 있으면 이 테스트의 전제가 깨짐"
        );

        // Enter 없이 명령을 치기만 한다 — 이 줄이 취소돼야 한다.
        h.run_frames(1, |i| {
            i.events
                .push(egui::Event::Text("Write-Host 'MUST-NOT-RUN'".to_string()))
        });
        assert!(
            h.wait_for_row("MUST-NOT-RUN"),
            "타이핑한 내용이 화면에 안 나옴 — 테스트 전제가 깨짐"
        );

        let copied = h.press_ctrl_c();
        assert_eq!(copied, None, "선택이 없는데 Ctrl+C가 복사로 처리됨");
        h.wait_for_row("^C");
        println!("=== Ctrl+C 후 화면 ===\n{}", h.dump_screen());

        // 줄이 취소됐으면 PSReadLine이 ^C를 찍었고, 그 명령은 실행되지 않는다.
        assert!(
            h.dump_screen().contains("^C"),
            "Ctrl+C를 눌렀는데 셸이 ^C를 표시하지 않음 — 0x03이 전달되지 않았다"
        );

        // 새 명령이 정상 실행되는지로 프롬프트가 살아있음을 확인하고,
        // 취소된 명령이 실행되지 않았음을 함께 본다.
        h.session
            .write_input(b"Write-Host 'PROMPT-ALIVE'\r")
            .expect("write");
        h.wait_for_row("PROMPT-ALIVE");
        let screen = h.dump_screen();
        assert!(
            screen.contains("PROMPT-ALIVE"),
            "Ctrl+C 후 프롬프트가 살아있지 않음"
        );
        assert!(
            !screen.lines().any(|l| l.trim() == "MUST-NOT-RUN"),
            "취소했어야 할 명령이 실행돼버림"
        );
    }

    /// 타이핑하면 선택이 풀려야 한다 — 선택해둔 채로 명령을 계속 치면 화면이
    /// 반전된 채 남아 헷갈린다(일반 터미널과 같은 동작).
    #[cfg(windows)]
    #[test]
    fn typing_clears_selection() {
        let mut h = SelectionHarness::new();
        h.session.write_input(b"Write-Host 'CLEARME'\r").expect("write");
        assert!(h.wait_for_row("CLEARME"), "출력이 화면에 안 나타남");

        let row = h.find_row("CLEARME").expect("출력이 화면에 없음");
        h.drag_select(row, 0, 10);
        assert!(
            h.session.term.selection.is_some(),
            "드래그했는데 선택이 안 만들어짐"
        );

        h.run_frames(1, |i| i.events.push(egui::Event::Text("x".to_string())));
        assert!(
            h.session.term.selection.is_none(),
            "타이핑했는데 선택이 안 풀림"
        );
    }

    // ---- DEV-012: IME 조합 입력 ----

    /// DEV-012 핵심: **조합 중인 글자는 셸로 새어나가면 안 된다.** 새어나가면
    /// 한글 자모가 하나씩 셸에 들어가 명령이 깨진다("ㅎ", "하", "한" 이 순서대로
    /// 전부 입력되는 꼴). 확정(Commit) 시점에만 보내야 한다.
    #[cfg(windows)]
    #[test]
    fn ime_preedit_is_not_sent_to_shell_until_commit() {
        let mut h = SelectionHarness::new();

        // 한글 "한글"을 치는 동안 IME가 보내는 중간 상태들.
        for step in ["ㅎ", "하", "한", "한ㄱ", "한그", "한글"] {
            h.send_ime(egui::ImeEvent::Preedit {
                text: step.to_string(),
                active_range_chars: None,
            });
            assert_eq!(
                h.widget.preedit(),
                step,
                "조합 중인 글자가 위젯 상태에 반영되지 않음"
            );
        }
        // 셸이 조금이라도 반응할 시간을 준다 — 새어나갔다면 여기서 화면에 찍힌다.
        h.run_frames(40, |_| {});

        let during = h.dump_screen();
        println!("=== 조합 중 화면 ===\n{during}");
        assert!(
            !during.contains('ㅎ') && !during.contains('하') && !during.contains('한'),
            "조합 중인 글자가 셸로 새어나가 화면에 찍힘 — PTY로 보내면 안 된다:\n{during}"
        );

        // 확정 — 이제서야 셸로 간다.
        h.send_ime(egui::ImeEvent::Commit("한글".to_string()));
        assert_eq!(h.widget.preedit(), "", "확정했는데 조합 상태가 안 지워짐");

        assert!(
            h.wait_for_row("한글"),
            "확정한 글자가 셸에 전달되지 않음:\n{}",
            h.dump_screen()
        );
    }

    /// 빈 Preedit은 "조합이 취소·종료됐다"는 뜻이다(Esc 등). 상태가 지워져야
    /// 화면에 조합 글자가 남지 않는다.
    #[cfg(windows)]
    #[test]
    fn empty_preedit_clears_composition() {
        let mut h = SelectionHarness::new();

        h.send_ime(egui::ImeEvent::Preedit {
            text: "한".to_string(),
            active_range_chars: None,
        });
        assert_eq!(h.widget.preedit(), "한");

        h.send_ime(egui::ImeEvent::Preedit {
            text: String::new(),
            active_range_chars: None,
        });
        assert_eq!(h.widget.preedit(), "", "빈 Preedit인데 조합 상태가 남음");
    }

    /// IME 후보창이 커서를 따라오려면 매 프레임 커서 좌표를 통보해야 한다.
    /// 이 값을 안 내보내면 egui-winit이 창의 IME를 켜지 않아 **조합 이벤트 자체가
    /// 오지 않는다** — 즉 이게 없으면 한글 입력이 통째로 안 된다.
    #[cfg(windows)]
    #[test]
    fn ime_cursor_area_is_reported_for_candidate_window() {
        let mut h = SelectionHarness::new();
        let ime = h.ime_output().expect("IME 좌표를 통보하지 않음 — 한글 입력이 아예 안 된다");

        println!("IME rect={:?} cursor_rect={:?}", ime.rect, ime.cursor_rect);
        assert!(
            h.rect.contains_rect(ime.cursor_rect),
            "커서 좌표가 터미널 영역 밖: cursor_rect={:?}, terminal={:?}",
            ime.cursor_rect,
            h.rect
        );
        let cell = h.cell_size();
        assert!(
            (ime.cursor_rect.width() - cell.x).abs() < 0.5
                && (ime.cursor_rect.height() - cell.y).abs() < 0.5,
            "커서 좌표가 셀 한 칸 크기가 아님: {:?} vs cell {:?}",
            ime.cursor_rect.size(),
            cell
        );
    }

    /// 화면 좌표 → 셀 좌표 변환이 스크롤백(display_offset)을 반영하는지.
    /// 렌더 루프는 반대로 display_offset을 더하므로(DEV-003), 여기서 빼지 않으면
    /// 스크롤백을 보고 있을 때 엉뚱한 줄이 선택된다.
    #[cfg(windows)]
    #[test]
    fn pos_to_cell_accounts_for_scrollback_offset() {
        let mut h = SelectionHarness::new();
        // 스크롤백을 쌓는다.
        h.session
            .write_input(b"1..80 | ForEach-Object { Write-Host \"line$_\" }\r")
            .expect("write");
        h.run_frames(80, |_| {});

        let cell = h.cell_size();
        let pos = egui::Pos2::new(h.rect.left() + cell.x * 0.5, h.rect.top() + cell.y * 2.5);

        let (point_bottom, _) = pos_to_cell(pos, h.rect, cell, &h.session);
        assert_eq!(point_bottom.line.0, 2, "스크롤 안 한 상태에서 뷰포트 행과 그리드 행이 달라짐");

        // 스크롤백으로 10줄 올라간다.
        h.session
            .term
            .scroll_display(alacritty_terminal::grid::Scroll::Delta(10));
        let (point_scrolled, _) = pos_to_cell(pos, h.rect, cell, &h.session);

        assert_eq!(
            point_scrolled.line.0, -8,
            "스크롤백을 10줄 올렸으면 같은 화면 위치가 그리드에서 10줄 위(2-10=-8)를 가리켜야 함"
        );
    }
}
