mod ansi_color;
mod chrome;
mod file_panel;
mod font_fallback;
mod terminal_widget;
mod theme;

use file_panel::{FileAction, FilePanelState};
use std::path::{Path, PathBuf};
use syncshell_core::fsops;
use syncshell_core::fsview::FsView;
use syncshell_core::sync::SyncState;
use syncshell_core::terminal::TerminalSession;
use terminal_widget::TerminalWidget;
use theme::ThemeMode;

/// 탭 하나 — 독립된 (탐색기 + 터미널 세션 + 동기화 상태) 묶음(DEV-009).
///
/// **알려진 한계**: 탭마다 `TerminalWidget`(따라서 `FontFallback`)을 독립적으로
/// 갖는다. `FontFallback::ensure_covers`가 코드포인트를 새로 찾을 때마다
/// `egui::Context::set_fonts`로 **자기가 아는 폰트 전체를 통째로** 다시 설정하는데,
/// 이건 프로세스 전체에 하나뿐인 egui 폰트 테이블을 매번 덮어쓰는 것과 같다.
/// 그래서 탭 A가 방금 찾은 폴백 폰트를, 탭 B가 (A는 모르는) 자기 폴백을 등록하며
/// 되돌려버릴 수 있다 — 드문 비ASCII 코드포인트가 탭 전환 후 일시적으로 두부로
/// 보였다가 그 탭이 그 글자를 다시 그리는 프레임에 스스로 복구되는 정도의 시각적
/// 결함이다(데이터 손상이나 크래시는 아님). 제대로 고치려면 `FontFallback`을
/// 앱 전역에서 하나만 공유하도록 `TerminalWidget`의 공개 API를 바꿔야 하는데,
/// 그 API를 쓰는 Windows 전용 테스트 다수를 이 머신(macOS)에서 컴파일 확인할
/// 방법이 없어 — 검증 안 된 변경으로 Windows 빌드를 깨뜨릴 위험을 피하려고
/// 지금은 보류했다.
struct Tab {
    terminal: Option<TerminalSession>,
    terminal_widget: TerminalWidget,
    fs: FsView,
    sync: SyncState,
    /// 탐색기 클릭으로 만든 cd 명령인데, 사용자가 터미널에 아직 제출 안 한 입력이
    /// 있어서 당장 못 보내고 대기 중인 것. 사용자가 Enter(또는 Ctrl+C 등)로
    /// 그 줄을 끝내면(has_pending_user_input()이 false가 되면) 그때 보낸다
    /// (TR-006 피드백: "탭 자동완성 후 엔터 치면 다른 명령이 따라 들어와서 실패함").
    pending_cd: Option<String>,
    /// 탐색기 패널이 프레임 간에 들고 있어야 하는 상태(이름 바꾸기 대화상자, 상태 메시지).
    panel: FilePanelState,
    /// 마지막으로 클릭한 게 어느 패널인지(파일 복사/붙여넣기 단축키가 지금
    /// 반응해야 하는지 판단하는 데만 쓴다 — `ActivePanel` 설명 참고).
    active_panel: ActivePanel,
}

/// "지금 사용자가 어느 패널을 쓰고 있다고 봐야 하는가"의 근사치. 진짜 egui
/// 키보드 포커스를 못 쓰는 이유: 터미널 위젯이 매 프레임 무조건 자기 포커스를
/// 가져가므로(architecture 규칙 8, `claim_terminal_focus`) `ctx.memory(|m|
/// m.focused())`는 사실상 항상 터미널을 가리킨다. 그래서 "마지막으로 어디를
/// 클릭했는지"를 우리가 직접 추적한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ActivePanel {
    #[default]
    Terminal,
    Explorer,
}

impl Tab {
    /// `shell_override`는 DEV-010 `config.toml`의 `default_shell` — 지정돼
    /// 있으면 `default_shell()`의 자동 판단(플랫폼별 셸 추정)보다 우선한다.
    fn new(ctx: &egui::Context, start_dir: PathBuf, shell_override: Option<&str>) -> Self {
        let mut terminal_widget = TerminalWidget::new();
        terminal_widget.install_fonts(ctx);

        let repaint_ctx = ctx.clone();
        let shell = shell_override
            .map(|s| s.to_string())
            .unwrap_or_else(syncshell_core::terminal::default_shell);
        let terminal = TerminalSession::spawn(&shell, 80, 24, move || repaint_ctx.request_repaint()).ok();

        let repaint_ctx = ctx.clone();
        let fs = FsView::new(start_dir, move || repaint_ctx.request_repaint());

        Self {
            terminal,
            terminal_widget,
            fs,
            sync: SyncState::new(),
            pending_cd: None,
            panel: FilePanelState::default(),
            active_panel: ActivePanel::default(),
        }
    }

    /// 탭 바에 보여줄 이름 — 현재 폴더의 마지막 이름(비어 있으면, 예: 드라이브
    /// 루트면 전체 경로).
    fn title(&self) -> String {
        display_name(&self.fs.current_dir)
    }

