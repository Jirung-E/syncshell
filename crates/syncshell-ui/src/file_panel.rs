use eframe::egui;
use std::path::{Path, PathBuf};
use syncshell_core::fsview::{DirEntry, FsView};

/// 탐색기 패널에서 사용자가 고른 동작. 패널 렌더링 함수는 이걸 돌려주기만 하고
/// 실제 수행은 호출부(`SyncShellApp`)가 한다 — 터미널 주입이나 동기화 상태 갱신은
/// 패널이 모르는 자원을 건드려야 해서, 여기서 직접 하면 빌림 검사에 걸린다.
#[derive(Debug, Clone, PartialEq)]
pub enum FileAction {
    /// 폴더로 이동(동기화 대상 — 터미널에도 cd가 주입된다)
    Navigate(PathBuf),
    /// 연결 프로그램으로 열기
    OpenFile(PathBuf),
    /// 이 폴더를 터미널의 현재 위치로 만든다(탐색기는 안 옮김)
    OpenTerminalHere(PathBuf),
    CopyPath(PathBuf),
    /// 이름 바꾸기 대화상자를 연다(아직 아무것도 안 바꿈).
    BeginRename(PathBuf),
    /// 이름 바꾸기 확정 — (원래 경로, 새 이름)
    Rename(PathBuf, String),
    MoveToTrash(PathBuf),
    NewFolder,
    Refresh,
    /// 클립보드에 담기(복사) — 아직 아무것도 옮기지 않는다.
    Copy(PathBuf),
    /// 클립보드에 담기(잘라내기) — 아직 아무것도 옮기지 않는다.
    Cut(PathBuf),
    /// 클립보드에 담긴 걸 현재 폴더에 붙여넣는다.
    Paste,
}

/// 잘라내기/복사로 클립보드에 담긴 항목(DEV-008).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardEntry {
    pub path: PathBuf,
    /// true면 잘라내기(붙여넣기 = 이동), false면 복사(붙여넣기 = 복사).
    pub cut: bool,
}

/// 이름 바꾸기 대화상자가 떠 있는 동안의 상태.
pub struct RenameState {
    pub target: PathBuf,
    pub new_name: String,
    /// 대화상자가 뜬 첫 프레임에 텍스트 칸으로 포커스를 옮기고 전체 선택하기 위한 플래그.
    focus_pending: bool,
}

/// 패널이 프레임 간에 들고 있어야 하는 상태.
pub struct FilePanelState {
    pub rename: Option<RenameState>,
    /// 조작 결과 메시지(주로 오류). 다음 조작 때까지 패널 위에 남는다.
    pub status: Option<String>,
    pub status_is_error: bool,
    /// 복사/잘라내기로 담아둔 항목(DEV-008). 붙여넣기 전까지 유지된다.
    pub clipboard: Option<ClipboardEntry>,
    /// 클릭으로 "선택"된 파일(폴더는 클릭하면 바로 이동하므로 선택 대상이 아니다
    /// — DEV-015 결정 유지). Cmd+C/Ctrl+Shift+C 단축키가 뭘 복사할지 알려면
    /// 필요해서 추가함(실사용 피드백: "복사/붙여넣기 작동 안 함" → 우클릭
    /// 메뉴는 되지만 단축키가 없어서 그렇게 느꼈을 가능성이 컸음).
    pub selected: Option<PathBuf>,
    /// 좌측 디렉터리 트리에서 펼쳐진 폴더들.
    pub tree_expanded: std::collections::HashSet<PathBuf>,
    /// 트리에서 펼쳐본 폴더의 하위 폴더 목록. 배경 스레드에서 읽어오는 동안은
    /// `Loading`, 다 읽으면 `Loaded`로 바뀐다 — 시스템 임시 폴더처럼 항목이
    /// 아주 많은 폴더를 펼쳤을 때 UI 스레드가 멈추는 걸 실측으로 확인해서
    /// (5000행 렌더링 성능 테스트가 178ms까지 튀는 걸로 드러남) 비동기로 바꿨다.
    tree_children: std::collections::HashMap<PathBuf, TreeChildren>,
    /// 배경 스레드의 트리 하위 목록 읽기 결과가 도착하는 채널.
    tree_rx: std::sync::mpsc::Receiver<(PathBuf, Vec<PathBuf>)>,
    tree_tx: std::sync::mpsc::Sender<(PathBuf, Vec<PathBuf>)>,
    /// 마지막으로 현재 경로까지 조상 체인을 자동으로 펼쳐준 경로. 탐색기가 새
    /// 폴더로 이동할 때마다(터미널 cd 등으로도) 그 경로까지 트리가 자동으로
    /// 펼쳐지게 하되, 매 프레임 반복하면 사용자가 손으로 접어둔 것까지 되살아나
    /// 버리므로 "이 경로에 대해서는 이미 한 번 해줬다"를 기억해둔다.
    tree_synced_for: Option<PathBuf>,
    /// 우측 내용 영역을 목록(이름·크기·수정일)으로 보여줄지, 아이콘 격자로
    /// 보여줄지(실사용 요청: "아이콘뷰가 필요함").
    pub view_mode: ViewMode,
}

/// [`FilePanelState::view_mode`] 참고.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Icons,
}

/// [`FilePanelState::tree_children`] 참고.
enum TreeChildren {
    Loading,
    Loaded(Vec<PathBuf>),
}

impl Default for FilePanelState {
    fn default() -> Self {
        let (tree_tx, tree_rx) = std::sync::mpsc::channel();
        Self {
            rename: None,
            status: None,
            status_is_error: false,
            clipboard: None,
            selected: None,
            tree_expanded: std::collections::HashSet::new(),
            tree_children: std::collections::HashMap::new(),
            tree_rx,
            tree_tx,
            tree_synced_for: None,
            view_mode: ViewMode::List,
        }
    }
}

impl FilePanelState {
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_is_error = true;
    }

    pub fn set_info(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_is_error = false;
    }

    pub fn begin_rename(&mut self, target: &Path) {
        let current = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.rename = Some(RenameState {
            target: target.to_path_buf(),
            new_name: current,
            focus_pending: true,
        });
    }
}

