use crate::ansi_color::{resolve_bg, resolve_fg, DEFAULT_BG};
use crate::font_fallback::FontFallback;
use alacritty_terminal::term::cell::Flags;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Vec2};
use syncshell_core::terminal::TerminalSession;

pub struct TerminalWidget {
    font_id: FontId,
    fallback: FontFallback,
    /// 폰트 폴백 사전 스캔을 마지막으로 돌렸을 때의 화면 상태
    /// (내용 버전, 스크롤 위치, 열, 행). 이게 그대로면 보이는 글자도 그대로라
    /// 스캔을 건너뛴다 — `show()` 안의 사용처 주석 참고.
    last_scan_key: Option<(u64, usize, u16, u16)>,
    /// 사전 스캔을 실제로 돌린 횟수(건너뛴 프레임은 세지 않음). 위 최적화가
    /// 정말 동작하는지 테스트에서 확인하는 용도.
    scans_run: u64,
}

impl TerminalWidget {
    pub fn new() -> Self {
        Self {
            font_id: FontId::monospace(16.0),
            fallback: FontFallback::new(),
            last_scan_key: None,
            scans_run: 0,
        }
    }

    /// 사전 스캔을 실제로 돌린 누적 횟수 — 테스트에서 "안 바뀐 프레임은
    /// 건너뛰는지" 확인하는 용도.
    pub fn scans_run(&self) -> u64 {
        self.scans_run
    }

    /// 앱 시작 시 한 번 호출 — 주 폰트(라틴 고정폭)를 설치한다. 실제 코드포인트별
    /// 폴백(한글 등 못 커버하는 글자를 시스템에서 찾아오는 것)은 `show()`가 매
    /// 프레임 알아서 처리한다 — `font_fallback.rs` 참고.
    pub fn install_fonts(&mut self, ctx: &egui::Context) {
        self.fallback.install_primary(ctx);
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

    pub fn show(&mut self, ui: &mut egui::Ui, session: &mut TerminalSession) {
        let cell = self.cell_size(ui.ctx());
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }

        claim_terminal_focus(ui, &response);

        // 키/텍스트 입력 — 예광탄 범위: 일반 문자, Enter, Backspace, 방향키, Ctrl+C.
        // PageUp/PageDown/휠 스크롤은 셸로 보내는 게 아니라 로컬 스크롤백 뷰포트만
        // 옮긴다(alacritty_terminal이 이미 들고 있는 스크롤백을 그대로 활용 —
        // architecture 규칙에 없던 새 기능, TR-002 "생략" 목록에 있던 항목을
        // 실사용 피드백으로 지금 채워 넣음).
        let mut input_bytes: Vec<u8> = Vec::new();
        let mut scroll_lines: i32 = 0;
        let mut page_scroll: Option<alacritty_terminal::grid::Scroll> = None;
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
                        egui::Key::C if modifiers.ctrl => input_bytes.push(0x03),
                        egui::Key::PageUp => page_scroll = Some(alacritty_terminal::grid::Scroll::PageUp),
                        egui::Key::PageDown => page_scroll = Some(alacritty_terminal::grid::Scroll::PageDown),
                        _ => {}
                    },
                    _ => {}
                }
            }
        });
        if !input_bytes.is_empty() {
            let _ = session.write_keyboard_input(&input_bytes);
        }

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

        let cols = (rect.width() / cell.x).floor().max(1.0) as u16;
        let rows = (rect.height() / cell.y).floor().max(1.0) as u16;
        session.resize(cols, rows);

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

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, DEFAULT_BG);

        let content = session.term.renderable_content();
        // alacritty_terminal의 display_iter/cursor는 grid 좌표(스크롤 안 된 활성
        // 영역 기준 Line(0))를 그대로 준다 — display_offset만큼 떨어진 뷰포트
        // 좌표가 아니다(DEV-003 스크롤백 구현 중 발견: PageUp을 눌러 display_offset이
        // 바뀌면 display_iter가 내놓는 point.line이 전부 음수가 돼 기존의
        // `point.line.0 < 0` 가드에 전부 걸러져 화면이 통째로 빈 채로 그려졌었다).
        // alacritty_terminal이 제공하는 `term::point_to_viewport()`와 같은 공식
        // (line + display_offset)으로 뷰포트 상대 좌표로 변환해야 한다.
        let display_offset = content.display_offset as i32;
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

            let bg_color = resolve_bg(bg);
            if bg_color != DEFAULT_BG {
                painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, cell.y)), 0.0, bg_color);
            }

            if c != ' ' && c != '\0' {
                let fg_color = resolve_fg(fg);
                painter.text(Pos2::new(x, y), egui::Align2::LEFT_TOP, c, self.font_id.clone(), fg_color);
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
            painter.rect_stroke(
                Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell.x, cell.y)),
                0.0,
                egui::Stroke::new(1.5, Color32::from_rgb(0xff, 0xff, 0xff)),
                egui::StrokeKind::Outside,
            );
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
}