    /// 탐색기가 폴더로 이동할 때 터미널에도 `cd`를 흘려보낸다.
    ///
    /// 사용자가 터미널에 아직 제출 안 한 입력이 있으면(예: 탭 자동완성 중) 지금
    /// 주입하면 그 줄 중간에 섞여 들어가 명령이 깨진다 — 안전할 때까지 대기시킨다
    /// (매 프레임 `pump()`에서 재확인해 흘려보냄. TR-006 피드백).
    fn inject_cd(&mut self, cmd: String) {
        let Some(session) = &mut self.terminal else {
            return;
        };
        if session.has_pending_user_input() {
            // 사용자가 아직 제출 안 한 줄을 타이핑하는 중이다. 그 줄 끝에 그냥
            // 이어붙이면 명령이 깨진다(TR-006 "탭 자동완성 후 엔터 치면 다른
            // 명령이 따라 들어와서 실패함"과 같은 문제).
            if session.supports_line_prefix_insert() {
                // Ctrl+A(줄 맨 앞으로 — bash/zsh 기본 이맥스 키바인딩, 실측
                // 확인됨)로 커서를 옮기고 그 앞에 "cd 새경로; "를 끼워 넣는다.
                // 제출은 안 한다 — 사용자가 타이핑하던 내용은 그대로 뒤에
                // 남아 있다가, Enter를 누르면 "cd 새경로; 원래타이핑" 형태로
                // 새 위치에서 실행된다(실사용 피드백: 타이핑한 내용이 사라지지
                // 않고 새 폴더로 그대로 넘어가길 원함).
                let prefix = cmd.trim_end_matches('\r');
                let _ = session.write_input(&[0x01]);
                let _ = session.write_input(format!("{prefix}; ").as_bytes());
            } else {
                // PowerShell(PSReadLine 기본 "Windows" 모드)은 Ctrl+A가 "줄 맨
                // 앞으로"가 아니라 "전체 선택"이라, 그 상태에서 새로 타이핑하면
                // 선택된(=사용자가 치던 전체 줄) 내용을 지워버린다 — 이 방법을
                // 못 믿는 셸에서는 대신 줄을 취소(0x03 — BUG-001에서 모든
                // 셸에서 프롬프트 줄을 취소하는 걸로 이미 검증된 방법)하고
                // cd를 곧바로 적용한다. 타이핑하던 내용은 사라진다.
                let _ = session.write_input(&[0x03]);
                let _ = session.write_input(cmd.as_bytes());
            }
        } else if session.is_command_running() {
            // 지금 셸이 명령을 실행 중이면(프롬프트에서 줄을 읽는 게 아니라)
            // 이 바이트가 PTY 입력 큐에 쌓였다가, 그 프로그램이 stdin을 읽는
            // 종류(REPL·페이저·대화형 프롬프트 등)라면 엉뚱하게 그리로 들어갈
            // 수 있다 — 다음 프롬프트가 뜰 때까지 미룬다.
            self.pending_cd = Some(cmd);
        } else {
            let _ = session.write_input(cmd.as_bytes());
        }
    }

    /// 조작 후 목록을 갱신한다. 워처(DEV-007)가 어차피 잡아주지만, 사용자가
    /// 방금 누른 동작의 결과는 워처 디바운스(100ms)를 기다리지 않고 바로 보이는
    /// 편이 자연스럽다.
    fn refresh(&mut self) {
        let current = self.fs.current_dir.clone();
        self.fs.navigate(current);
    }