/// 탐색기 패널 전체를 그리고, 사용자가 고른 동작이 있으면 돌려준다.
///
/// `panel_active`는 Cmd+C/Ctrl+Shift+C(복사)·Cmd+V/Ctrl+Shift+V(붙여넣기)
/// 단축키를 지금 처리해도 되는지를 나타낸다. 터미널 위젯이 매 프레임 무조건
/// 키보드 포커스를 가져가서(architecture 규칙 8) egui의 진짜 포커스로는 "지금
/// 탐색기를 쓰고 있는지"를 판단할 수 없다 — 그래서 호출부(`SyncShellApp`)가
/// "마지막으로 클릭된 패널이 어디였는지"를 직접 추적해서 넘겨준다. 이게 없으면
/// Windows/Linux에서 터미널이 이미 쓰는 Ctrl+Shift+C(선택 없어도 항상 복사,
/// 리눅스 터미널 관례)와 겹쳐서 터미널에 포커스가 있을 때도 파일이 복사돼버릴
/// 수 있다.
pub fn show(ui: &mut egui::Ui, fs: &FsView, state: &mut FilePanelState, panel_active: bool) -> Option<FileAction> {
    let mut action: Option<FileAction> = None;
    drain_tree_loads(state);

    if panel_active {
        // macOS는 Cmd+C/Cmd+V — 터미널은 Ctrl+C만 쓰므로(Cmd와는 다른 보조키라
        // 애초에 안 겹침) 그대로 써도 된다. Windows/Linux는 Ctrl+Shift+C/V —
        // 터미널이 이미 평범한 Ctrl+C(인터럽트/선택 복사)를 쓰고 있어서, 일반
        // Ctrl+C를 또 쓰면 어느 쪽이 반응해야 하는지 애매해진다.
        #[cfg(target_os = "macos")]
        let (copy_mods, paste_mods) = (egui::Modifiers::COMMAND, egui::Modifiers::COMMAND);
        #[cfg(not(target_os = "macos"))]
        let (copy_mods, paste_mods) =
            (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);

        if ui.ctx().input_mut(|i| i.consume_key(copy_mods, egui::Key::C)) {
            if let Some(selected) = &state.selected {
                action = Some(FileAction::Copy(selected.clone()));
            }
        }
        if ui.ctx().input_mut(|i| i.consume_key(paste_mods, egui::Key::V)) {
            action = Some(FileAction::Paste);
        }
    }

    ui.horizontal(|ui| {
        ui.heading("탐색기");
        // 목록/아이콘 전환 — 실사용 요청: "아이콘뷰가 필요함". 아이콘 텍스트
        // ("⊞"/"☰" 같은) 대신 한글 라벨을 쓴다 — 예전에 "✕"(U+2715)가 일반
        // 폰트에 없어 두부로 나온 적이 있어서(file_panel 상태 메시지 닫기
        // 버튼), 검증 안 된 기호 글리프를 새로 들이는 것보다 이미 커버가
        // 확인된 한글 쪽이 안전하다.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.selectable_label(state.view_mode == ViewMode::Icons, "아이콘").clicked() {
                state.view_mode = ViewMode::Icons;
            }
            if ui.selectable_label(state.view_mode == ViewMode::List, "목록").clicked() {
                state.view_mode = ViewMode::List;
            }
        });
    });
    // 현재 경로를 조상 폴더별로 쪼갠 클릭 가능한 세그먼트(브레드크럼)로 보여준다
    // — 경로 직접 입력은 스코프에서 뺐다(터미널에서 cd 치면 어차피 동기화되므로
    // 커맨드라인이 그 역할을 대신함, 2026-08-06 결정). 우클릭 = 폴더 배경 메뉴
    // (새 폴더/새로고침/경로 복사). 목록이 가상 스크롤(show_rows)로 바뀌면서
    // "목록의 빈 공간 우클릭"은 더 이상 안전하게 구현할 수 없어(가상 스크롤
    // 영역은 빈 공간이 따로 없음) 여기로 옮겼다 — 많은 탐색기가 경로 표시줄
    // 우클릭에 폴더 메뉴를 두는 것과 같은 자리.
    breadcrumb(ui, &fs.current_dir, &mut action, state.clipboard.is_some());

    if let Some(msg) = state.status.clone() {
        let color = if state.status_is_error {
            egui::Color32::from_rgb(0xe0, 0x6c, 0x75)
        } else {
            egui::Color32::from_rgb(0x98, 0xc3, 0x79)
        };
        let mut dismissed = false;
        ui.horizontal(|ui| {
            ui.colored_label(color, msg);
            // "✕"(U+2715, Dingbats)이 아니라 "×"(U+00D7, 곱셈 기호)를 쓴다 —
            // Apple SD Gothic Neo를 포함해 대부분의 일반 폰트가 U+2715는 안
            // 갖고 있어서 두부로 보이는 실사용 버그가 있었다(fontdb로 실측 확인).
            // U+00D7는 기본 라틴 확장 범위라 사실상 모든 폰트가 갖고 있다.
            if ui.small_button("×").clicked() {
                dismissed = true;
            }
        });
        if dismissed {
            state.status = None;
        }
    }
    ui.separator();

    // 탐색기가 새 폴더로 옮겨갈 때마다(사용자 클릭이든 터미널 cd 동기화든)
    // 좌측 트리가 그 경로까지 자동으로 펼쳐지게 한다 — 매 프레임 반복하지
    // 않도록 이미 해준 경로는 건너뛴다(사용자가 손으로 접어둔 게 되살아나지
    // 않게).
    ensure_tree_expanded_to_current(state, &fs.current_dir);

    // `ui.horizontal()`은 자기 내부 Ui의 높이를 부모의 남은 높이 전체가 아니라
    // 한 줄 높이(`interact_size.y`)로 잡는다(`ui.vertical()`과 다른 부분 —
    // egui 소스의 `horizontal_centered` 문서에 "Like horizontal, but allocates
    // the full vertical height"라고 명시돼 있어, 반대로 읽으면 평범한
    // `horizontal`은 그렇지 않다는 뜻). 그 안에서 그냥 `ui.available_height()`를
    // 다시 물어보면 이 좁은 한 줄 높이가 나와서, 트리·파일 목록 스크롤 영역이
    // 실제로는 몇 줄짜리 좁은 띠 안에 갇혀버린다(실측: 클릭 테스트에서 트리
    // 두 번째 행이 clip 영역 밖으로 밀려나 클릭이 전혀 안 먹힘 — 5000행 성능
    // 테스트가 "통과"한 것도 사실은 같은 이유로 거의 안 그려져서였다). 그래서
    // 진짜 남은 높이를 `ui.horizontal()` 밖에서 미리 구해서 넘겨준다.
    let available_height = ui.available_height();

    ui.horizontal(|ui| {
        let tree_width = 160.0;
        ui.allocate_ui_with_layout(
            egui::vec2(tree_width, available_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("dir_tree_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let root = tree_root(&fs.current_dir);
                        tree_node(ui, &root, &fs.current_dir, state, &mut action, 0);
                    });
            },
        );

        ui.separator();

        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), available_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
            // ".."과 오류 표시는 목록(entries) 밖 고정 영역에 둔다 — 아래 스크롤 영역이
            // `show_rows`로 가상 스크롤되므로, 스크롤 대상이 아닌 것들은 여기서 미리 그린다.
            if let Some(parent) = fs.current_dir.parent() {
                if ui.selectable_label(false, "..").clicked() {
                    action = Some(FileAction::Navigate(parent.to_path_buf()));
                }
            }
            if let Some(err) = &fs.error {
                ui.colored_label(egui::Color32::from_rgb(0xe0, 0x6c, 0x75), err);
            }

            match state.view_mode {
                ViewMode::List => list_view(ui, fs, state, &mut action),
                ViewMode::Icons => icon_view(ui, fs, state, &mut action),
            }
            },
        );
    });

    if let Some(picked) = rename_dialog(ui.ctx(), state) {
        action = Some(picked);
    }

    action
}

/// 항목(파일/폴더) 하나의 클릭 반응 — 목록 뷰·아이콘 뷰가 모양만 다르고 클릭
/// 의미는 같아야 하므로 공유한다. 폴더는 단일 클릭 진입(TR-003/TR-005의 동기화
/// 설계 전제, DEV-015), 파일은 단일 클릭으로 "선택"만 하고 더블클릭으로 연다
/// (한 번만 클릭해도 실행파일이 열려버리던 문제, TR-006 피드백).
fn handle_entry_click(
    entry: &DirEntry,
    response: &egui::Response,
    state: &mut FilePanelState,
    action: &mut Option<FileAction>,
) {
    if entry.is_dir {
        if response.clicked() {
            state.selected = None; // 다른 폴더로 옮겨가니 옛 선택은 의미 없음
            *action = Some(FileAction::Navigate(entry.path.clone()));
        }
    } else if response.double_clicked() {
        *action = Some(FileAction::OpenFile(entry.path.clone()));
    } else if response.clicked() {
        state.selected = Some(entry.path.clone());
    }
}

/// 이름·크기·수정일이 나오는 목록 뷰(기존 기본 형태).
///
/// DEV-005 Test plan: "대용량 디렉터리(수천 개 파일) 가상 스크롤". 처음엔
/// ScrollArea에 전체 목록을 그냥 다 그렸는데, 5000개 항목에서 프레임당 200ms대
/// (60fps 예산 16.7ms의 10배 이상)까지 나오는 걸 실측하고서 `show_rows`(화면에
/// 보이는 행만 그리는 내장 가상 스크롤)로 바꿨다.
fn list_view(ui: &mut egui::Ui, fs: &FsView, state: &mut FilePanelState, action: &mut Option<FileAction>) {
    let row_height = ui.text_style_height(&egui::TextStyle::Button).max(20.0);
    let meta_width = 130.0;
    egui::ScrollArea::vertical()
        .id_salt("file_list_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, fs.entries.len(), |ui, row_range| {
            for entry in &fs.entries[row_range] {
                ui.horizontal(|ui| {
                    let name_width = (ui.available_width() - meta_width).max(20.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(name_width, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            // 아이콘은 DEV-011 메모와 같은 방침 — 셸 아이콘 추출 대신
                            // 확장자 기반 정적 아이콘으로 시작한다(SHGetFileInfo 등
                            // 플랫폼 API 직접 호출 금지, DEV-008 방침과도 일치).
                            let label = format!("{} {}", entry_icon(entry.is_dir, &entry.name), entry.name);
                            // 파일은 선택 상태를 하이라이트로 보여준다(Cmd+C 등 단축키가
                            // 뭘 대상으로 할지 눈에 보여야 함) — 폴더는 클릭하면 바로
                            // 이동해버려서 "선택" 개념 자체가 없다(항상 false).
                            let selected = !entry.is_dir && state.selected.as_deref() == Some(entry.path.as_path());
                            // .truncate() 필수 — 안 그러면 이름이 name_width보다 길 때
                            // 잘리지 않고 그대로 그려져서 옆 크기/수정일 컬럼과 겹친다
                            // (실사용 버그 리포트: "문서 제목과 수정 날짜가 겹쳐서 출력").
                            let response = ui.add(egui::Button::selectable(selected, label).truncate());
                            #[cfg(test)]
                            tests::record_row_rect_for_test(&entry.path, response.rect);

                            handle_entry_click(entry, &response, state, action);

                            response.context_menu(|ui| {
                                if let Some(picked) = entry_menu(ui, &entry.path, entry.is_dir) {
                                    *action = Some(picked);
                                    ui.close();
                                }
                            });
                        },
                    );

                    ui.allocate_ui_with_layout(
                        egui::vec2(meta_width, row_height),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.weak(if entry.is_dir { String::new() } else { format_size(entry.size) });
                            ui.weak(entry.modified.map(format_modified).unwrap_or_default());
                        },
                    );
                });
            }
        });
}

