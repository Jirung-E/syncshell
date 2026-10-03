use crate::chrome::{self, Icon};
use crate::theme::{self, Palette};
use eframe::egui::{self, pos2, vec2, Color32, FontId, Rect};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// 트리 보기에서 그 자리에 펼쳐 둔 폴더들(DEV-022). 현재 폴더 기준이라
    /// 다른 폴더로 이동하면 비운다([`reset_tree_if_moved`]).
    pub tree_expanded: std::collections::HashSet<PathBuf>,
    /// 펼친 폴더의 하위 항목. 배경 스레드에서 읽어오는 동안은 `Loading`, 다
    /// 읽으면 `Loaded`로 바뀐다 — UI 스레드는 디스크를 읽지 않는다(항목이 아주
    /// 많은 폴더를 펼쳤을 때 프레임이 멈추는 걸 실측으로 확인함, 5000행 성능
    /// 테스트가 178ms까지 튐).
    tree_children: std::collections::HashMap<PathBuf, TreeChildren>,
    /// 배경 스레드의 하위 목록 읽기 결과가 도착하는 채널.
    tree_rx: std::sync::mpsc::Receiver<(PathBuf, Vec<DirEntry>)>,
    tree_tx: std::sync::mpsc::Sender<(PathBuf, Vec<DirEntry>)>,
    /// 지금 펼침 상태가 어느 폴더 기준인지.
    tree_for: Option<PathBuf>,
    /// 트리(이름·크기·수정일 + 그 자리 펼침)로 보여줄지, 아이콘 격자로
    /// 보여줄지(실사용 요청: "아이콘뷰가 필요함", DEV-022). 앱 전역 설정이라
    /// 호출부(`SyncShellApp`)가 매 프레임 넣어주고 바뀌면 다시 읽어간다.
    pub view_mode: ViewMode,
}

/// [`FilePanelState::view_mode`] 참고.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Tree,
    Icons,
}

impl ViewMode {
    /// `state.toml`의 `[ui] view` 값.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Icons => "icons",
        }
    }

    /// 모르는 값(손으로 고쳤거나 예전 값)은 기본(트리)으로.
    pub fn parse(s: &str) -> Self {
        match s {
            "icons" => Self::Icons,
            _ => Self::Tree,
        }
    }
}

/// [`FilePanelState::tree_children`] 참고.
enum TreeChildren {
    Loading,
    Loaded(Vec<DirEntry>),
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
            tree_for: None,
            view_mode: ViewMode::Tree,
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

