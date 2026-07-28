use std::path::PathBuf;

/// 탐색기 ↔ 터미널 동기화의 단일 진리원.
///
/// 두 개의 진입점만 있다 — 호출부(UI)가 "사용자가 실제로 클릭했다"와
/// "동기화 이벤트로 인해 화면만 옮긴다"를 절대 섞지 않는 게 이 구조의 전제다:
/// - `user_navigated`: 탐색기에서 사용자가 실제로 폴더를 클릭했을 때만 호출.
///   터미널에 주입할 명령 문자열을 돌려준다.
/// - `terminal_cwd_changed`: OSC7으로 감지된 cwd 변화가 들어올 때마다 호출.
///   탐색기를 옮겨야 하면 `Some(path)`를 돌려준다 — 이때 호출부는 `FsView::navigate`를
///   **직접** 불러야 하고, 절대 `user_navigated`를 다시 거치면 안 된다 (거치면 루프 위험).
///
/// 에코 억제는 "지금 이 값을 기대하고 있다" 같은 별도 상태를 두지 않고,
/// `current`(탐색기가 지금 보여주고 있다고 알고 있는 경로) 하나와의 단순 비교로만 한다.
/// 별도 expecting 슬롯을 뒀던 첫 구현은 사용자가 폴더를 연달아 두 번 클릭했을 때
/// (예: A → B) 중간값 A의 에코가 뒤늦게 도착하면 `current`가 A로 되돌아가고,
/// 그 뒤에 도착하는 진짜 최종값 B의 에코가 "이미 기대 중이던 값"으로 오인되어
/// 억제당해 탐색기가 A에 멈춘 채 다시는 B로 안 움직이는 버그가 있었다(손으로 추적해서
/// 발견, sync_check 예제 실행 전에 수정). `current` 단일 비교는 이 문제가 없다 —
/// 매번 "지금 탐색기가 보여주는 것과 다른가"만 보므로 어떤 순서로 이벤트가 와도
/// 결국 최신 상태로 수렴한다.
pub struct SyncState {
    current: Option<PathBuf>,
}

impl SyncState {
    pub fn new() -> Self {
        Self { current: None }
    }