/// 아이콘 격자 뷰(실사용 요청: "아이콘뷰가 필요함"). 목록 뷰와 같은 가상
/// 스크롤 원칙(DEV-005 Test plan: 대용량 폴더에서 안 끊김)을 격자에 맞게
/// 적용한다 — `show_rows`는 "한 줄"(row) 단위로만 가상화하므로, 한 "줄"에
/// 여러 칸(항목)을 채워 격자처럼 보이게 한다(줄 수 = 항목 수를 열 수로 나눠
/// 올림).
fn icon_view(ui: &mut egui::Ui, fs: &FsView, state: &mut FilePanelState, action: &mut Option<FileAction>) {
    const CELL_SIZE: egui::Vec2 = egui::vec2(84.0, 84.0);
    let cols = ((ui.available_width() / CELL_SIZE.x).floor() as usize).max(1);
    let row_count = fs.entries.len().div_ceil(cols);

    egui::ScrollArea::vertical()
        .id_salt("file_icon_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, CELL_SIZE.y, row_count, |ui, row_range| {
            for row in row_range {
                ui.horizontal(|ui| {
                    for col in 0..cols {
                        let Some(entry) = fs.entries.get(row * cols + col) else { break };
                        icon_cell(ui, entry, state, action, CELL_SIZE);
                    }
                });
            }
        });
}

/// 아이콘 격자의 칸 하나 — 큰 아이콘 + 그 아래 이름(한 줄, 넘치면 말줄임).
/// `ui.put()`으로 절대 좌표에 라벨을 배치한다(아이콘 위·이름 아래로 세로
/// 배치하면서 가운데 정렬까지 하려면 `Button`의 기본 가로 레이아웃으로는
/// 어려워서, 칸 전체를 우리가 직접 클릭 영역으로 잡고 그 안에 라벨 두 개를
/// 원하는 위치에 얹는 방식을 썼다).
fn icon_cell(
    ui: &mut egui::Ui,
    entry: &DirEntry,
    state: &mut FilePanelState,
    action: &mut Option<FileAction>,
    size: egui::Vec2,
) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    #[cfg(test)]
    tests::record_row_rect_for_test(&entry.path, rect);

    if ui.is_rect_visible(rect) {
        let selected = !entry.is_dir && state.selected.as_deref() == Some(entry.path.as_path());
        if selected || response.hovered() {
            let visuals = ui.style().interact_selectable(&response, selected);
            ui.painter().rect_filled(rect, 4.0, visuals.bg_fill);
        }

        const NAME_HEIGHT: f32 = 26.0;
        let icon_rect = egui::Rect::from_min_size(rect.min, egui::vec2(size.x, size.y - NAME_HEIGHT));
        ui.put(
            icon_rect,
            egui::Label::new(egui::RichText::new(entry_icon(entry.is_dir, &entry.name)).size(28.0))
                .halign(egui::Align::Center)
                .selectable(false),
        );
        let name_rect =
            egui::Rect::from_min_size(egui::pos2(rect.min.x, rect.max.y - NAME_HEIGHT), egui::vec2(size.x, NAME_HEIGHT));
        ui.put(
            name_rect,
            egui::Label::new(egui::RichText::new(&entry.name).size(11.0))
                .halign(egui::Align::Center)
                .truncate()
                .selectable(false),
        );
    }

    handle_entry_click(entry, &response, state, action);

    response.context_menu(|ui| {
        if let Some(picked) = entry_menu(ui, &entry.path, entry.is_dir) {
            *action = Some(picked);
            ui.close();
        }
    });
}

/// 현재 경로를 조상 폴더 단위로 쪼개 클릭 가능한 세그먼트로 보여준다(홈 > Documents
/// > project 같은 형태). 경로 직접 입력(텍스트로 타이핑)은 스코프에서 뺐다(터미널의
/// `cd`가 이미 탐색기와 동기화되므로 커맨드라인이 그 역할을 대신함, 2026-08-06 결정).
/// 맨 끝(현재 폴더) 세그먼트는 눌러도 이동하지 않는다 — 이미 거기 있으므로. 배경(빈
/// 공간) 메뉴(새 폴더/새로고침/경로 복사)는 이 맨 끝 세그먼트를 우클릭하면 뜬다 —
/// 조상 세그먼트들과 구분자 사이의 빈틈은 위젯이 아니라서 거기 붙이면 클릭이
/// 안정적으로 안 잡힌다(가상 스크롤 도입 전엔 폭 전체를 쓰는 라벨 하나였어서
/// 문제 없었지만, 지금은 세그먼트별 버튼들의 모음이라 컨테이너 전체에 붙이면
/// 안쪽 버튼에 클릭이 먼저 먹혀 컨테이너까지 안 올라옴).
fn breadcrumb(ui: &mut egui::Ui, current_dir: &Path, action: &mut Option<FileAction>, has_clipboard: bool) {
    let ancestors: Vec<PathBuf> = current_dir.ancestors().map(|p| p.to_path_buf()).collect();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        // `ancestors`는 현재 폴더가 맨 앞, 루트가 맨 뒤 순서라 뒤집어서 루트부터 보여준다.
        for (idx, ancestor) in ancestors.iter().rev().enumerate() {
            if idx > 0 {
                ui.weak(">");
            }
            let is_current = idx == ancestors.len() - 1;
            let name = ancestor
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| ancestor.display().to_string());
            let response = ui.add(egui::Button::selectable(is_current, name).small());
            #[cfg(test)]
            tests::record_breadcrumb_segment_rect_for_test(ancestor, response.rect);
            if response.clicked() && !is_current {
                *action = Some(FileAction::Navigate(ancestor.clone()));
            }
            if is_current {
                response.context_menu(|ui| {
                    if let Some(picked) = background_menu(ui, current_dir, has_clipboard) {
                        *action = Some(picked);
                        ui.close();
                    }
                });
            }
        }
    });
}

/// 트리에서 현재 폴더 위로 자동으로 펼쳐 보여줄 조상 단계 수(자기 자신 포함).
/// [`tree_root`]와 [`ensure_tree_expanded_to_current`]가 같이 쓴다 — 반드시
/// 같은 값이어야 "뿌리부터 현재 폴더까지 전부 펼쳐져 보인다"가 성립한다.
const TREE_AUTO_EXPAND_LEVELS: usize = 4;

/// 트리의 뿌리 — 진짜 파일시스템 루트(`/`)가 아니라 현재 폴더에서 가까운
/// 조상([`TREE_AUTO_EXPAND_LEVELS`]단계 이내)이다. 처음엔 진짜 루트를 뿌리로
/// 삼고 현재 폴더까지 조상 체인을 전부 자동으로 펼쳤는데, macOS의
/// `/private/var/folders/.../T`(임시 파일 폴더) 같은 중간 조상이 항목 수천
/// 개짜리라 배경 로딩이 오래 걸리고, 로딩이 끝날 때마다 트리 위쪽에 새 행이
/// 끼어들어 아래쪽(현재 폴더 근처) 행들의 위치가 계속 흔들리는 게 실측으로
/// 드러났다(트리 노드 클릭 테스트가 위치를 알아낸 직후 클릭하는 그 짧은 사이에도
/// 위치가 바뀌어 클릭이 빗나갔음). 더 위쪽 조상(진짜 루트까지)은 브레드크럼으로
/// 여전히 갈 수 있다 — 트리는 "지금 있는 곳 근처"를 보여주는 용도로 좁혔다.
fn tree_root(current_dir: &Path) -> PathBuf {
    current_dir
        .ancestors()
        .take(TREE_AUTO_EXPAND_LEVELS)
        .last()
        .unwrap_or(current_dir)
        .to_path_buf()
}