    // 경로바(박스 안 브레드크럼) + 오른쪽 트리/아이콘 전환(DEV-022/023). 예전의
    // "탐색기" 제목 줄은 뺐다 — 경로바가 이 패널이 무엇인지 충분히 말해준다.
    // 현재 경로를 조상 폴더별로 쪼갠 클릭 가능한 세그먼트(브레드크럼)로 보여준다
    // — 경로 직접 입력은 스코프에서 뺐다(터미널에서 cd 치면 어차피 동기화되므로
    // 커맨드라인이 그 역할을 대신함, 2026-08-06 결정). 우클릭 = 폴더 배경 메뉴
    // (새 폴더/새로고침/경로 복사). 목록이 가상 스크롤(show_rows)로 바뀌면서
    // "목록의 빈 공간 우클릭"은 더 이상 안전하게 구현할 수 없어(가상 스크롤
    // 영역은 빈 공간이 따로 없음) 여기로 옮겼다 — 많은 탐색기가 경로 표시줄
    // 우클릭에 폴더 메뉴를 두는 것과 같은 자리.
    let pal = theme::current(ui);
    ui.horizontal(|ui| {
        const SEGMENT_W: f32 = 2.0 * 28.0 + 4.0;
        let box_w = (ui.available_width() - SEGMENT_W - ui.spacing().item_spacing.x).max(40.0);
        ui.allocate_ui_with_layout(vec2(box_w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            egui::Frame::NONE
                .fill(pal.bg)
                .stroke(egui::Stroke::new(1.0, pal.border))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    breadcrumb(ui, &fs.current_dir, &mut action, state.clipboard.is_some());
                });
        });
        // 트리/아이콘 전환. 글리프 대신 painter로 그린 아이콘이라 폰트에 없는
        // 기호가 두부로 나올 걱정이 없다(chrome.rs 참고).
        let picked = chrome::segmented(
            ui,
            pal,
            &[
                (Icon::Tree, "트리 보기", state.view_mode == ViewMode::Tree),
                (Icon::Grid, "아이콘 보기", state.view_mode == ViewMode::Icons),
            ],
        );
        match picked {
            Some(0) => state.view_mode = ViewMode::Tree,
            Some(1) => state.view_mode = ViewMode::Icons,
            _ => {}
        }
    });

    if let Some(msg) = state.status.clone() {
        // 테마(DEV-021)를 따른다 — 고정 색이면 라이트 모드에서 연두색 안내
        // 문구가 흰 바탕에 묻혀 안 읽힌다.
        let color = if state.status_is_error { ui.visuals().error_fg_color } else { ui.visuals().strong_text_color() };
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

    reset_tree_if_moved(state, &fs.current_dir);

    // ".."과 오류 표시는 목록(entries) 밖 고정 영역에 둔다 — 아래 스크롤 영역이
    // `show_rows`로 가상 스크롤되므로, 스크롤 대상이 아닌 것들은 여기서 미리 그린다.
    if let Some(parent) = fs.current_dir.parent() {
        if ui.selectable_label(false, "..").clicked() {
            action = Some(FileAction::Navigate(parent.to_path_buf()));
        }
    }
    if let Some(err) = &fs.error {
        ui.colored_label(ui.visuals().error_fg_color, err);
    }

    match state.view_mode {
        ViewMode::Tree => tree_view(ui, fs, state, &mut action),
        ViewMode::Icons => icon_view(ui, fs, state, &mut action),
    }

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

/// 트리 보기의 한 줄이 무엇을 그리는지. 매 프레임 펼침 상태로 평탄화하는데,
/// 항목(`DirEntry`) 자체는 복사하지 않고 위치만 들고 있다가 화면에 보이는
/// 줄만 꺼내 그린다 — 5000개짜리 폴더에서 매 프레임 전부 복사하지 않게.
enum TreeRow {
    /// `fs.entries[i]`
    Top(usize),
    /// `parents[parent]` 폴더의 `idx`번째 하위 항목
    Child { parent: usize, idx: usize, depth: usize },
    /// 펼쳤지만 아직 배경 스레드가 하위 목록을 읽는 중
    Loading { depth: usize },
}

const TREE_ROW_HEIGHT: f32 = 26.0;
const TREE_INDENT: f32 = 18.0;
const SIZE_COL: f32 = 64.0;
const DATE_COL: f32 = 112.0;
const COL_GAP: f32 = 8.0;
/// 줄 오른쪽 여백(스크롤바가 겹치는 자리 포함).
const ROW_RIGHT_PAD: f32 = 12.0;

fn flatten_tree(fs: &FsView, state: &FilePanelState) -> (Vec<TreeRow>, Vec<PathBuf>) {
    let mut rows = Vec::with_capacity(fs.entries.len());
    let mut parents = Vec::new();
    for (i, entry) in fs.entries.iter().enumerate() {
        rows.push(TreeRow::Top(i));
        if entry.is_dir {
            push_tree_children(state, &entry.path, 1, &mut rows, &mut parents);
        }
    }
    (rows, parents)
}

fn push_tree_children(
    state: &FilePanelState,
    path: &Path,
    depth: usize,
    rows: &mut Vec<TreeRow>,
    parents: &mut Vec<PathBuf>,
) {
    if !state.tree_expanded.contains(path) {
        return;
    }
    match state.tree_children.get(path) {
        Some(TreeChildren::Loaded(children)) => {
            let parent = parents.len();
            parents.push(path.to_path_buf());
            for (idx, child) in children.iter().enumerate() {
                rows.push(TreeRow::Child { parent, idx, depth });
                if child.is_dir {
                    push_tree_children(state, &child.path, depth + 1, rows, parents);
                }
            }
        }
        Some(TreeChildren::Loading) | None => rows.push(TreeRow::Loading { depth }),
    }
}

/// 트리 보기(DEV-022) — 현재 폴더의 항목을 이름·크기·수정일 열과 함께 보여주고,
/// 폴더 옆 화살표로 그 자리에서 하위 항목을 펼친다. 예전의 "좌측 폴더 트리 +
/// 우측 목록" 2단을 하나로 합친 것이다(상위 폴더로는 브레드크럼/".."로 간다).
///
/// DEV-005 Test plan: "대용량 디렉터리(수천 개 파일) 가상 스크롤" — 펼친 하위
/// 항목까지 한 줄 목록으로 평탄화해서 `show_rows`(화면에 보이는 줄만 그림)를
/// 그대로 쓴다.
///
/// **알려진 한계**: 펼친 하위 폴더의 내용은 펼친 시점에 한 번 읽는다 — 파일
/// 감시(DEV-007)는 현재 폴더만 보므로, 하위 폴더 안의 변경은 접었다 다시 펼쳐야
/// 반영된다.
fn tree_view(ui: &mut egui::Ui, fs: &FsView, state: &mut FilePanelState, action: &mut Option<FileAction>) {
    let pal = theme::current(ui);

    // 열 머리
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), egui::Sense::hover());
    let font = FontId::proportional(11.0);
    let cy = head.center().y;
    let meta_right = head.right() - ROW_RIGHT_PAD;
    let painter = ui.painter();
    painter.text(pos2(head.left() + 6.0 + 20.0 + 24.0, cy), egui::Align2::LEFT_CENTER, "이름", font.clone(), pal.muted);
    painter.text(pos2(meta_right - DATE_COL - COL_GAP, cy), egui::Align2::RIGHT_CENTER, "크기", font.clone(), pal.muted);
    painter.text(pos2(meta_right, cy), egui::Align2::RIGHT_CENTER, "수정일", font, pal.muted);
    painter.hline(head.x_range(), head.bottom(), egui::Stroke::new(1.0, pal.border));

    let (rows, parents) = flatten_tree(fs, state);
    ui.scope(|ui| {
        // 줄 사이 틈 없이 붙인다 — 선택/호버 바탕이 줄 단위로 이어져 보이게.
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical()
            .id_salt("file_list_scroll")
            .auto_shrink([false, false])
            .show_rows(ui, TREE_ROW_HEIGHT, rows.len(), |ui, row_range| {
                for row in &rows[row_range] {
                    let (entry, depth) = match *row {
                        TreeRow::Top(i) => (fs.entries[i].clone(), 0),
                        TreeRow::Child { parent, idx, depth } => {
                            match state.tree_children.get(&parents[parent]) {
                                Some(TreeChildren::Loaded(children)) => (children[idx].clone(), depth),
                                _ => continue,
                            }
                        }
                        TreeRow::Loading { depth } => {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(ui.available_width(), TREE_ROW_HEIGHT), egui::Sense::hover());
                            ui.painter().text(
                                pos2(rect.left() + 6.0 + depth as f32 * TREE_INDENT + 20.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                "불러오는 중…",
                                FontId::proportional(12.0),
                                pal.muted,
                            );
                            continue;
                        }
                    };
                    tree_row(ui, pal, &entry, depth, state, action);
                }
            });
    });
}

