use eframe::egui;
use std::path::{Path, PathBuf};
use syncshell_core::fsview::FsView;

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
}

/// 이름 바꾸기 대화상자가 떠 있는 동안의 상태.
pub struct RenameState {
    pub target: PathBuf,
    pub new_name: String,
    /// 대화상자가 뜬 첫 프레임에 텍스트 칸으로 포커스를 옮기고 전체 선택하기 위한 플래그.
    focus_pending: bool,
}

/// 패널이 프레임 간에 들고 있어야 하는 상태.
#[derive(Default)]
pub struct FilePanelState {
    pub rename: Option<RenameState>,
    /// 조작 결과 메시지(주로 오류). 다음 조작 때까지 패널 위에 남는다.
    pub status: Option<String>,
    pub status_is_error: bool,
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
pub fn show(ui: &mut egui::Ui, fs: &FsView, state: &mut FilePanelState) -> Option<FileAction> {
    let mut action: Option<FileAction> = None;

    ui.heading("탐색기");
    ui.label(fs.current_dir.display().to_string());

    if let Some(msg) = state.status.clone() {
        let color = if state.status_is_error {
            egui::Color32::from_rgb(0xe0, 0x6c, 0x75)
        } else {
            egui::Color32::from_rgb(0x98, 0xc3, 0x79)
        };
        let mut dismissed = false;
        ui.horizontal(|ui| {
            ui.colored_label(color, msg);
            if ui.small_button("✕").clicked() {
                dismissed = true;
            }
        });
        if dismissed {
            state.status = None;
        }
    }
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(parent) = fs.current_dir.parent() {
                if ui.selectable_label(false, "..").clicked() {
                    action = Some(FileAction::Navigate(parent.to_path_buf()));
                }
            }
            if let Some(err) = &fs.error {
                ui.colored_label(egui::Color32::from_rgb(0xe0, 0x6c, 0x75), err);
            }

            for entry in &fs.entries {
                let label = if entry.is_dir {
                    format!("📁 {}", entry.name)
                } else {
                    format!("   {}", entry.name)
                };
                let response = ui.selectable_label(false, &label);

                if entry.is_dir {
                    // 폴더는 단일 클릭 진입 유지 — TR-003/TR-005의 동기화 설계가
                    // 이걸 전제로 짜여 있다(DEV-015 조사 결과, 폴더까지 더블클릭으로
                    // 바꾸면 sync 쪽 영향 범위가 커서 보류).
                    if response.clicked() {
                        action = Some(FileAction::Navigate(entry.path.clone()));
                    }
                } else if response.double_clicked() {
                    // 파일은 반드시 더블클릭 — 한 번만 클릭해도 실행파일이
                    // 열려버리던 문제(TR-006 피드백)를 막기 위해 일반 탐색기처럼
                    // 더블클릭을 요구한다(DEV-015).
                    action = Some(FileAction::OpenFile(entry.path.clone()));
                }

                response.context_menu(|ui| {
                    if let Some(picked) = entry_menu(ui, &entry.path, entry.is_dir) {
                        action = Some(picked);
                        ui.close();
                    }
                });
            }

            // 목록의 빈 공간에서 우클릭해도 메뉴가 떠야 한다(일반 탐색기와 동일).
            // 스크롤 영역을 남는 높이까지 채운 뒤(auto_shrink false) 그 위에
            // 반응 영역을 하나 깔아 둔다.
            let empty_space = ui.available_size();
            if empty_space.y > 0.0 {
                let (_, bg) = ui.allocate_exact_size(empty_space, egui::Sense::click());
                bg.context_menu(|ui| {
                    if let Some(picked) = background_menu(ui, &fs.current_dir) {
                        action = Some(picked);
                        ui.close();
                    }
                });
            }
        });

    if let Some(picked) = rename_dialog(ui.ctx(), state) {
        action = Some(picked);
    }

    action
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

/// 빈 공간 우클릭 메뉴(현재 폴더 대상).
fn background_menu(ui: &mut egui::Ui, current_dir: &Path) -> Option<FileAction> {
    let mut action = None;
    ui.set_min_width(180.0);

    if ui.button("새 폴더").clicked() {
        action = Some(FileAction::NewFolder);
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
            action = show(ui, fs, state);
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
            action = show(ui, &fs, &mut state);
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
            action = show(ui, &fs, &mut state);
        });

        std::fs::remove_dir_all(&base).ok();
        assert_eq!(action, None, "Esc로 닫았는데 액션이 나옴");
        assert!(state.rename.is_none(), "Esc로 닫았는데 상태가 안 정리됨");
    }

    /// begin_rename은 현재 파일명을 기본값으로 채워야 한다 — 확장자까지 다시
    /// 타이핑하게 만들면 안 된다.
    #[test]
    fn begin_rename_prefills_current_name() {
        let mut state = FilePanelState::default();
        state.begin_rename(Path::new(r"C:\some\folder\보고서 최종.txt"));
        assert_eq!(state.rename.as_ref().unwrap().new_name, "보고서 최종.txt");
    }
}