/// 배경 스레드에서 읽어온 트리 하위 목록 결과를 받아 반영한다. `show()` 시작
/// 부분에서 매 프레임 호출한다 — 아직 아무것도 안 왔으면 그냥 넘어간다.
fn drain_tree_loads(state: &mut FilePanelState) {
    while let Ok((path, children)) = state.tree_rx.try_recv() {
        state.tree_children.insert(path, TreeChildren::Loaded(children));
    }
}

/// 어떤 폴더의 "폴더만" 목록 읽기를 배경 스레드에서 시작한다(이미 읽는 중이거나
/// 다 읽었으면 아무것도 안 함 — 여러 프레임에 걸쳐 반복 호출해도 안전). UI
/// 스레드는 절대 디스크를 읽지 않는다 — 시스템 임시 폴더처럼 항목이 아주 많은
/// 폴더를 펼쳤을 때 프레임이 멈추는 걸 실측으로 확인했다(5000행 렌더링 성능
/// 테스트가 178ms까지 튀는 걸로 드러남). 심볼릭 링크는 대상을 따라가서 판단한다
/// (`fsview::read_dir_sorted`와 같은 방침 — 링크 폴더가 파일로 잘못 분류돼 트리에서
/// 열 수 없게 되는 걸 막음, TR-006 회귀).
fn spawn_tree_load(ctx: &egui::Context, state: &mut FilePanelState, path: &Path) {
    if state.tree_children.contains_key(path) {
        return;
    }
    state.tree_children.insert(path.to_path_buf(), TreeChildren::Loading);
    let tx = state.tree_tx.clone();
    let ctx = ctx.clone();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let mut children: Vec<PathBuf> = std::fs::read_dir(&path)
            .map(|read| {
                read.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.metadata().map(|m| m.is_dir()).unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default();
        children.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        // 실제 항목이 수천 개인 폴더(공유 임시 폴더, 잔뜩 쌓인 다운로드 폴더 등)를
        // 자동으로 펼쳤을 때 트리 한 단계에 수천 행이 통째로 쏟아지는 걸 막는다 —
        // 파일 목록(`show_rows`)과 달리 트리는 아직 가상 스크롤이 아니라서
        // (중첩 구조라 더 복잡함) 상한이 없으면 그만큼 다 그린다.
        const TREE_MAX_CHILDREN: usize = 500;
        children.truncate(TREE_MAX_CHILDREN);
        let _ = tx.send((path, children));
        ctx.request_repaint();
    });
}

/// 탐색기가 새 경로로 옮겨갈 때 그 경로까지의 조상 체인을 트리에서 자동으로
/// 펼쳐준다 — 그래야 지금 보고 있는 폴더가 트리 안에서 어디인지 바로 보인다.
/// 같은 경로에 대해 매 프레임 반복하지 않는다(사용자가 손으로 접어둔 다른
/// 폴더까지 되살리지 않기 위해).
fn ensure_tree_expanded_to_current(state: &mut FilePanelState, current_dir: &Path) {
    if state.tree_synced_for.as_deref() == Some(current_dir) {
        return;
    }
    // [`tree_root`]와 같은 단계 수만큼만 편다 — 왜 전부가 아니라 일부만인지는
    // 그쪽 문서 참고.
    for ancestor in current_dir.ancestors().take(TREE_AUTO_EXPAND_LEVELS) {
        state.tree_expanded.insert(ancestor.to_path_buf());
    }
    state.tree_synced_for = Some(current_dir.to_path_buf());
}

/// 트리 한 줄(펼침/접힘 화살표 + 폴더 이름) — 화살표는 펼침 상태만 토글하고,
/// 이름을 누르면 그 폴더로 이동한다(우측 리스트와 동일한 폴더=단일 클릭 진입
/// 규칙, DEV-015). 펼쳐진 폴더는 재귀적으로 자기 하위 폴더를 그린다.
fn tree_node(
    ui: &mut egui::Ui,
    path: &Path,
    current_dir: &Path,
    state: &mut FilePanelState,
    action: &mut Option<FileAction>,
    depth: usize,
) {
    let expanded = state.tree_expanded.contains(path);
    let is_current = path == current_dir;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 14.0);
        let arrow = if expanded { "▼" } else { "▶" };
        if ui.add(egui::Button::new(arrow).small().frame(false)).clicked() {
            if expanded {
                state.tree_expanded.remove(path);
            } else {
                state.tree_expanded.insert(path.to_path_buf());
                spawn_tree_load(ui.ctx(), state, path);
            }
        }
        let label = format!("📁 {name}");
        let response = ui.add(egui::Button::selectable(is_current, label).truncate());
        #[cfg(test)]
        tests::record_tree_node_rect_for_test(path, response.rect);
        if response.clicked() {
            *action = Some(FileAction::Navigate(path.to_path_buf()));
        }
    });

    if expanded {
        spawn_tree_load(ui.ctx(), state, path);
        match state.tree_children.get(path) {
            Some(TreeChildren::Loaded(children)) => {
                let children = children.clone();
                for child in &children {
                    tree_node(ui, child, current_dir, state, action, depth + 1);
                }
            }
            Some(TreeChildren::Loading) | None => {
                ui.horizontal(|ui| {
                    ui.add_space((depth + 1) as f32 * 14.0);
                    ui.weak("불러오는 중…");
                });
            }
        }
    }
}

/// 확장자 기반 정적 아이콘(DEV-011/DEV-008 방침 — 셸 아이콘 추출 같은 플랫폼
/// API를 직접 부르지 않는다). 목록에서 자주 보는 확장자 몇 개만 구분하고,
/// 나머지는 범용 파일 아이콘으로 묶는다 — 확장자별 아이콘 테이블을 무한정
/// 늘리는 건 관리 비용 대비 얻는 게 적다.
fn entry_icon(is_dir: bool, name: &str) -> &'static str {
    if is_dir {
        return "📁";
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg") => "🖼️",
        Some("zip" | "tar" | "gz" | "7z" | "rar" | "xz") => "🗜️",
        Some("mp3" | "wav" | "flac" | "aac" | "m4a") => "🎵",
        Some("mp4" | "mov" | "mkv" | "avi" | "webm") => "🎬",
        Some("pdf") => "📕",
        Some("md" | "txt") => "📄",
        Some("rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "java" | "sh") => "💻",
        _ => "📄",
    }
}