/// 트리 한 줄. 줄 전체가 클릭 영역(폴더 = 이동, 파일 = 선택/더블클릭 열기 —
/// [`handle_entry_click`])이고, 폴더 앞 화살표만 따로 "그 자리 펼침/접기"다.
fn tree_row(
    ui: &mut egui::Ui,
    pal: &Palette,
    entry: &DirEntry,
    depth: usize,
    state: &mut FilePanelState,
    action: &mut Option<FileAction>,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), TREE_ROW_HEIGHT), egui::Sense::click());
    #[cfg(test)]
    tests::record_row_rect_for_test(&entry.path, rect);

    let x0 = rect.left() + 6.0 + depth as f32 * TREE_INDENT;
    let chevron_rect = Rect::from_min_size(pos2(x0, rect.center().y - 7.0), vec2(14.0, 14.0));
    // 화살표는 줄보다 나중에 등록해야 egui가 위에 있는 걸로 보고 클릭을 화살표에
    // 준다(겹치는 위젯은 나중에 등록된 쪽이 이김) — 그래야 펼치려고 누른 클릭이
    // 그 폴더로 "이동"까지 해버리지 않는다.
    let chevron_clicked = entry.is_dir && {
        let r = ui.interact(chevron_rect.expand(4.0), ui.id().with(("tree_chevron", &entry.path)), egui::Sense::click());
        #[cfg(test)]
        tests::record_chevron_rect_for_test(&entry.path, r.rect);
        r.clicked()
    };

    if ui.is_rect_visible(rect) {
        let selected = !entry.is_dir && state.selected.as_deref() == Some(entry.path.as_path());
        let painter = ui.painter();
        let fill = if selected {
            pal.selection
        } else if response.hovered() {
            pal.hover
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect.shrink2(vec2(2.0, 1.0)), 5.0, fill);
        let (fg, meta) = if selected { (pal.selection_text, pal.selection_text) } else { (pal.text, pal.muted) };

        if entry.is_dir {
            let icon = if state.tree_expanded.contains(&entry.path) { Icon::ChevronDown } else { Icon::ChevronRight };
            chrome::paint_icon(painter, chevron_rect.shrink(2.0), icon, pal.muted);
        }
        let icon_rect = Rect::from_min_size(pos2(x0 + 20.0, rect.center().y - 8.0), vec2(16.0, 16.0));
        let (icon, icon_color) = if entry.is_dir { (Icon::Folder, pal.accent) } else { (Icon::File, pal.muted) };
        chrome::paint_icon(painter, icon_rect, icon, icon_color);

        let meta_right = rect.right() - ROW_RIGHT_PAD;
        let name_left = icon_rect.right() + 8.0;
        let name_max = (meta_right - DATE_COL - COL_GAP - SIZE_COL - COL_GAP - name_left).max(10.0);
        let galley = name_galley(ui, &entry.name, name_max, fg);
        painter.galley(pos2(name_left, rect.center().y - galley.size().y / 2.0), galley, fg);

        let small = FontId::proportional(12.0);
        let cy = rect.center().y;
        if !entry.is_dir {
            painter.text(pos2(meta_right - DATE_COL - COL_GAP, cy), egui::Align2::RIGHT_CENTER, format_size(entry.size), small.clone(), meta);
        }
        if let Some(m) = entry.modified {
            painter.text(pos2(meta_right, cy), egui::Align2::RIGHT_CENTER, format_modified(m), small, meta);
        }
    }

    if chevron_clicked {
        toggle_tree_expanded(ui.ctx(), state, &entry.path);
    } else {
        handle_entry_click(entry, &response, state, action);
    }

    response.context_menu(|ui| {
        if let Some(picked) = entry_menu(ui, &entry.path, entry.is_dir) {
            *action = Some(picked);
            ui.close();
        }
    });
}