    fn handle_file_action(&mut self, ctx: &egui::Context, action: FileAction) {
        match action {
            FileAction::Navigate(path) => {
                // 탐색기 → 터미널: 사용자가 실제로 클릭했을 때만 SyncState.user_navigated를
                // 거친다(터미널이 유발한 이동은 pump()에서 self.fs.navigate를 직접
                // 호출하고 여기를 거치지 않으므로 다시 주입되지 않는다).
                self.fs.navigate(path.clone());
                let cmd = self.sync.user_navigated(path);
                self.inject_cd(cmd);
            }
            FileAction::OpenFile(path) => {
                // .lnk 바로가기는 open이 셸 수준(ShellExecute)에서 대상을 알아서
                // 찾아 연다 — 별도 파싱 불필요(TR-006 피드백).
                if let Err(e) = open::that(&path) {
                    self.panel.set_error(format!("열 수 없습니다: {e}"));
                }
            }
            FileAction::OpenTerminalHere(path) => {
                // 탐색기는 그대로 두고 터미널만 그 폴더로 옮긴다. SyncState에도
                // 알려줘야 터미널의 OSC7 응답을 "우리가 시킨 것"으로 인식하고
                // 탐색기를 되돌리지 않는다.
                let cmd = self.sync.user_navigated(path.clone());
                self.inject_cd(cmd);
                self.fs.navigate(path);
            }
            FileAction::CopyPath(path) => {
                ctx.copy_text(path.display().to_string());
                self.panel.set_info("경로를 복사했습니다");
            }
            FileAction::BeginRename(path) => {
                self.panel.begin_rename(&path);
            }
            FileAction::Rename(path, new_name) => match fsops::rename(&path, &new_name) {
                Ok(_) => {
                    self.panel.status = None;
                    self.refresh();
                }
                Err(e) => self.panel.set_error(e.to_string()),
            },
            FileAction::MoveToTrash(path) => {
                let name = display_name(&path);
                match fsops::move_to_trash(&path) {
                    Ok(()) => {
                        self.panel.set_info(format!("'{name}'을(를) 휴지통으로 보냈습니다"));
                        self.refresh();
                    }
                    Err(e) => self.panel.set_error(format!("삭제할 수 없습니다: {e}")),
                }
            }
            FileAction::NewFolder => {
                let dir = self.fs.current_dir.clone();
                match fsops::create_dir_unique(&dir, "새 폴더") {
                    Ok(created) => {
                        self.panel.status = None;
                        self.refresh();
                        // 만들자마자 이름을 바꾸도록 대화상자를 띄운다 — 탐색기가
                        // 새 폴더를 만들면 곧바로 이름 편집 상태로 들어가는 것과 같은 흐름.
                        self.panel.begin_rename(&created);
                    }
                    Err(e) => self.panel.set_error(format!("폴더를 만들 수 없습니다: {e}")),
                }
            }
            FileAction::Refresh => self.refresh(),
            FileAction::Copy(path) => {
                self.panel.clipboard = Some(file_panel::ClipboardEntry { path, cut: false });
                self.panel.set_info("복사함");
            }
            FileAction::Cut(path) => {
                self.panel.clipboard = Some(file_panel::ClipboardEntry { path, cut: true });
                self.panel.set_info("잘라냄");
            }
            FileAction::Paste => {
                let Some(entry) = self.panel.clipboard.clone() else {
                    return;
                };
                let dest_dir = self.fs.current_dir.clone();
                let result = if entry.cut {
                    fsops::move_into(&entry.path, &dest_dir)
                } else {
                    fsops::copy_into(&entry.path, &dest_dir)
                };
                match result {
                    Ok(_) => {
                        // 잘라내기는 원본이 이미 옮겨졌으니 한 번 붙여넣으면 클립보드를
                        // 비운다 — 다시 붙여넣으려 하면 원본을 못 찾아 오류가 나는데,
                        // 그보다는 메뉴에서 "붙여넣기"가 곧바로 비활성화되는 쪽이 낫다.
                        if entry.cut {
                            self.panel.clipboard = None;
                        }
                        self.panel.status = None;
                        self.refresh();
                    }
                    Err(e) => self.panel.set_error(format!("붙여넣기 실패: {e}")),
                }
            }
        }
    }

    /// DEV-008: OS 파일 관리자(Finder/탐색기 등)에서 끌어다 놓은 파일을 받는다.
    /// 터미널 영역에 놓으면 경로 텍스트를 삽입하고(실행은 안 함 — 사용자가 이어서
    /// 타이핑하거나 Enter를 누를 수 있게), 그 밖(주로 탐색기 패널)에 놓으면 현재
    /// 폴더로 복사한다.
    fn handle_dropped_files(&mut self, ctx: &egui::Context, terminal_rect: egui::Rect) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if dropped.is_empty() {
            return;
        }

        let drop_pos = ctx.input(|i| i.pointer.interact_pos());
        let onto_terminal = drop_pos.is_some_and(|p| terminal_rect.contains(p));

        if onto_terminal {
            if let Some(session) = &mut self.terminal {
                // 경로마다 큰따옴표로 감싼다 — sync.rs의 cd 주입과 같은 인용 규칙
                // (PowerShell·zsh 양쪽에서 공백·특수문자 포함 경로를 안전하게 다룸).
                let text = dropped
                    .iter()
                    .map(|p| format!("\"{}\"", p.display()))
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = session.write_keyboard_input(text.as_bytes());
            }
        } else {
            let dest_dir = self.fs.current_dir.clone();
            let mut failed = Vec::new();
            for path in &dropped {
                if let Err(e) = fsops::copy_into(path, &dest_dir) {
                    failed.push(format!("{}: {e}", path.display()));
                }
            }
            if failed.is_empty() {
                self.panel.set_info(format!("{}개 항목을 복사했습니다", dropped.len()));
            } else {
                self.panel.set_error(format!("일부 항목을 복사하지 못했습니다: {}", failed.join(", ")));
            }
            self.refresh();
        }
    }

    /// 매 프레임 시작에 부르는 배경 작업(OSC7 cwd 동기화, 셸 종료 감지, 대기 중인
    /// cd 흘려보내기, 워처 pump). 셸이 종료됐으면 true를 돌려준다 — 호출부가
    /// 탭을 닫을지 창을 닫을지 결정한다(탭이 여러 개면 이 탭만 닫는다).
    fn pump(&mut self) -> bool {
        let mut shell_exited = false;
        if let Some(session) = &mut self.terminal {
            session.pump();
            if let Some(cwd) = session.take_cwd_change() {
                if let Some(target) = self.sync.terminal_cwd_changed(cwd) {
                    self.fs.navigate(target);
                }
            }
            if session.exited {
                shell_exited = true;
            }

            // 대기 중인 cd 주입이 있고 이제 안전해졌으면(사용자가 줄을 끝냈고,
            // 실행 중이던 명령도 끝났으면) 보낸다.
            if self.pending_cd.is_some() && !session.has_pending_user_input() && !session.is_command_running() {
                if let Some(cmd) = self.pending_cd.take() {
                    let _ = session.write_input(cmd.as_bytes());
                }
            }
        }
        self.fs.pump();
        shell_exited
    }
}