/// 바이트 수를 사람이 읽는 크기 문자열로. 1024 기준(KiB/MiB/...)이지만 표기는
/// 익숙한 KB/MB로 — 탐색기 사용자에게 "KiB"는 불필요한 낯섦이다.
fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// 수정 시각을 로컬 시간 "YYYY-MM-DD HH:MM"로. 못 읽거나(권한 등) 시계가 UNIX
/// 이전으로 나오는 등 이상값이면 빈 문자열 — 컬럼이 깨진 문자열로 지저분해지는
/// 것보다 비어 보이는 쪽이 낫다.
fn format_modified(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// 항목(폴더/파일) 우클릭 메뉴.
fn entry_menu(ui: &mut egui::Ui, path: &Path, is_dir: bool) -> Option<FileAction> {
    let mut action = None;
    ui.set_min_width(180.0);

    if is_dir {
        if ui.button("열기").clicked() {
            action = Some(FileAction::Navigate(path.to_path_buf()));
        }
        if ui.button("여기서 터미널 열기").clicked() {
            action = Some(FileAction::OpenTerminalHere(path.to_path_buf()));
        }
    } else if ui.button("열기").clicked() {
        action = Some(FileAction::OpenFile(path.to_path_buf()));
    }

    ui.separator();
    if ui.button("복사").clicked() {
        action = Some(FileAction::Copy(path.to_path_buf()));
    }
    if ui.button("잘라내기").clicked() {
        action = Some(FileAction::Cut(path.to_path_buf()));
    }

    ui.separator();
    if ui.button("이름 바꾸기").clicked() {
        // 실제 이름 변경은 대화상자를 거친다 — 여기서는 열기만 알린다.
        action = Some(FileAction::BeginRename(path.to_path_buf()));
    }
    if ui.button("삭제 (휴지통)").clicked() {
        action = Some(FileAction::MoveToTrash(path.to_path_buf()));
    }

    ui.separator();
    if ui.button("경로 복사").clicked() {
        action = Some(FileAction::CopyPath(path.to_path_buf()));
    }

    action
}

/// 빈 공간 우클릭 메뉴(현재 폴더 대상). `has_clipboard`가 false면 "붙여넣기"는
/// 눌러도 반응 없는 상태로 보여준다(비활성화) — 일반 탐색기와 같은 감각.
fn background_menu(ui: &mut egui::Ui, current_dir: &Path, has_clipboard: bool) -> Option<FileAction> {
    let mut action = None;
    ui.set_min_width(180.0);

    if ui.button("새 폴더").clicked() {
        action = Some(FileAction::NewFolder);
    }
    if ui.add_enabled(has_clipboard, egui::Button::new("붙여넣기")).clicked() {
        action = Some(FileAction::Paste);
    }
    ui.separator();
    if ui.button("새로 고침").clicked() {
        action = Some(FileAction::Refresh);
    }
    if ui.button("현재 경로 복사").clicked() {
        action = Some(FileAction::CopyPath(current_dir.to_path_buf()));
    }

    action
}

/// 이름 바꾸기 대화상자. 확정되면 `FileAction::Rename`을 돌려준다.
fn rename_dialog(ctx: &egui::Context, state: &mut FilePanelState) -> Option<FileAction> {
    let Some(rename) = &mut state.rename else {
        return None;
    };

    let mut result = None;
    let mut close = false;

    let modal = egui::Modal::new(egui::Id::new("rename_dialog")).show(ctx, |ui| {
        ui.set_min_width(320.0);
        ui.heading("이름 바꾸기");
        ui.add_space(4.0);

        let edit = ui.add(
            egui::TextEdit::singleline(&mut rename.new_name)
                .desired_width(f32::INFINITY)
                .hint_text("새 이름"),
        );
        if rename.focus_pending {
            edit.request_focus();
            rename.focus_pending = false;
        }

        // Enter로 확정 — 대화상자에서 기대되는 기본 동작.
        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("확인").clicked() || submitted {
                result = Some(FileAction::Rename(
                    rename.target.clone(),
                    rename.new_name.clone(),
                ));
                close = true;
            }
            if ui.button("취소").clicked() {
                close = true;
            }
        });
    });

    // 바깥 클릭이나 Esc로도 닫힌다(egui Modal이 판단해 준다).
    if modal.should_close() {
        close = true;
    }
    if close {
        state.rename = None;
    }
    result
}