/// 이름 칸 한 줄. 넘치면 말줄임(…)으로 자른다 — 안 자르면 옆 크기/수정일 열과
/// 겹친다(실사용 버그 리포트: "문서 제목과 수정 날짜가 겹쳐서 출력").
fn name_galley(ui: &egui::Ui, name: &str, max_width: f32, color: Color32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        name.to_owned(),
        egui::TextFormat::simple(FontId::proportional(13.0), color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
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
        let pal = theme::current(ui);
        let selected = !entry.is_dir && state.selected.as_deref() == Some(entry.path.as_path());
        if selected {
            ui.painter().rect(rect.shrink(2.0), 8.0, pal.selection, egui::Stroke::new(1.0, pal.accent), egui::StrokeKind::Inside);
        } else if response.hovered() {
            ui.painter().rect_filled(rect.shrink(2.0), 8.0, pal.hover);
        }

        const NAME_HEIGHT: f32 = 26.0;
        let icon_center = pos2(rect.center().x, rect.min.y + (size.y - NAME_HEIGHT) / 2.0 + 4.0);
        let (icon, color) = if entry.is_dir { (Icon::Folder, pal.accent) } else { (Icon::File, pal.muted) };
        chrome::paint_icon(ui.painter(), Rect::from_center_size(icon_center, vec2(30.0, 30.0)), icon, color);
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
        let faint = theme::current(ui).faint;
        for (idx, ancestor) in ancestors.iter().rev().enumerate() {
            // 맨 앞 루트("/", "C:\\")는 그 자체가 구분자 모양이라 바로 뒤엔 또
            // 찍지 않는다("/ / Users"가 되지 않게).
            if idx > 1 {
                ui.label(egui::RichText::new("/").color(faint));
            }
            let is_current = idx == ancestors.len() - 1;
            let name = ancestor
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| ancestor.display().to_string());
            // 지금 폴더는 진하게, 조상은 흐리게 — 선택 바탕 대신 글자로 구분한다.
            let text = if is_current { egui::RichText::new(name).strong() } else { egui::RichText::new(name).weak() };
            let response = ui.add(egui::Button::new(text).small().frame(false));
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

/// 배경 스레드에서 읽어온 하위 목록 결과를 받아 반영한다. `show()` 시작
/// 부분에서 매 프레임 호출한다 — 아직 아무것도 안 왔으면 그냥 넘어간다. 그새
/// 접었거나 다른 폴더로 이동해서 더 이상 펼쳐져 있지 않은 폴더의 결과는 버린다
/// (나중에 다시 펼칠 때 오래된 내용이 보이지 않게).
fn drain_tree_loads(state: &mut FilePanelState) {
    while let Ok((path, children)) = state.tree_rx.try_recv() {
        if state.tree_expanded.contains(&path) {
            state.tree_children.insert(path, TreeChildren::Loaded(children));
        }
    }
}

/// 폴더 하나의 하위 항목 읽기를 배경 스레드에서 시작한다(이미 읽는 중이거나 다
/// 읽었으면 아무것도 안 함). 정렬·심볼릭 링크 처리는 현재 폴더 목록과 똑같아야
/// 해서 core의 `read_dir_sorted`를 그대로 쓴다.
fn spawn_tree_load(ctx: &egui::Context, state: &mut FilePanelState, path: &Path) {
    if state.tree_children.contains_key(path) {
        return;
    }
    state.tree_children.insert(path.to_path_buf(), TreeChildren::Loading);
    let tx = state.tree_tx.clone();
    let ctx = ctx.clone();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let children = syncshell_core::fsview::read_dir_sorted(&path).unwrap_or_default();
        let _ = tx.send((path, children));
        ctx.request_repaint();
    });
}