pub struct SyncShellApp {
    tabs: Vec<Tab>,
    /// 지금 화면에 보이는 탭의 인덱스. 항상 `tabs`의 유효한 범위를 가리킨다
    /// (탭이 최소 하나는 항상 있으므로 — 마지막 탭은 닫을 수 없다).
    active: usize,
    /// DEV-010 `config.toml`에서 읽은 값 — 새 탭을 열 때마다 다시 쓴다.
    default_shell: Option<String>,
    /// 매 프레임 갱신되는 창 위치/크기 — `on_exit`에는 `egui::Context`가 안 넘어와서
    /// (eframe API 제약) 여기 캐시해뒀다가 종료 시 `state.toml`에 쓴다.
    last_window_rect: Option<egui::Rect>,
    /// 탐색기를 터미널 기준 어느 쪽에 둘지(실사용 요청: "위/아래로도 배치할 수
    /// 있게"). 탭마다 다르면 탭 전환할 때마다 레이아웃이 바뀌어 오히려 헷갈려서
    /// 세션 전체에 하나만 둔다(탭별이 아님).
    panel_layout: PanelLayout,
    /// 다크/라이트(DEV-021). 앱 전역 하나 — `state.toml`의 `[ui] theme`으로 저장된다.
    theme: ThemeMode,
}

/// [`SyncShellApp::panel_layout`] 참고.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PanelLayout {
    /// 탐색기(왼쪽) | 터미널(오른쪽) — 기존 기본값.
    #[default]
    Side,
    /// 탐색기(위) / 터미널(아래).
    Stacked,
}

impl PanelLayout {
    fn toggled(self) -> Self {
        match self {
            Self::Side => Self::Stacked,
            Self::Stacked => Self::Side,
        }
    }

    /// 토글 버튼에 보여줄 라벨 — "지금 상태"가 아니라 "눌렀을 때 될 상태"를
    /// 보여준다(대부분의 토글 버튼 관례 — 지금 상태를 그대로 라벨로 쓰면 눌러도
    /// 안 바뀐 것처럼 헷갈린다).
    fn toggle_label(self) -> &'static str {
        match self {
            Self::Side => "레이아웃: 상하로",
            Self::Stacked => "레이아웃: 좌우로",
        }
    }
}

impl SyncShellApp {
    pub fn new(cc: &eframe::CreationContext<'_>, state: syncshell_core::session::State) -> Self {
        let config = syncshell_core::session::load_config();
        let fallback_dir = std::env::current_dir().unwrap_or_else(|_| {
            if cfg!(windows) {
                PathBuf::from("C:\\")
            } else {
                PathBuf::from("/")
            }
        });

        // DEV-010: 저장된 탭이 있으면 복원한다. 폴더가 그새 지워졌으면(다른
        // 프로그램이 지웠거나 드라이브가 빠짐) 존재하는 조상까지 조용히 올라가고,
        // 그마저 없으면 기본 시작 위치로 — 크래시 없이 뭔가는 뜨게 한다.
        let mut tabs: Vec<Tab> = state
            .tabs
            .iter()
            .map(|t| {
                let dir = syncshell_core::session::resolve_existing_or_fallback(&t.path, &fallback_dir);
                Tab::new(&cc.egui_ctx, dir, config.default_shell.as_deref())
            })
            .collect();
        if tabs.is_empty() {
            tabs.push(Tab::new(&cc.egui_ctx, fallback_dir, config.default_shell.as_deref()));
        }
        let active = state.active_tab.min(tabs.len() - 1);

        let theme = ThemeMode::parse(&state.ui.theme);
        theme::apply(&cc.egui_ctx, theme);

        Self {
            tabs,
            active,
            default_shell: config.default_shell,
            last_window_rect: None,
            panel_layout: PanelLayout::default(),
            theme,
        }
    }

    /// 종료 시 `state.toml`에 쓸 내용(DEV-010). `on_exit`과 분리해둔 건 저장
    /// 내용을 파일을 거치지 않고 테스트하기 위해서다.
    fn session_state(&self) -> syncshell_core::session::State {
        syncshell_core::session::State {
            tabs: self
                .tabs
                .iter()
                .map(|t| syncshell_core::session::TabState { path: t.fs.current_dir.clone() })
                .collect(),
            active_tab: self.active,
            window: self.last_window_rect.map(|r| syncshell_core::session::WindowState {
                x: r.min.x,
                y: r.min.y,
                width: r.width(),
                height: r.height(),
            }),
            ui: syncshell_core::session::UiState { theme: self.theme.as_str().to_string() },
        }
    }

    fn set_theme(&mut self, ctx: &egui::Context, theme: ThemeMode) {
        if self.theme != theme {
            self.theme = theme;
            theme::apply(ctx, theme);
        }
    }

    /// 새 탭을 열고 그리로 전환한다. 지금 보고 있는 탭의 폴더에서 이어서
    /// 시작한다 — 매번 홈/실행 위치로 되돌아가는 것보다 자연스럽다.
    fn new_tab(&mut self, ctx: &egui::Context) {
        let start_dir = self.tabs[self.active].fs.current_dir.clone();
        self.tabs.push(Tab::new(ctx, start_dir, self.default_shell.as_deref()));
        self.active = self.tabs.len() - 1;
    }