/// DEV-005: 크기/아이콘/수정일 표시 포맷 함수 테스트. UI 렌더링과 분리된
/// 순수 함수라 egui 컨텍스트 없이 직접 검증할 수 있다.
#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn format_size_uses_appropriate_unit() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn format_modified_renders_local_date_and_time() {
        // 2024-01-15 12:00:00 UTC — 표시 자체는 로컬 시간대로 바뀌므로 형식만 확인한다
        // (연-월-일 시:분, 4자리 연도). 정확한 시:분까지 어설션하면 CI 시간대에 따라
        // 깨지므로 형식만 본다.
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_705_320_000);
        let rendered = format_modified(t);
        assert_eq!(rendered.len(), "YYYY-MM-DD HH:MM".len(), "형식이 예상과 다름: {rendered:?}");
        assert!(rendered.chars().next().unwrap().is_ascii_digit());
    }

    #[test]
    fn entry_icon_distinguishes_directories_and_known_extensions() {
        assert_eq!(entry_icon(true, "anything"), "📁");
        assert_eq!(entry_icon(false, "photo.PNG"), "🖼️", "대소문자 구분 없이 확장자를 봐야 함");
        assert_eq!(entry_icon(false, "archive.zip"), "🗜️");
        assert_eq!(entry_icon(false, "main.rs"), "💻");
        assert_eq!(entry_icon(false, "no_extension"), "📄");
    }

    /// DEV-005 Test plan: "C:\Windows\System32 같은 대용량 폴더에서 스크롤이
    /// 끊기지 않는지". 실제 디스크 I/O 없이(느리고 CI에서 들쭉날쭉해짐) `entries`를
    /// 직접 5000개로 채워서, 목록 전체를 매 프레임 그대로 그리는(가상 스크롤 없음)
    /// 지금 구현의 프레임당 레이아웃 비용을 잰다. 엄격한 60fps 기준(16.7ms)이
    /// 아니라 넉넉한 임계값으로 "심각한 회귀"만 잡는다 — 정확한 수치는 머신마다
    /// 다르고, 목적은 정밀 벤치가 아니라 향후 컬럼을 추가하다 실수로 프레임을
    /// 몇십 ms대로 떨어뜨리는 걸 잡아내는 것.
    #[test]
    fn rendering_5000_entries_stays_within_frame_budget() {
        let mut fs = FsView::new(std::env::temp_dir(), || {});
        fs.entries = (0..5000)
            .map(|i| syncshell_core::fsview::DirEntry {
                name: format!("file_{i:05}.txt"),
                path: std::env::temp_dir().join(format!("file_{i:05}.txt")),
                is_dir: false,
                size: 1234,
                modified: Some(std::time::SystemTime::now()),
            })
            .collect();

        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 800.0)));

        // 첫 프레임(폰트 아틀라스 워밍업 등 일회성 비용)은 재지 않는다.
        let _ = ctx.run_ui(input.clone(), |ui| {
            show(ui, &fs, &mut state, true);
        });

        let mut worst = std::time::Duration::ZERO;
        for _ in 0..10 {
            let start = std::time::Instant::now();
            let _ = ctx.run_ui(input.clone(), |ui| {
                show(ui, &fs, &mut state, true);
            });
            worst = worst.max(start.elapsed());
        }

        println!("5000행 렌더링 최대 프레임 시간: {worst:?}");
        // 가상 스크롤(show_rows) 도입 전에는 5000개 항목에서 200ms대(60fps 예산의
        // 10배 이상)가 나왔다. 도입 후 실측 ~2ms — 여유를 두되(느린 CI 머신 감안)
        // "전체 목록을 다시 그리는" 회귀로 되돌아가면 반드시 걸리도록 30ms로 잡는다.
        assert!(
            worst < std::time::Duration::from_millis(30),
            "5000개 항목 렌더링이 비정상적으로 느림({worst:?}) — 가상 스크롤이 깨졌을 수 있음"
        );
    }

    /// 실사용 버그 리포트("탐색기에서 문서 제목과 수정 날짜가 겹쳐서 출력됨") 회귀
    /// 테스트: 이름 컬럼에 할당된 `name_width`보다 긴 파일명이 잘리지 않고 그대로
    /// 그려지면, 그 옆에 오른쪽 정렬로 그리는 크기/수정일 컬럼과 겹친다.
    /// `Button::truncate()`가 빠져 있던 게 원인이었다 — 이제는 이름 컬럼 폭을
    /// 넘어서지 않아야 한다. 실제 행이 쓰는 것과 같은 위젯 구성
    /// (`allocate_ui_with_layout` + `entry_icon` + `Button::selectable().truncate()`)
    /// 을 그대로 재현해서 확인한다.
    #[test]
    fn long_filename_is_truncated_within_name_column_width() {
        let ctx = egui::Context::default();
        let name_width = 150.0f32;
        let row_height = 20.0f32;
        let long_name = "이_파일은_이름이_아주아주아주아주아주아주아주아주아주아주_길다.txt";

        let mut response_rect = egui::Rect::NOTHING;
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 100.0)));
        let _ = ctx.run_ui(input, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(name_width, row_height),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let label = format!("{} {}", entry_icon(false, long_name), long_name);
                    response_rect = ui.add(egui::Button::selectable(false, label).truncate()).rect;
                },
            );
        });

        assert!(
            response_rect.width() <= name_width + 1.0, // 부동소수점 오차 약간 허용
            "긴 파일명이 이름 컬럼 폭({name_width})을 넘어감: {response_rect:?} — 옆 컬럼(크기/수정일)과 겹칠 수 있음"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir_with(tag: &str, names: &[&str]) -> PathBuf {
        let base = std::env::temp_dir().join(format!("syncshell-panel-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&base).unwrap();
        for n in names {
            if n.ends_with('/') {
                std::fs::create_dir_all(base.join(n.trim_end_matches('/'))).unwrap();
            } else {
                std::fs::write(base.join(n), b"x").unwrap();
            }
        }
        base
    }

    /// 트리 관련 테스트 전용 — 일반 `temp_dir_with()`와 달리 우리가 통제하는
    /// 폴더를 몇 단계 더 파고 들어가서 테스트 폴더를 만든다. 트리는 현재 폴더에서
    /// 가까운 조상 몇 단계([`TREE_AUTO_EXPAND_LEVELS`])까지 자동으로 펼치는데,
    /// OS 공유 임시 폴더 바로 밑에 만들면 그 조상 중 하나가 우리와 무관한 항목이
    /// 수천 개인 공유 폴더가 돼버려서(실측: macOS에서 4879개) 트리가 감당 못 할
    /// 만큼 부풀고 테스트 화면(600px) 밖으로 밀려나 클릭 위치를 못 찾는다 —
    /// 우리만 쓰는 폴더를 몇 단계 파서 그 안에 두면 트리 뿌리가 항상 우리가
    /// 통제하는(항목이 몇 개 안 되는) 폴더가 된다.
    fn isolated_tree_test_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir()
            .join("syncshell-tree-test-root")
            .join("a")
            .join("b")
            .join(format!("{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn settled_view(dir: &Path) -> FsView {
        let mut fs = FsView::new(dir.to_path_buf(), || {});
        for _ in 0..100 {
            fs.pump();
            if !fs.entries.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        fs
    }

    fn frame(ctx: &egui::Context, fs: &FsView, state: &mut FilePanelState) -> Option<FileAction> {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
        let mut action = None;
        let _ = ctx.run_ui(input, |ui| {
            action = show(ui, fs, state, true);
        });
        action
    }

    /// 이름 바꾸기 대화상자가 뜬 상태에서 "확인"을 누르면 원래 경로와 입력한
    /// 새 이름이 담긴 Rename 액션이 나와야 한다 — 실제 프로덕션 위젯 코드로 확인.
    #[test]
    fn rename_dialog_confirm_yields_rename_action() {
        let base = temp_dir_with("rename-dialog", &["target.txt"]);
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();

        let target = base.join("target.txt");
        state.begin_rename(&target);
        // 대화상자를 띄운다(첫 프레임에 텍스트 칸이 포커스를 받는다).
        frame(&ctx, &fs, &mut state);
        state.rename.as_mut().unwrap().new_name = "renamed.txt".to_string();

        // Enter로 확정 — 대화상자에서 사용자가 실제로 가장 많이 쓰는 경로다.
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
        input.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut action = None;
        let _ = ctx.run_ui(input, |ui| {
            action = show(ui, &fs, &mut state, true);
        });

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(
            action,
            Some(FileAction::Rename(target, "renamed.txt".to_string())),
            "Enter로 확정했는데 Rename 액션이 안 나옴"
        );
    }

    /// 취소하거나 Esc로 닫으면 아무 액션도 안 나오고 상태가 정리돼야 한다.
    #[test]
    fn rename_dialog_closes_without_action_on_escape() {
        let base = temp_dir_with("rename-esc", &["a.txt"]);
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();

        state.begin_rename(&base.join("a.txt"));
        frame(&ctx, &fs, &mut state);
        assert!(state.rename.is_some(), "대화상자가 안 열림");

        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut action = None;
        let _ = ctx.run_ui(input, |ui| {
            action = show(ui, &fs, &mut state, true);
        });

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(action, None, "Esc로 닫았는데 액션이 나옴");
        assert!(state.rename.is_none(), "Esc로 닫았는데 상태가 안 정리됨");
    }

    thread_local! {
        /// 실제 `show()`가 파일 행 버튼을 그릴 때 그 `Response::rect`를 여기 담아둔다
        /// (아래 `record_row_rect_for_test`를 통해, 프로덕션 코드의 `#[cfg(test)]`
        /// 훅에서 호출됨). 헤더·브레드크럼(경로가 길면 여러 줄로 접힘)·좌측 트리
        /// 컬럼까지 다 감안한 "진짜 좌표"를 직접 계산해서 재현하려던 이전 방식은
        /// 레이아웃이 바뀔 때마다(트리 추가 등) 깨졌다 — 대신 실제 위젯이 어디
        /// 그려졌는지를 그대로 물어보는 쪽이 더 튼튼하다.
        static FIRST_ROW_RECT: std::cell::Cell<Option<egui::Rect>> = const { std::cell::Cell::new(None) };
    }

    /// `show()` 안의 파일 행 렌더링 지점에서만 호출된다(`#[cfg(test)]`). 이미
    /// 하나 기록해뒀으면 덮어쓰지 않는다 — 여러 항목 중 "첫 번째" 행의 위치만
    /// 필요하기 때문.
    pub(super) fn record_row_rect_for_test(_path: &Path, rect: egui::Rect) {
        FIRST_ROW_RECT.with(|c| {
            if c.get().is_none() {
                c.set(Some(rect));
            }
        });
    }

    thread_local! {
        /// 브레드크럼 세그먼트 각각의 실제 렌더 위치(조상 경로별로). 배경(빈 공간)
        /// 메뉴 우클릭이나 "조상 폴더 클릭 이동" 테스트가 이 자리를 겨냥한다 —
        /// 예전엔 경로 전체가 고정폭 라벨 한 줄이었지만, 지금은 조상 경로 세그먼트
        /// 각각이 줄바꿈될 수 있는 `horizontal_wrapped` 버튼들이라 손으로 위치를
        /// 재현하기 어렵다.
        static BREADCRUMB_SEGMENTS: std::cell::RefCell<std::collections::HashMap<PathBuf, egui::Rect>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }

    pub(super) fn record_breadcrumb_segment_rect_for_test(path: &Path, rect: egui::Rect) {
        BREADCRUMB_SEGMENTS.with(|m| m.borrow_mut().insert(path.to_path_buf(), rect));
    }

    thread_local! {
        /// 트리 노드(폴더 이름 버튼) 각각의 실제 렌더 위치. "트리에서 폴더 클릭 시
        /// 이동" 테스트가 이 자리를 겨냥한다.
        static TREE_NODE_RECTS: std::cell::RefCell<std::collections::HashMap<PathBuf, egui::Rect>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }

    pub(super) fn record_tree_node_rect_for_test(path: &Path, rect: egui::Rect) {
        TREE_NODE_RECTS.with(|m| m.borrow_mut().insert(path.to_path_buf(), rect));
    }

    /// 첫 번째 항목 행이 실제로 어디 그려지는지, 실제 프로덕션 `show()`를 한 번
    /// 돌려서 알아낸다(클릭 없이 레이아웃만) — 레이아웃을 손으로 재현하지 않으므로
    /// 좌측 트리 컬럼이나 브레드크럼 줄바꿈 같은 변화에도 안 깨진다.
    fn find_first_row_pos(ctx: &egui::Context, fs: &FsView) -> egui::Pos2 {
        find_first_row_pos_in_view(ctx, fs, ViewMode::List)
    }

    /// [`find_first_row_pos`]와 같지만 어느 뷰(목록/아이콘)로 그릴지 지정한다.
    /// 아이콘 뷰의 칸은(84×84, 목록 행과 달리 폭 전체가 클릭 영역이라 왼쪽에서
    /// 5px 지점도 언제나 칸 안쪽이라) 목록 행과 동일한 방식으로 위치를 찾을 수
    /// 있다.
    fn find_first_row_pos_in_view(ctx: &egui::Context, fs: &FsView, view_mode: ViewMode) -> egui::Pos2 {
        FIRST_ROW_RECT.with(|c| c.set(None));
        let mut state = FilePanelState::default();
        state.view_mode = view_mode;
        let mut probe = egui::RawInput::default();
        probe.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
        let _ = ctx.run_ui(probe, |ui| {
            let _ = show(ui, fs, &mut state, true);
        });
        let rect = FIRST_ROW_RECT
            .with(|c| c.get())
            .expect("첫 항목이 안 그려짐 — 테스트 전제(폴더에 파일이 있어야 함)가 깨짐");
        egui::pos2(rect.left() + 5.0, rect.center().y)
    }

    /// 브레드크럼에서 특정 조상 경로의 세그먼트가 실제로 어디 그려지는지 알아낸다.
    /// 브레드크럼은 매 프레임 `fs.current_dir`만으로 결정되는 순수 렌더링이라
    /// (트리와 달리 배경 스레드 로딩을 기다릴 필요가 없음) 한 프레임이면 충분하다.
    fn find_breadcrumb_segment_pos(ctx: &egui::Context, fs: &FsView, target: &Path) -> egui::Pos2 {
        BREADCRUMB_SEGMENTS.with(|m| m.borrow_mut().clear());
        let mut state = FilePanelState::default();
        let mut probe = egui::RawInput::default();
        probe.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
        let _ = ctx.run_ui(probe, |ui| {
            let _ = show(ui, fs, &mut state, true);
        });
        let rect = BREADCRUMB_SEGMENTS
            .with(|m| m.borrow().get(target).copied())
            .unwrap_or_else(|| panic!("브레드크럼에서 {target:?} 세그먼트를 못 찾음 — 조상 경로가 맞는지 확인 필요"));
        rect.center()
    }

    /// 트리에서 특정 폴더 노드가 실제로 어디 그려지는지 알아낸다. 트리 하위 목록은
    /// 배경 스레드에서 비동기로 읽어오므로(`spawn_tree_load`), 도착할 때까지 같은
    /// `state`로 여러 프레임을 반복해서 그려본다 — 넘겨받은 `state`를 계속 재사용해야
    /// 한다(매번 새 `FilePanelState`를 쓰면 펼침·캐시 상태가 프레임마다 초기화돼
    /// 영원히 "불러오는 중"만 보임).
    fn find_tree_node_pos(ctx: &egui::Context, fs: &FsView, state: &mut FilePanelState, target: &Path) -> egui::Pos2 {
        for _ in 0..200 {
            TREE_NODE_RECTS.with(|m| m.borrow_mut().clear());
            let mut probe = egui::RawInput::default();
            probe.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
            let _ = ctx.run_ui(probe, |ui| {
                let _ = show(ui, fs, state, true);
            });
            if let Some(rect) = TREE_NODE_RECTS.with(|m| m.borrow().get(target).copied()) {
                return egui::pos2(rect.left() + 5.0, rect.center().y);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("트리에서 {target:?} 노드를 찾지 못함(시간 초과) — 배경 로딩이 너무 오래 걸리거나 노드가 안 나타남");
    }

    /// 지정한 화면 좌표에 마우스 버튼 press+release를 보낸다(secondary=true면
    /// 우클릭). `ctx.run_ui`를 두 프레임 돌려서 클릭을 완성한다 — 실제 클릭도
    /// press 프레임과 release 프레임으로 나뉘어 들어온다. release 프레임에서
    /// `show()`가 돌려준 액션을 반환한다.
    fn click_at(
        ctx: &egui::Context,
        fs: &FsView,
        state: &mut FilePanelState,
        pos: egui::Pos2,
        secondary: bool,
    ) -> Option<FileAction> {
        let button = if secondary { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
        let mut action = None;
        // 워밍업 프레임: 이 컨텍스트에서 행 위젯이 처음 그려지는 프레임에는
        // egui가 히트테스트용 위젯 순서(레이어)를 아직 몰라서 press 프레임의
        // hovered()가 false로 나온다(실측: 워밍업 없이 press→false, release
        // →true, clicked()는 끝까지 false). 클릭 시퀀스 전에 이벤트 없는 프레임을
        // 한 번 그려서 위젯을 "기존재" 상태로 만들어 둔다.
        {
            let mut warmup = egui::RawInput::default();
            warmup.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
            let _ = ctx.run_ui(warmup, |ui| {
                action = show(ui, fs, state, true);
            });
        }
        for pressed in [true, false] {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
            input.events.push(egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            action = None;
            let _ = ctx.run_ui(input, |ui| {
                action = show(ui, fs, state, true);
            });
        }
        action
    }

    /// 지정한 위치를 우클릭해서 컨텍스트 메뉴를 열고, `want`를 만족하는 액션이
    /// 나올 때까지 그 아래쪽 후보 좌표들을 눌러본다. 컨텍스트 메뉴 팝업(`entry_menu`/
    /// `background_menu`)의 정확한 내부 좌표(버튼 높이·구분선 여백 등 egui 내부
    /// 스타일 상수)는 미리 계산할 방법이 없어서, "우클릭한 지점 아래 어딘가"라는
    /// 것만 알고 실제로 눌러보며 찾는다 — 매 시도 사이에 원하는 게 아직 안 나왔으면
    /// 메뉴를 다시 연다(잘못 누른 항목이 메뉴를 닫혀버릴 수 있어서).
    fn right_click_then_find_action(
        ctx: &egui::Context,
        fs: &FsView,
        state: &mut FilePanelState,
        click_pos: egui::Pos2,
        want: impl Fn(&FileAction) -> bool,
    ) -> Option<FileAction> {
        click_at(ctx, fs, state, click_pos, true);
        for dy in [18.0, 26.0, 34.0, 44.0, 52.0, 62.0, 72.0, 82.0, 92.0, 104.0, 116.0, 128.0] {
            for dx in [10.0, 30.0, 50.0, 70.0, 90.0, 120.0, 150.0] {
                let pos = egui::pos2(click_pos.x + dx, click_pos.y + dy);
                if let Some(action) = click_at(ctx, fs, state, pos, false) {
                    if want(&action) {
                        return Some(action);
                    }
                }
                click_at(ctx, fs, state, click_pos, true);
            }
        }
        None
    }

    /// DEV-008 실사용 버그 리포트("복사/붙여넣기 작동 안함") 확인용: 우클릭 →
    /// "복사" → (다른 곳) 우클릭 → "붙여넣기"가 실제 프로덕션 위젯 코드(`show`,
    /// `entry_menu`, `background_menu`)를 통해 끝까지 동작하는지 확인한다.
    #[test]
    fn copy_then_paste_produces_copy_and_paste_actions_via_real_menus() {
        let base = temp_dir_with("copy-paste-ui", &["source.txt"]);
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        let target = base.join("source.txt");

        let row_pos = find_first_row_pos(&ctx, &fs);

        // 우클릭 → 메뉴에서 "복사"를 찾아 누른다 — 실제 프로덕션 위젯 코드
        // (`show`, `entry_menu`)를 통해서만.
        let copy_action =
            right_click_then_find_action(&ctx, &fs, &mut state, row_pos, |a| matches!(a, FileAction::Copy(_)));
        assert_eq!(
            copy_action,
            Some(FileAction::Copy(target.clone())),
            "우클릭 메뉴에서 '복사'를 찾아 눌렀는데 Copy 액션이 안 나옴"
        );

        // show()는 액션만 돌려주고 실제 반영은 호출부(lib.rs)의 몫이다 — 여기서는
        // lib.rs가 하는 일(클립보드에 담기)을 그대로 흉내내서 붙여넣기까지 잇는다.
        state.clipboard = Some(ClipboardEntry { path: target.clone(), cut: false });

        // 브레드크럼의 맨 끝(현재 폴더) 세그먼트 위치를 알아내서 우클릭 →
        // "붙여넣기"를 찾아 누른다 — 배경 메뉴는 그 세그먼트에 붙어있다(`breadcrumb`).
        let path_label_pos = find_breadcrumb_segment_pos(&ctx, &fs, &fs.current_dir);

        let paste_action =
            right_click_then_find_action(&ctx, &fs, &mut state, path_label_pos, |a| matches!(a, FileAction::Paste));

        std::fs::remove_dir_all(&base).ok();

        assert_eq!(
            paste_action,
            Some(FileAction::Paste),
            "우클릭 메뉴에서 '붙여넣기'를 찾아 눌렀는데 Paste 액션이 안 나옴"
        );
    }

    /// DEV-008 후속: 파일을 클릭하면 (더블클릭 전까지는) 열리지 않고 "선택"만
    /// 돼야 한다 — Cmd+C 등 단축키가 뭘 복사할지 알려면 선택 개념이 필요해서
    /// 추가했다.
    #[test]
    fn single_click_on_file_selects_without_opening() {
        let base = temp_dir_with("select-file", &["a.txt"]);
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        let row_pos = find_first_row_pos(&ctx, &fs);

        let action = click_at(&ctx, &fs, &mut state, row_pos, false);
        let selected = state.selected.clone();

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(action, None, "단일 클릭인데 액션이 나옴(더블클릭 전엔 안 열려야 함)");
        assert_eq!(selected, Some(base.join("a.txt")), "클릭했는데 선택 상태가 안 됨");
    }

    /// 실사용 요청: "아이콘뷰가 필요함". 목록 뷰와 동일한 클릭 의미(단일 클릭
    /// 선택, 폴더 없음 확인)가 아이콘 뷰에서도 그대로 지켜지는지 확인한다.
    #[test]
    fn icon_view_click_on_file_selects_without_opening() {
        let base = temp_dir_with("icon-select-file", &["a.txt"]);
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        state.view_mode = ViewMode::Icons;
        let pos = find_first_row_pos_in_view(&ctx, &fs, ViewMode::Icons);

        let action = click_at(&ctx, &fs, &mut state, pos, false);
        let selected = state.selected.clone();

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(action, None, "아이콘 뷰에서 단일 클릭인데 액션이 나옴(더블클릭 전엔 안 열려야 함)");
        assert_eq!(selected, Some(base.join("a.txt")), "아이콘 뷰에서 클릭했는데 선택 상태가 안 됨");
    }

    /// 아이콘 뷰에서도 폴더 클릭 = 즉시 이동(DEV-015 방침)이 유지되는지.
    #[test]
    fn icon_view_click_on_folder_navigates() {
        let base = temp_dir_with("icon-nav-folder", &["sub/"]);
        let sub = base.join("sub");
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        state.view_mode = ViewMode::Icons;
        let pos = find_first_row_pos_in_view(&ctx, &fs, ViewMode::Icons);

        let action = click_at(&ctx, &fs, &mut state, pos, false);

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(action, Some(FileAction::Navigate(sub)), "아이콘 뷰에서 폴더 클릭했는데 Navigate 액션이 안 나옴");
    }

    /// DEV-005 Test plan("대용량 디렉터리에서 안 끊김")을 아이콘 뷰에도 그대로
    /// 적용한다 — 목록 뷰가 `show_rows`로 가상 스크롤하듯, 아이콘 뷰도 격자
    /// 형태로 가상 스크롤해야 한다(`icon_view` 참고). 격자는 한 "줄"에 여러
    /// 칸을 담아 `show_rows`를 재사용하므로, 실수로 전체를 다 그리게 되돌아가는
    /// 회귀를 잡는다.
    #[test]
    fn icon_view_rendering_5000_entries_stays_within_frame_budget() {
        let mut fs = FsView::new(std::env::temp_dir(), || {});
        fs.entries = (0..5000)
            .map(|i| syncshell_core::fsview::DirEntry {
                name: format!("file_{i:05}.txt"),
                path: std::env::temp_dir().join(format!("file_{i:05}.txt")),
                is_dir: false,
                size: 1234,
                modified: Some(std::time::SystemTime::now()),
            })
            .collect();

        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        state.view_mode = ViewMode::Icons;
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 800.0)));

        // 첫 프레임(폰트 아틀라스 워밍업 등 일회성 비용)은 재지 않는다.
        let _ = ctx.run_ui(input.clone(), |ui| {
            show(ui, &fs, &mut state, true);
        });

        let mut worst = std::time::Duration::ZERO;
        for _ in 0..10 {
            let start = std::time::Instant::now();
            let _ = ctx.run_ui(input.clone(), |ui| {
                show(ui, &fs, &mut state, true);
            });
            worst = worst.max(start.elapsed());
        }

        println!("아이콘 뷰 5000개 항목 렌더링 최대 프레임 시간: {worst:?}");
        assert!(
            worst < std::time::Duration::from_millis(30),
            "아이콘 뷰에서 5000개 항목 렌더링이 비정상적으로 느림({worst:?}) — 가상 스크롤이 깨졌을 수 있음"
        );
    }

    /// DEV-005 Test plan: "브레드크럼의 조상 폴더 클릭 시 해당 경로로 정확히
    /// 이동하는지". 맨 끝(현재 폴더) 세그먼트가 아니라 그 앞 조상 세그먼트를
    /// 클릭해야 한다 — 현재 폴더 세그먼트는 눌러도 이동하지 않는 게 맞는 동작.
    #[test]
    fn breadcrumb_ancestor_click_navigates_to_that_folder() {
        let base = temp_dir_with("breadcrumb-nav", &["sub/"]);
        let sub = base.join("sub");
        std::fs::write(sub.join("marker.txt"), b"x").unwrap();
        let fs = settled_view(&sub);
        let ctx = egui::Context::default();

        let base_pos = find_breadcrumb_segment_pos(&ctx, &fs, &base);
        let mut state = FilePanelState::default();
        let action = click_at(&ctx, &fs, &mut state, base_pos, false);

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(
            action,
            Some(FileAction::Navigate(base)),
            "브레드크럼 조상 세그먼트를 클릭했는데 Navigate 액션이 안 나옴"
        );
    }

    /// DEV-005 Test plan: "트리에서 폴더 클릭 시 우측 리스트 및 탐색기 현재
    /// 경로가 정확히 따라가는지". 현재 폴더(`base`)는 자동으로 펼쳐져 있으므로
    /// 그 하위 폴더(`sub`)가 트리에 나타나고, 그걸 클릭하면 Navigate 액션이
    /// 나와야 한다.
    #[test]
    fn tree_node_click_navigates_to_that_folder() {
        let base = isolated_tree_test_dir("tree-nav");
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("marker.txt"), b"x").unwrap();
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();

        let sub_pos = find_tree_node_pos(&ctx, &fs, &mut state, &sub);
        let action = click_at(&ctx, &fs, &mut state, sub_pos, false);

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(
            action,
            Some(FileAction::Navigate(sub)),
            "트리에서 하위 폴더 노드를 클릭했는데 Navigate 액션이 안 나옴"
        );
    }

    /// macOS는 Cmd+C/Cmd+V, 그 외(Windows/Linux)는 Ctrl+Shift+C/V — `show()`의
    /// 플랫폼별 분기와 정확히 같은 조합을 써야 한다(다르면 테스트가 production
    /// 코드와 다른 걸 확인하는 셈이 됨).
    #[cfg(target_os = "macos")]
    fn copy_paste_modifiers() -> (egui::Modifiers, egui::Modifiers) {
        (egui::Modifiers::COMMAND, egui::Modifiers::COMMAND)
    }
    #[cfg(not(target_os = "macos"))]
    fn copy_paste_modifiers() -> (egui::Modifiers, egui::Modifiers) {
        (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT)
    }

    fn send_key_with_modifiers(
        ctx: &egui::Context,
        fs: &FsView,
        state: &mut FilePanelState,
        key: egui::Key,
        modifiers: egui::Modifiers,
        panel_active: bool,
    ) -> Option<FileAction> {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        let mut action = None;
        let _ = ctx.run_ui(input, |ui| {
            action = show(ui, fs, state, panel_active);
        });
        action
    }

    /// DEV-008 후속(사용자 피드백: "복사/붙여넣기 작동 안함" → 실은 우클릭 메뉴는
    /// 됐지만 Cmd+C/V 단축키가 없어서 그렇게 느꼈을 가능성이 컸음). 패널이 활성
    /// 상태일 때 단축키로 선택된 파일을 복사하고 붙여넣을 수 있어야 한다.
    #[test]
    fn copy_paste_keyboard_shortcuts_work_when_panel_active() {
        let base = temp_dir_with("kbd-copy-paste", &["a.txt"]);
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        let row_pos = find_first_row_pos(&ctx, &fs);
        click_at(&ctx, &fs, &mut state, row_pos, false); // 선택

        let (copy_mods, paste_mods) = copy_paste_modifiers();

        let copy_action = send_key_with_modifiers(&ctx, &fs, &mut state, egui::Key::C, copy_mods, true);
        assert_eq!(
            copy_action,
            Some(FileAction::Copy(base.join("a.txt"))),
            "단축키로 복사가 안 됨"
        );

        let paste_action = send_key_with_modifiers(&ctx, &fs, &mut state, egui::Key::V, paste_mods, true);

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(paste_action, Some(FileAction::Paste), "단축키로 붙여넣기가 안 됨");
    }

    /// 터미널에 포커스가 있을 때(패널 비활성)는 단축키가 씹혀야 한다 — Windows/
    /// Linux의 Ctrl+Shift+C는 터미널이 이미 "선택 항상 복사"(리눅스 관례)로 쓰고
    /// 있어서, 파일 패널이 무조건 반응하면 터미널 사용 중에도 파일이 복사돼버리는
    /// 혼란이 생긴다.
    #[test]
    fn keyboard_shortcuts_are_ignored_when_panel_not_active() {
        let base = temp_dir_with("kbd-inactive", &["a.txt"]);
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();
        let row_pos = find_first_row_pos(&ctx, &fs);
        click_at(&ctx, &fs, &mut state, row_pos, false); // 선택은 돼 있음

        let (copy_mods, _) = copy_paste_modifiers();
        let action = send_key_with_modifiers(&ctx, &fs, &mut state, egui::Key::C, copy_mods, false);

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(action, None, "패널이 비활성인데도 단축키가 반응함");
    }

    /// begin_rename은 현재 파일명을 기본값으로 채워야 한다 — 확장자까지 다시
    /// 타이핑하게 만들면 안 된다.
    #[test]
    fn begin_rename_prefills_current_name() {
        let mut state = FilePanelState::default();
        state.begin_rename(Path::new("/some/folder/보고서 최종.txt"));
        assert_eq!(state.rename.as_ref().unwrap().new_name, "보고서 최종.txt");
    }
}