/// 화살표를 누르면 펼침/접기. 접을 때 읽어둔 내용도 버린다 — 다시 펼치면 새로
/// 읽어서, 그동안 바뀐 내용이 보이게 한다(하위 폴더는 파일 감시 대상이 아님).
fn toggle_tree_expanded(ctx: &egui::Context, state: &mut FilePanelState, path: &Path) {
    if state.tree_expanded.remove(path) {
        state.tree_children.remove(path);
    } else {
        state.tree_expanded.insert(path.to_path_buf());
        spawn_tree_load(ctx, state, path);
    }
}

/// 펼침 상태는 "지금 보고 있는 폴더" 기준이다 — 다른 폴더로 이동하면(클릭이든
/// 터미널 cd 동기화든) 비운다.
fn reset_tree_if_moved(state: &mut FilePanelState, current_dir: &Path) {
    if state.tree_for.as_deref() != Some(current_dir) {
        state.tree_expanded.clear();
        state.tree_children.clear();
        state.tree_for = Some(current_dir.to_path_buf());
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

    /// 실사용 버그 리포트("문서 제목과 수정 날짜가 겹쳐서 출력")의 회귀 테스트:
    /// 이름 칸 폭보다 긴 파일명은 말줄임으로 잘려 그 폭을 넘지 않아야 한다 —
    /// 넘으면 옆에 오른쪽 정렬로 그리는 크기/수정일 열과 겹친다. 트리 보기의 실제
    /// 줄이 쓰는 `name_galley`를 그대로 쓴다.
    #[test]
    fn long_filename_is_truncated_within_name_column_width() {
        let ctx = egui::Context::default();
        let name_width = 150.0f32;
        let long_name = "이_파일은_이름이_아주아주아주아주아주아주아주아주아주아주_길다.txt";

        let mut size = egui::Vec2::ZERO;
        let mut rows = 0;
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 100.0)));
        let _ = ctx.run_ui(input, |ui| {
            let galley = name_galley(ui, long_name, name_width, Color32::WHITE);
            size = galley.size();
            rows = galley.rows.len();
        });

        assert!(
            size.x <= name_width + 1.0, // 부동소수점 오차 약간 허용
            "긴 파일명이 이름 칸 폭({name_width})을 넘어감: {size:?} — 옆 열(크기/수정일)과 겹칠 수 있음"
        );
        assert_eq!(rows, 1, "긴 파일명이 여러 줄로 접힘 — 한 줄에서 말줄임으로 잘려야 함");
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
    pub(super) fn record_row_rect_for_test(path: &Path, rect: egui::Rect) {
        FIRST_ROW_RECT.with(|c| {
            if c.get().is_none() {
                c.set(Some(rect));
            }
        });
        ROW_RECTS.with(|m| m.borrow_mut().insert(path.to_path_buf(), rect));
    }

    thread_local! {
        /// 항목 줄 각각의 실제 렌더 위치(경로별) — 펼친 하위 항목처럼 "첫 줄"이
        /// 아닌 특정 줄을 겨냥할 때 쓴다.
        static ROW_RECTS: std::cell::RefCell<std::collections::HashMap<PathBuf, egui::Rect>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
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
        /// 트리 보기에서 폴더 앞 펼침 화살표 각각의 실제 클릭 영역(DEV-022).
        static CHEVRON_RECTS: std::cell::RefCell<std::collections::HashMap<PathBuf, egui::Rect>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }

    pub(super) fn record_chevron_rect_for_test(path: &Path, rect: egui::Rect) {
        CHEVRON_RECTS.with(|m| m.borrow_mut().insert(path.to_path_buf(), rect));
    }

    /// 첫 번째 항목 행이 실제로 어디 그려지는지, 실제 프로덕션 `show()`를 한 번
    /// 돌려서 알아낸다(클릭 없이 레이아웃만) — 레이아웃을 손으로 재현하지 않으므로
    /// 좌측 트리 컬럼이나 브레드크럼 줄바꿈 같은 변화에도 안 깨진다.
    fn find_first_row_pos(ctx: &egui::Context, fs: &FsView) -> egui::Pos2 {
        find_first_row_pos_in_view(ctx, fs, ViewMode::Tree)
    }

    /// [`find_first_row_pos`]와 같지만 어느 뷰(트리/아이콘)로 그릴지 지정한다.
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

    /// 트리 보기에서 `target`의 펼침 화살표(`chevron == true`) 또는 줄이 실제로
    /// 어디 그려지는지 알아낸다. 펼친 하위 목록은 배경 스레드에서 읽어오므로
    /// (`spawn_tree_load`) 나타날 때까지 같은 `state`로 여러 프레임을 그려본다 —
    /// 넘겨받은 `state`를 계속 써야 한다(새로 만들면 펼침 상태가 초기화됨).
    fn find_tree_pos(
        ctx: &egui::Context,
        fs: &FsView,
        state: &mut FilePanelState,
        target: &Path,
        chevron: bool,
    ) -> egui::Pos2 {
        for _ in 0..200 {
            ROW_RECTS.with(|m| m.borrow_mut().clear());
            CHEVRON_RECTS.with(|m| m.borrow_mut().clear());
            let mut probe = egui::RawInput::default();
            probe.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
            let _ = ctx.run_ui(probe, |ui| {
                let _ = show(ui, fs, state, true);
            });
            let map = if chevron { &CHEVRON_RECTS } else { &ROW_RECTS };
            if let Some(rect) = map.with(|m| m.borrow().get(target).copied()) {
                // 줄의 가운데는 화살표(왼쪽 끝)와 겹치지 않는다.
                return rect.center();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("트리에서 {target:?}를 찾지 못함(시간 초과) — 배경 로딩이 너무 오래 걸리거나 줄이 안 나타남");
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

    /// DEV-022: 폴더 앞 화살표를 누르면 이동하지 않고 그 자리에서 하위 항목이
    /// 펼쳐지고(배경 로딩), 펼쳐진 하위 폴더 줄을 누르면 그 폴더로 이동해야 한다.
    /// 다시 누르면 접혀서 하위 줄이 사라진다.
    #[test]
    fn tree_chevron_expands_in_place_and_child_click_navigates() {
        let base = temp_dir_with("tree-expand", &["sub/", "sub/inner/", "sub/note.txt"]);
        let sub = base.join("sub");
        let inner = sub.join("inner");
        let fs = settled_view(&base);
        let ctx = egui::Context::default();
        let mut state = FilePanelState::default();

        let chevron = find_tree_pos(&ctx, &fs, &mut state, &sub, true);
        let expand_action = click_at(&ctx, &fs, &mut state, chevron, false);
        assert_eq!(expand_action, None, "화살표를 눌렀는데 폴더로 이동해버림 — 펼치기만 해야 함");
        assert!(state.tree_expanded.contains(&sub), "화살표를 눌렀는데 펼침 상태가 안 됨");

        let inner_pos = find_tree_pos(&ctx, &fs, &mut state, &inner, false);
        let note_seen = ROW_RECTS.with(|m| m.borrow().contains_key(&sub.join("note.txt")));
        let action = click_at(&ctx, &fs, &mut state, inner_pos, false);

        let chevron = find_tree_pos(&ctx, &fs, &mut state, &sub, true);
        click_at(&ctx, &fs, &mut state, chevron, false);
        let collapsed = !state.tree_expanded.contains(&sub);

        std::fs::remove_dir_all(&base).ok();
        assert!(note_seen, "펼친 폴더의 파일(note.txt)이 트리에 안 나옴 — 폴더뿐 아니라 파일도 보여야 함");
        assert_eq!(action, Some(FileAction::Navigate(inner)), "펼친 하위 폴더 줄을 눌렀는데 Navigate가 안 나옴");
        assert!(collapsed, "화살표를 다시 눌렀는데 안 접힘");
    }

    /// 펼침 상태는 지금 보는 폴더 기준 — 다른 폴더로 옮겨가면 비워져야 한다
    /// (안 그러면 돌아왔을 때 예전 펼침과 오래된 하위 목록이 남아 있음).
    #[test]
    fn tree_expansion_resets_when_current_folder_changes() {
        let mut state = FilePanelState::default();
        let a = PathBuf::from("/a");
        reset_tree_if_moved(&mut state, &a);
        state.tree_expanded.insert(a.join("x"));
        reset_tree_if_moved(&mut state, &a);
        assert_eq!(state.tree_expanded.len(), 1, "같은 폴더인데 펼침이 초기화됨");
        reset_tree_if_moved(&mut state, Path::new("/b"));
        assert!(state.tree_expanded.is_empty(), "다른 폴더로 옮겼는데 펼침이 남아 있음");
    }

    #[test]
    fn view_mode_round_trips_through_string_and_defaults_to_tree() {
        for mode in [ViewMode::Tree, ViewMode::Icons] {
            assert_eq!(ViewMode::parse(mode.as_str()), mode);
        }
        assert_eq!(ViewMode::parse("list"), ViewMode::Tree, "모르는/예전 값은 트리로");
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