    /// 탭을 닫는다. 마지막 하나 남은 탭은 닫지 않는다 — 앱이 탭 0개인 상태로
    /// 남는 것보다, 마지막 탭을 닫고 싶으면 창을 닫는 쪽이 자연스럽다(대부분의
    /// 탭 기반 앱과 같은 동작).
    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || self.tabs.len() <= 1 {
            return;
        }
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if self.active > index {
            self.active -= 1;
        }
    }

    /// 탭 바를 그리고, 새 탭/탭 전환/탭 닫기 키보드 단축키를 처리한다.
    /// 단축키는 `consume_key`로 입력 큐에서 제거해야 한다 — 안 그러면 이 프레임
    /// 뒤에 그려지는 터미널 위젯이 같은 키 이벤트를 "타이핑"으로 오인해서
    /// 셸에까지 흘러들어간다.
    fn show_tab_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        use egui::{Key, Modifiers};

        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::T)) {
            self.new_tab(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W)) {
            self.close_tab(self.active);
        }
        // Ctrl/Cmd+Tab로 다음 탭, +Shift로 이전 탭 — 브라우저와 같은 관례.
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Tab)) {
            self.active = (self.active + 1) % self.tabs.len();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab)) {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }

        let mut switch_to = None;
        let mut close_index = None;
        let mut want_new_tab = false;
        ui.horizontal(|ui| {
            // 레이아웃 방향 토글(실사용 요청: "위|아래로도 배치할 수 있게")을
            // 먼저 그려서 탭 바 오른쪽 끝에 고정시킨다 — 이 안에서 다시
            // right_to_left로 그리면 오른쪽부터 채워지고, 뒤이어 그리는 탭들은
            // (아래 left_to_right 그대로) 남은 공간을 왼쪽부터 채운다.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // right_to_left라 먼저 그린 게 맨 오른쪽 — 테마 전환이 끝에 온다.
                let pal = self.theme.palette();
                let picked = chrome::segmented(
                    ui,
                    pal,
                    &[
                        (chrome::Icon::Moon, "다크 모드", self.theme == ThemeMode::Dark),
                        (chrome::Icon::Sun, "라이트 모드", self.theme == ThemeMode::Light),
                    ],
                );
                match picked {
                    Some(0) => self.set_theme(ctx, ThemeMode::Dark),
                    Some(1) => self.set_theme(ctx, ThemeMode::Light),
                    _ => {}
                }
                if ui.button(self.panel_layout.toggle_label()).clicked() {
                    self.panel_layout = self.panel_layout.toggled();
                }
            });

            // DEV-014: macOS는 투명 타이틀바(run()에서 설정) 위에 탭 바가 신호등
            // 버튼과 같은 줄에 뜬다 — 버튼 폭만큼 왼쪽을 비워야 겹치지 않는다.
            // 신호등은 항상 왼쪽에 고정 폭(~78px)으로 뜨므로 하드코딩 가능.
            #[cfg(target_os = "macos")]
            ui.add_space(78.0);

            for (i, tab) in self.tabs.iter().enumerate() {
                ui.push_id(i, |ui| {
                    let selected = i == self.active;
                    if ui.selectable_label(selected, tab.title()).clicked() {
                        switch_to = Some(i);
                    }
                    // "✕"(U+2715)가 아니라 "×"(U+00D7) — 대부분의 일반 폰트가
                    // U+2715는 안 갖고 있어 두부로 보이는 실사용 버그가 있었다
                    // (file_panel.rs 상태 메시지 닫기 버튼과 같은 문제, fontdb로 실측).
                    if self.tabs.len() > 1 && ui.small_button("×").clicked() {
                        close_index = Some(i);
                    }
                });
            }
            if ui.button("+").on_hover_text("새 탭 (Ctrl/Cmd+T)").clicked() {
                want_new_tab = true;
            }
        });

        if want_new_tab {
            self.new_tab(ctx);
        } else if let Some(i) = switch_to {
            self.active = i;
        }
        if let Some(i) = close_index {
            self.close_tab(i);
        }
    }
}

/// macOS 커스텀 타이틀바(투명 titlebar 위에 탭 바가 신호등 버튼과 같은 줄에
/// 뜸, `run()`의 macOS 전용 크롬 설정 참고)에서 탭 바가 신호등 버튼 높이보다
/// 얇으면 버튼이 탭 바 위로 삐져나온 것처럼 겹쳐 보인다(실사용 피드백:
/// "신호등 버튼이 타이틀바에 걸쳐진 모양새" — 스크린샷으로 확인).
///
/// 28.0은 임의값이 아니라 macOS 표준 "툴바 없는" 타이틀바 높이다(Terminal.app,
/// TextEdit 등 NSToolbar를 안 쓰는 앱들이 공통으로 쓰는 값 — Big Sur 이후
/// 늘어난 큰 타이틀바(~52pt)는 NSToolbar가 있는 앱(Finder, Safari 등)에만
/// 해당하고 우리는 툴바가 없으므로 무관). 처음엔 38.0으로 짚었다가 실사용
/// 확인(스크린샷)에서 "너무 넓어짐" 피드백을 받고서 macOS 표준값으로
/// 낮췄다 — "맥 기본 터미널의 타이틀이랑 같은 높이"라는 요청과 정확히 일치.
const TAB_BAR_HEIGHT: f32 = 32.0;