    /// 탐색기 이동은 항상 직계 자식(폴더 클릭) 또는 부모(".." 클릭)로만 일어나므로,
    /// 가능하면 절대경로 대신 상대경로(폴더 이름 또는 "..")로 cd를 주입한다 —
    /// 터미널 히스토리에 매번 전체 경로가 찍히는 것보다 자연스럽다(TR-006 피드백).
    /// 드라이브를 넘나드는 등 직계 관계가 아닌 경우(예: 터미널 쪽 상태와 어긋난
    /// 드문 경우)에는 안전하게 절대경로로 폴백한다.
    pub fn user_navigated(&mut self, path: PathBuf) -> String {
        // 줄 끝은 반드시 \r 하나만 — 실제 키보드 Enter도 \r만 보낸다
        // (terminal_widget.rs). \r\n을 쓰면 \r로 제출된 직후 남는 \n이 빈 줄에서
        // 또 한 번의 Enter처럼 처리되어 PSReadLine이 연속줄 프롬프트(">>")를
        // 잘못 띄우는 것을 실측으로 확인했다(TR-006 "첫 실행시 엔터가 이미 쳐진
        // 것 같다" 피드백 — 초기화 스크립트에서 \r\n → \r로 바꿔서 재현·해결).
        let cmd = match &self.current {
            Some(cur) if Some(cur.as_path()) == path.parent() => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                format!("cd \"{name}\"\r")
            }
            Some(cur) if cur.parent() == Some(path.as_path()) => "cd ..\r".to_string(),
            _ => format!("cd \"{}\"\r", path.display()),
        };
        self.current = Some(path);
        cmd
    }

    pub fn terminal_cwd_changed(&mut self, path: PathBuf) -> Option<PathBuf> {
        if self.current.as_ref() == Some(&path) {
            return None;
        }
        self.current = Some(path.clone());
        Some(path)
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_click_produces_cd_and_does_not_self_trigger() {
        let mut sync = SyncState::new();
        let cmd = sync.user_navigated(PathBuf::from(r"C:\Windows"));
        assert_eq!(cmd, "cd \"C:\\Windows\"\r");

        // 주입한 cd의 에코가 도착 — 탐색기를 다시 옮기라는 이벤트가 나오면 안 됨
        let event = sync.terminal_cwd_changed(PathBuf::from(r"C:\Windows"));
        assert_eq!(event, None);
    }

    #[test]
    fn navigating_into_child_folder_uses_relative_name() {
        let mut sync = SyncState::new();
        let _ = sync.user_navigated(PathBuf::from(r"C:\Users\User"));
        let cmd = sync.user_navigated(PathBuf::from(r"C:\Users\User\Documents"));
        assert_eq!(cmd, "cd \"Documents\"\r");
    }

    #[test]
    fn navigating_to_parent_uses_dotdot() {
        let mut sync = SyncState::new();
        let _ = sync.user_navigated(PathBuf::from(r"C:\Users\User\Documents"));
        let cmd = sync.user_navigated(PathBuf::from(r"C:\Users\User"));
        assert_eq!(cmd, "cd ..\r");
    }

    #[test]
    fn navigating_to_unrelated_path_falls_back_to_absolute() {
        let mut sync = SyncState::new();
        let _ = sync.user_navigated(PathBuf::from(r"C:\Users\User"));
        // 드라이브를 건너뛰는 등 직계 부모/자식 관계가 아닌 이동
        let cmd = sync.user_navigated(PathBuf::from(r"D:\Projects"));
        assert_eq!(cmd, "cd \"D:\\Projects\"\r");
    }

    #[test]
    fn terminal_cd_triggers_explorer_move() {
        let mut sync = SyncState::new();
        let event = sync.terminal_cwd_changed(PathBuf::from(r"C:\Users\User"));
        assert_eq!(event, Some(PathBuf::from(r"C:\Users\User")));
    }

    #[test]
    fn repeated_same_path_is_not_reported_twice() {
        let mut sync = SyncState::new();
        assert!(sync.terminal_cwd_changed(PathBuf::from(r"C:\A")).is_some());
        assert_eq!(sync.terminal_cwd_changed(PathBuf::from(r"C:\A")), None);
    }

    #[test]
    fn distinct_terminal_originated_changes_after_echo_still_fire() {
        let mut sync = SyncState::new();
        let _ = sync.user_navigated(PathBuf::from(r"C:\A"));
        assert_eq!(sync.terminal_cwd_changed(PathBuf::from(r"C:\A")), None); // 에코 억제

        // 사용자가 터미널에서 직접 cd — 새로운 변경이므로 탐색기가 따라가야 함
        let event = sync.terminal_cwd_changed(PathBuf::from(r"C:\B"));
        assert_eq!(event, Some(PathBuf::from(r"C:\B")));
    }

    /// 연달아 두 번 클릭(A → B)한 뒤 중간값 A의 에코가 늦게 도착해도,
    /// 그 뒤에 도착하는 진짜 최종값 B는 반드시 탐색기 이동으로 이어져야 한다.
    /// (별도 expecting 슬롯을 뒀던 구현에서 이 케이스가 깨졌던 회귀 테스트.)
    #[test]
    fn rapid_double_click_always_converges_to_final_state() {
        let mut sync = SyncState::new();
        let _ = sync.user_navigated(PathBuf::from(r"C:\A"));
        let _ = sync.user_navigated(PathBuf::from(r"C:\B"));

        // 중간값 A의 지연된 에코 — current가 B인 상태에서 A가 오면 "다르다"고
        // 판단해 일시적으로 탐색기를 A로 옮긴다(과도기적 깜빡임, 허용 범위).
        let event = sync.terminal_cwd_changed(PathBuf::from(r"C:\A"));
        assert_eq!(event, Some(PathBuf::from(r"C:\A")));

        // 진짜 최종값 B가 도착하면, current가 지금 A이므로 "다르다"고 판단해
        // 반드시 탐색기를 B로 옮긴다 — 여기서 멈춰있으면 회귀.
        let event = sync.terminal_cwd_changed(PathBuf::from(r"C:\B"));
        assert_eq!(event, Some(PathBuf::from(r"C:\B")));

        // 이후 같은 B가 다시 와도(예: 다음 프레임 재확인) 더 이상 이벤트가 없다.
        assert_eq!(sync.terminal_cwd_changed(PathBuf::from(r"C:\B")), None);
    }
}
