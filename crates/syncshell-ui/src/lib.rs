mod ansi_color;
mod font_fallback;
mod terminal_widget;

use std::path::PathBuf;
use syncshell_core::fsview::FsView;
use syncshell_core::sync::SyncState;
use syncshell_core::terminal::TerminalSession;
use terminal_widget::TerminalWidget;

pub struct SyncShellApp {
    terminal: Option<TerminalSession>,
    terminal_widget: TerminalWidget,
    fs: FsView,
    sync: SyncState,
    /// 탐색기 클릭으로 만든 cd 명령인데, 사용자가 터미널에 아직 제출 안 한 입력이
    /// 있어서 당장 못 보내고 대기 중인 것. 사용자가 Enter(또는 Ctrl+C 등)로
    /// 그 줄을 끝내면(has_pending_user_input()이 false가 되면) 그때 보낸다
    /// (TR-006 피드백: "탭 자동완성 후 엔터 치면 다른 명령이 따라 들어와서 실패함").
    pending_cd: Option<String>,
}

impl SyncShellApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut terminal_widget = TerminalWidget::new();
        terminal_widget.install_fonts(&cc.egui_ctx);

        let ctx = cc.egui_ctx.clone();
        let terminal = TerminalSession::spawn("powershell.exe", 80, 24, move || ctx.request_repaint()).ok();

        let start_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("C:\\"));
        let ctx = cc.egui_ctx.clone();
        let fs = FsView::new(start_dir, move || ctx.request_repaint());

        Self {
            terminal,
            terminal_widget,
            fs,
            sync: SyncState::new(),
            pending_cd: None,
        }
    }
}

impl eframe::App for SyncShellApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 터미널 → 탐색기: OSC7으로 감지된 cwd 변화를 SyncState에 흘려보낸다.
        // SyncState가 "이건 우리가 주입한 cd의 에코다"라고 판단하면 None을 돌려주므로
        // 탐색기를 다시 옮기지 않는다 — 여기서 무한루프가 끊긴다.
        if let Some(session) = &mut self.terminal {
            session.pump();
            if let Some(cwd) = session.take_cwd_change() {
                if let Some(target) = self.sync.terminal_cwd_changed(cwd) {
                    self.fs.navigate(target);
                }
            }
            // 셸이 종료되면(예: exit 입력) 앱도 같이 닫는다 — 예광탄은 탭/분할이
            // 없어 터미널이 하나뿐이므로, 그게 죽으면 남길 이유가 없다. 이전에는
            // 이 신호를 아예 안 봐서 셸만 멈추고 앱은 빈 창으로 남아있었다
            // (TR-006 피드백).
            if session.exited {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }

            // 대기 중인 cd 주입이 있고 이제 안전해졌으면(사용자가 줄을 끝냈으면) 보낸다.
            if self.pending_cd.is_some() && !session.has_pending_user_input() {
                if let Some(cmd) = self.pending_cd.take() {
                    let _ = session.write_input(cmd.as_bytes());
                }
            }
        }
        self.fs.pump();

        egui::Panel::left("file_panel")
            .resizable(true)
            .default_size(320.0)
            .show(ui, |ui| {
                ui.heading("탐색기");
                ui.label(self.fs.current_dir.display().to_string());
                ui.separator();

                let mut navigate_to: Option<PathBuf> = None;

                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(parent) = self.fs.current_dir.parent() {
                        if ui.selectable_label(false, "..").clicked() {
                            navigate_to = Some(parent.to_path_buf());
                        }
                    }
                    if let Some(err) = &self.fs.error {
                        ui.colored_label(egui::Color32::from_rgb(0xe0, 0x6c, 0x75), err);
                    }
                    for entry in &self.fs.entries {
                        let label = if entry.is_dir {
                            format!("📁 {}", entry.name)
                        } else {
                            format!("   {}", entry.name)
                        };
                        let response = ui.selectable_label(false, &label);
                        if entry.is_dir {
                            // 폴더는 단일 클릭 진입 유지 — TR-003/TR-005의 동기화
                            // 설계가 이걸 전제로 짜여 있다(DEV-015 조사 결과, 폴더까지
                            // 더블클릭으로 바꾸면 sync 쪽 영향 범위가 커서 보류).
                            if response.clicked() {
                                navigate_to = Some(entry.path.clone());
                            }
                        } else if response.double_clicked() {
                            // 파일은 반드시 더블클릭 — 한 번만 클릭해도 실행파일이
                            // 열려버리던 문제(TR-006 피드백)를 막기 위해 일반
                            // 탐색기처럼 더블클릭을 요구한다.
                            // .lnk 바로가기는 open이 셸 수준(ShellExecute)에서 대상을
                            // 알아서 찾아 연다 — 별도 파싱 불필요(TR-006 피드백).
                            let _ = open::that(&entry.path);
                        }
                    }
                });

                // 탐색기 → 터미널: 사용자가 실제로 클릭했을 때만 SyncState.user_navigated를
                // 거친다 (터미널이 유발한 이동은 위에서 self.fs.navigate를 직접 호출하고
                // 여기를 거치지 않으므로 다시 주입되지 않는다).
                if let Some(path) = navigate_to {
                    self.fs.navigate(path.clone());
                    let cmd = self.sync.user_navigated(path);
                    // 사용자가 터미널에 아직 제출 안 한 입력이 있으면(예: 탭 자동완성
                    // 중) 지금 주입하면 그 줄 중간에 섞여 들어가 명령이 깨진다 — 안전할
                    // 때까지 대기시킨다(위 ui() 상단에서 매 프레임 재확인해 흘려보냄).
                    let pending = self
                        .terminal
                        .as_ref()
                        .map(|s| s.has_pending_user_input())
                        .unwrap_or(false);
                    if pending {
                        self.pending_cd = Some(cmd);
                    } else if let Some(session) = &mut self.terminal {
                        let _ = session.write_input(cmd.as_bytes());
                    }
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(session) = &mut self.terminal {
                    self.terminal_widget.show(ui, session);
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("셸을 시작하지 못했습니다 (powershell.exe 확인 필요)");
                    });
                }
            });
    }

    // architecture 규칙: 영속 데이터는 ~/.syncshell/ 하나로 통일한다.
    // eframe 자체 persistence는 쓰지 않는다 (기본 feature도 꺼져 있지만 명시적으로 재확인).
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