impl eframe::App for SyncShellApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // `default_size`가 아니라 `exact_size`를 써야 한다 — `default_size`는
        // 첫 프레임 초기값 힌트일 뿐 허용 범위를 그대로 열어두고(egui 소스
        // 확인: min/max range가 안 좁혀짐), 이후 프레임에서 내용(탭 한 줄) 쪽으로
        // 다시 줄어들 수 있다. 처음엔 이걸 몰라서 default_size로 고쳤다고
        // 보고했는데, 실사용 확인에서 여전히 얇았다 — `exact_size`는 범위를
        // 그 값 하나로 완전히 고정한다(`Rangef::point`).
        let pal = self.theme.palette();
        let chrome_frame = egui::Frame::NONE
            .fill(pal.chrome)
            .inner_margin(egui::Margin::symmetric(8, 0))
            .stroke(egui::Stroke::NONE);
        egui::Panel::top("tab_bar").exact_size(TAB_BAR_HEIGHT).frame(chrome_frame).show(ui, |ui| {
            // 탭 바 내용(`show_tab_bar`가 그리는 가로 한 줄)을 패널의 전체
            // 높이 안에서 세로 가운데 정렬한다 — 안 그러면 내용이 패널 맨
            // 위에 붙어 그려지고 아래쪽 여백만 늘어나 신호등 버튼과 높이가
            // 안 맞는다.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                self.show_tab_bar(&ui.ctx().clone(), ui);
            });
        });

        // 활성 탭의 셸이 종료됐으면(예: exit 입력) 탭을 닫는다 — 탭이 하나뿐이면
        // 창까지 같이 닫는다(이전에는 이 신호를 아예 안 봐서 셸만 멈추고 앱은
        // 빈 창으로 남아있었다, TR-006 피드백).
        if self.tabs[self.active].pump() {
            if self.tabs.len() <= 1 {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.close_tab(self.active);
            }
        }

        let theme = self.theme;
        let active = &mut self.tabs[self.active];
        active.terminal_widget.set_theme(theme);

        let mut action = None;
        let panel_active = active.active_panel == ActivePanel::Explorer;
        // 레이아웃 방향(실사용 요청: "위|아래로도 배치할 수 있게") — 좌우일 땐
        // 폭을, 상하일 땐 높이를 드래그로 조절한다. `Panel::left`/`Panel::top`
        // 둘 다 같은 빌더 API(`resizable`/`default_size`/`show`)를 쓰므로 어느
        // 쪽으로 만들지만 갈라주면 나머지는 공통이다.
        let file_panel_base = match self.panel_layout {
            PanelLayout::Side => egui::Panel::left("file_panel"),
            PanelLayout::Stacked => egui::Panel::top("file_panel"),
        };
        let default_size = match self.panel_layout {
            PanelLayout::Side => 320.0,
            PanelLayout::Stacked => 220.0,
        };
        let file_panel = file_panel_base.resizable(true).default_size(default_size).show(ui, |ui| {
            action = file_panel::show(ui, &active.fs, &mut active.panel, panel_active);
        });
        if let Some(action) = action {
            active.handle_file_action(ui.ctx(), action);
        }

        let central = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(session) = &mut active.terminal {
                    active.terminal_widget.show(ui, session);
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label(format!(
                            "셸을 시작하지 못했습니다 ({} 확인 필요)",
                            syncshell_core::terminal::default_shell()
                        ));
                    });
                }
            });

        active.handle_dropped_files(ui.ctx(), central.response.rect);

        // 이번 프레임에 클릭이 있었으면, 그 위치로 "지금 어느 패널을 쓰고
        // 있는지"를 갱신한다 — 다음 프레임의 복사/붙여넣기 단축키 판단에 쓰인다
        // (한 프레임 지연은 사람이 체감 못 함 — `ActivePanel` 설명 참고).
        if let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
            if ui.ctx().input(|i| i.pointer.any_click()) {
                if file_panel.response.rect.contains(pos) {
                    active.active_panel = ActivePanel::Explorer;
                } else if central.response.rect.contains(pos) {
                    active.active_panel = ActivePanel::Terminal;
                }
            }
        }

        // DEV-010: on_exit에는 Context가 안 넘어오므로 창 위치/크기를 매 프레임
        // 여기 캐시해둔다 — 종료 시 이 마지막 값을 state.toml에 쓴다.
        if let Some(rect) = ui.ctx().input(|i| i.viewport().outer_rect) {
            self.last_window_rect = Some(rect);
        }
    }

    // architecture 규칙: 영속 데이터는 ~/.syncshell/ 하나로 통일한다.
    // eframe 자체 persistence는 쓰지 않는다 (기본 feature도 꺼져 있지만 명시적으로 재확인).
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// DEV-010: 종료 시 열린 탭(경로)·활성 탭·창 위치/크기를 `state.toml`에
    /// 저장한다. `config.toml`(사람이 쓰는 설정)은 여기서 절대 건드리지 않는다.
    fn on_exit(&mut self) {
        let state = self.session_state();
        if let Err(e) = syncshell_core::session::save_state(&state) {
            // 저장 실패는 조용히 무시한다 — 세션 복원은 편의 기능이지, 이것
            // 때문에 종료 자체가 막히거나 사용자에게 강제로 뭘 시키면 안 된다.
            eprintln!("세션 상태를 저장하지 못함: {e}");
        }
    }
}