pub fn run() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        persist_window: false,
        viewport: egui::ViewportBuilder::default()
            .with_title("syncshell")
            // 1000px 폭이면 터미널 패널이 ~47컬럼밖에 안 나와서(맑은 고딕이 완전한
            // monospace가 아니라 셀 폭을 넓은 'M' 기준으로 잡는 것도 한몫함),
            // `Get-ChildItem`/`ls`의 기본 테이블 포맷터가 이름 컬럼을 통째로
            // 드롭해버리는 걸 실측으로 확인함(TR-006 피드백 — 이건 PowerShell
            // 자체의 폭 적응 동작이지 렌더러의 wrap 버그는 아니었음). 기본 창을
            // 넓혀서 실사용에서 이 상황을 덜 겪게 한다.
            .with_inner_size([1400.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "syncshell",
        native_options,
        Box::new(|cc| Ok(Box::new(SyncShellApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    fn pointer_input(pos: egui::Pos2, pressed: bool, time: f64) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)));
        input.time = Some(time);
        input.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        input
    }

    /// DEV-015 회귀 테스트: egui의 `double_clicked()`가 이 버전에서 기대대로
    /// 동작하는지 — 한 번 클릭으로는 false, 짧은 시간 안에 같은 위치를 두 번
    /// 클릭하면 true. 실제 파일 목록 위젯과 동일한 판정 메커니즘을 최소
    /// 시나리오(버튼 하나)로 검증한다(전체 레이아웃 좌표를 재현할 필요 없음).
    #[test]
    fn double_click_detection_requires_two_quick_clicks() {
        let ctx = egui::Context::default();

        // 버튼이 실제로 어디 그려지는지 먼저 알아낸다(레이아웃만, 클릭 없음).
        let mut button_rect = egui::Rect::NOTHING;
        let mut probe = egui::RawInput::default();
        probe.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)));
        let _ = ctx.run_ui(probe, |ui| {
            button_rect = ui.button("test").rect;
        });
        let pos = button_rect.center();

        let step = |pressed: bool, time: f64, label: &str| {
            let mut result = (false, false);
            let _ = ctx.run_ui(pointer_input(pos, pressed, time), |ui| {
                let resp = ui.button("test");
                println!(
                    "{label}: clicked={} double_clicked={}",
                    resp.clicked(),
                    resp.double_clicked()
                );
                result = (resp.clicked(), resp.double_clicked());
            });
            result
        };

        step(true, 0.0, "press1");
        let (c1, d1) = step(false, 0.05, "release1");
        step(true, 0.15, "press2");
        let (c2, d2) = step(false, 0.2, "release2");

        assert!(c1, "첫 클릭이 clicked()로도 안 잡힘 — 테스트 시나리오 자체가 잘못됨");
        assert!(!d1, "첫 클릭만으로 double_clicked()가 true가 됨");
        assert!(c2, "두 번째 클릭이 clicked()로도 안 잡힘");
        assert!(d2, "짧은 간격의 두 번째 클릭에서 double_clicked()가 false — 더블클릭 감지 자체가 깨짐");
    }
}