/// 오류 메시지/탭 제목에 쓸 사람이 읽기 좋은 이름(마지막 경로 조각). 드라이브
/// 루트처럼 파일명이 없는 경우엔 전체 경로를 그대로 쓴다.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn run() -> eframe::Result<()> {
    // DEV-010: 창 크기/위치는 여기서 미리 읽어야 한다 — SyncShellApp::new()가
    // 불릴 시점엔 이미 창이 만들어진 뒤라(eframe이 NativeOptions로 먼저 창을
    // 띄우고 나서 App::new를 호출하는 구조) 너무 늦다.
    let state = syncshell_core::session::load_state();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("syncshell")
        // 1000px 폭이면 터미널 패널이 ~47컬럼밖에 안 나와서(맑은 고딕이 완전한
        // monospace가 아니라 셀 폭을 넓은 'M' 기준으로 잡는 것도 한몫함),
        // `Get-ChildItem`/`ls`의 기본 테이블 포맷터가 이름 컬럼을 통째로
        // 드롭해버리는 걸 실측으로 확인함(TR-006 피드백 — 이건 PowerShell
        // 자체의 폭 적응 동작이지 렌더러의 wrap 버그는 아니었음). 기본 창을
        // 넓혀서 실사용에서 이 상황을 덜 겪게 한다.
        .with_inner_size([1400.0, 800.0]);
    if let Some(w) = &state.window {
        viewport = viewport.with_inner_size([w.width, w.height]).with_position([w.x, w.y]);
    }

    // DEV-014: 플랫폼별 창 크롬 방침 — macOS만 네이티브 유지, 나머지는 직접
    // 그린다(대부분의 크로스플랫폼 에디터가 쓰는 구성). macOS 쪽만 구현했다 —
    // Windows는 `WM_NCHITTEST`/Win11 스냅 레이아웃까지 걸린 raw-window-handle
    // 코드가 필요한데, 이 저장소는 지금 macOS에서만 개발·검증 중이라 Windows
    // 빌드로 확인할 방법이 없는 채로 그 코드를 작성하는 위험을 피했다.
    #[cfg(target_os = "macos")]
    {
        // 타이틀바를 투명하게 만들고(titlebar_shown=false) 콘텐츠가 그 뒤까지
        // 채우게 한다(fullsize_content_view) — VS Code·Zed가 쓰는 표준 패턴.
        // 신호등 버튼(titlebar_buttons_shown)은 계속 보이게 둔다 — 콘텐츠 위에
        // 네이티브로 뜨는 게 이 방침의 핵심이다. 제목 텍스트는 끈다 — 이제
        // 탭 바가 그 자리를 대신한다.
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_titlebar_buttons_shown(true);
    }

    let native_options = eframe::NativeOptions {
        persist_window: false,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "syncshell",
        native_options,
        Box::new(move |cc| Ok(Box::new(SyncShellApp::new(cc, state)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    /// DEV-020 실사용 요청: "위|아래로도 배치할 수 있게". 토글이 실제로
    /// 좌우↔상하를 오가는지, 그리고 라벨이 "지금 상태"가 아니라 "눌렀을 때
    /// 될 상태"를 보여주는지(안 그러면 눌러도 안 바뀐 것처럼 헷갈림) 확인한다.
    #[test]
    fn panel_layout_toggle_switches_between_side_and_stacked() {
        let side = PanelLayout::Side;
        assert_eq!(side.toggled(), PanelLayout::Stacked, "좌우에서 토글했는데 상하로 안 바뀜");
        assert_eq!(side.toggle_label(), "레이아웃: 상하로", "좌우 상태의 버튼 라벨이 '상하로 바꾸기'를 안내하지 않음");

        let stacked = side.toggled();
        assert_eq!(stacked.toggled(), PanelLayout::Side, "상하에서 다시 토글했는데 좌우로 안 돌아옴(왕복 안 됨)");
        assert_eq!(stacked.toggle_label(), "레이아웃: 좌우로", "상하 상태의 버튼 라벨이 '좌우로 바꾸기'를 안내하지 않음");
    }

    /// DEV-021: 테마를 바꾸면 egui 위젯 색(패널 바탕)이 실제로 바뀌고, 종료 시
    /// 저장할 상태에 그 선택이 들어가야 한다(다음 실행에서 복원되는 근거 —
    /// 파일 쓰기 자체는 `on_exit_saves_tabs_and_active_index_to_state_toml`이 본다).
    #[test]
    fn switching_theme_updates_visuals_and_is_saved_on_exit() {
        let ctx = egui::Context::default();
        let mut app = SyncShellApp {
            tabs: vec![Tab::new(&ctx, std::env::temp_dir(), None)],
            active: 0,
            default_shell: None,
            last_window_rect: None,
            panel_layout: PanelLayout::default(),
            theme: ThemeMode::Dark,
        };
        theme::apply(&ctx, ThemeMode::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, theme::DARK.panel);

        app.set_theme(&ctx, ThemeMode::Light);
        assert_eq!(ctx.global_style().visuals.panel_fill, theme::LIGHT.panel, "라이트로 바꿨는데 패널 색이 그대로");

        assert_eq!(app.session_state().ui.theme, "light", "종료 시 저장할 상태에 테마 선택이 안 들어감");
    }

    /// DEV-009: 탭 추가/전환/닫기의 인덱스 계산이 정확한지 — 실제 `Tab::new`(진짜
    /// 셸을 스폰함)로 확인한다. 오프바이원 실수가 나기 쉬운 영역이라 직접 검증.
    #[test]
    fn tab_management_tracks_active_index_correctly() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir();
        let mut app = SyncShellApp {
            tabs: vec![Tab::new(&ctx, dir.clone(), None)],
            active: 0,
            default_shell: None,
            last_window_rect: None,
            panel_layout: PanelLayout::default(),
            theme: ThemeMode::default(),
        };
        assert_eq!(app.tabs.len(), 1);

        // 새 탭을 열면 그 탭이 활성화된다.
        app.new_tab(&ctx);
        app.new_tab(&ctx);
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.active, 2, "새 탭을 열었는데 그 탭이 활성화되지 않음");

        // 활성 탭(인덱스 2)보다 앞(인덱스 0)을 닫으면, 활성 탭은 같은 논리적 탭을
        // 계속 가리켜야 하므로 인덱스가 한 칸 당겨져야 한다.
        app.close_tab(0);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 1, "활성 탭보다 앞의 탭을 닫았는데 활성 인덱스가 안 당겨짐");

        // 지금 활성 탭(인덱스 1, 목록의 마지막) 자체를 닫으면, 활성 인덱스는
        // 범위를 벗어나지 않도록 새 마지막 인덱스로 보정돼야 한다.
        app.close_tab(1);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active, 0, "마지막 탭을 닫았는데 활성 인덱스가 범위를 벗어남");

        // 탭이 하나 남았으면 닫기를 무시한다 — 탭 0개 상태를 만들지 않는다.
        app.close_tab(0);
        assert_eq!(app.tabs.len(), 1, "마지막 탭인데 닫혀버림");
    }

    /// DEV-010: 실제 OS 창 닫기 이벤트를 이 자동화 환경에서 재현할 방법이 없어서
    /// (SIGTERM은 winit 이벤트 루프를 안 거쳐서 `on_exit`이 안 불림 — 직접 실행해서
    /// 확인함), `on_exit()`를 직접 호출해 저장 로직만 따로 검증한다. eframe이
    /// 실제 종료 시 `on_exit()`를 부르는지 자체는 eframe 쪽 계약을 신뢰한다.
    #[test]
    fn on_exit_saves_tabs_and_active_index_to_state_toml() {
        use eframe::App as _;

        let test_home = std::env::temp_dir().join(format!("syncshell-lib-onexit-{}", std::process::id()));
        std::fs::create_dir_all(&test_home).unwrap();
        // SAFETY: 이 테스트 안에서만 쓰고, 끝나면 바로 지운다. 다른 테스트와
        // 병렬로 겹치면 다른 SYNCSHELL_HOME 값을 서로 밟을 수 있지만, 이 크레이트
        // 안에서 SYNCSHELL_HOME을 건드리는 테스트가 이거 하나뿐이라 실제 충돌은
        // 없다(session.rs의 SYNCSHELL_HOME 테스트들은 syncshell-core 쪽에 있고
        // 별도 프로세스/바이너리로 도는 크레이트라 겹치지 않는다).
        unsafe {
            std::env::set_var("SYNCSHELL_HOME", &test_home);
        }

        let ctx = egui::Context::default();
        let dir_a = std::env::temp_dir();
        let dir_b = PathBuf::from("/");
        let mut app = SyncShellApp {
            tabs: vec![Tab::new(&ctx, dir_a.clone(), None), Tab::new(&ctx, dir_b.clone(), None)],
            active: 1,
            default_shell: None,
            last_window_rect: Some(egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1200.0, 700.0))),
            panel_layout: PanelLayout::default(),
            theme: ThemeMode::default(),
        };

        app.on_exit();

        let saved = syncshell_core::session::load_state();
        unsafe {
            std::env::remove_var("SYNCSHELL_HOME");
        }
        std::fs::remove_dir_all(&test_home).ok();

        assert_eq!(saved.tabs.len(), 2);
        assert_eq!(saved.tabs[0].path, dir_a);
        assert_eq!(saved.tabs[1].path, dir_b);
        assert_eq!(saved.active_tab, 1, "활성 탭 인덱스가 저장되지 않음");
        let window = saved.window.expect("창 크기/위치가 저장되지 않음");
        assert_eq!(window.width, 1200.0);
        assert_eq!(window.height, 700.0);
    }

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
