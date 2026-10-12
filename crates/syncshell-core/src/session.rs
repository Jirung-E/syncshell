//! 창·표시 상태 복원과 앱 설정 저장(DEV-010). 탭과 탭별 경로는 복원하지
//! 않는다 — 시작은 항상 홈 폴더 탭 하나(DEV-031).
//!
//! ## 저장 위치: `~/.syncshell/` 하나, 세 플랫폼 동일
//!
//! `directories` 크레이트나 XDG 3분할(config/state/cache)은 쓰지 않는다.
//! 개발 도구는 설정을 찾을 수 있어야 하고(`~/.cargo`, `~/.ssh`, `~/.gitconfig`
//! 관례), `%LOCALAPPDATA%\Packages\...\LocalState\` 류로 파묻히면 안 된다.
//!
//! ```text
//! ~/.syncshell/
//! ├── config.toml    ← 사람이 만지는 것 (셸, 동기화 기본값)
//! └── state.toml     ← 앱이 덮어쓰는 것 (창 위치/크기, 테마·배율 등 표시 상태)
//! ```
//!
//! 파일을 둘로 나누는 이유는 git 관리가 아니라: 설정은 사람이 쓰고 상태는 앱이
//! 매 종료마다 덮어쓴다 — 한 파일이면 앱이 통째로 재작성하면서 사용자 주석과
//! 포맷을 날린다. 세션 저장 중 문제가 생겨도 설정까지 손상되지 않는다(blast
//! radius 분리).
//!
//! `SYNCSHELL_HOME` 환경변수로 위치를 오버라이드할 수 있다(포터블 실행·테스트용).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `~/.syncshell/`(또는 `SYNCSHELL_HOME`) 경로. 호출부는 이 함수 하나만 거쳐야
/// 한다 — 경로를 직접 조립하지 않아야 나중에(예: 리눅스 패키징 메인테이너가
/// XDG를 요구하는 경우) 이 함수 하나만 바꾸면 된다.
pub fn syncshell_home() -> PathBuf {
    if let Ok(over) = std::env::var("SYNCSHELL_HOME") {
        return PathBuf::from(over);
    }
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(home_var).unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".syncshell")
}

pub fn config_path() -> PathBuf {
    syncshell_home().join("config.toml")
}

pub fn state_path() -> PathBuf {
    syncshell_home().join("state.toml")
}

/// `~/.syncshell/`이 없으면 만든다. Windows에서는 만든 직후 숨김 속성을 세팅한다
/// — "." 로 시작하는 이름이 Windows에서는 자동으로 숨겨지지 않기 때문이다(숨김
/// 속성은 이름과 별개). syncshell 자체가 파일 탐색기라 이걸 안 하면 자기가 만든
/// 폴더가 자기 화면에 노출된다.
pub fn ensure_home_dir() -> std::io::Result<()> {
    let home = syncshell_home();
    if !home.exists() {
        std::fs::create_dir_all(&home)?;
        #[cfg(windows)]
        hide_dir_windows(&home);
    }
    Ok(())
}

#[cfg(windows)]
fn hide_dir_windows(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // 실패해도(예: 권한 문제) 폴더 자체는 이미 만들어졌으니 치명적으로 다루지
    // 않는다 — 숨김 속성은 편의 기능이지 필수 기능이 아니다.
    unsafe {
        SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN);
    }
}

/// 사람이 편집하는 설정. 없거나 손상됐으면 기본값으로 조용히 복구한다 —
/// 탐색기 앱이 설정 파일 하나 때문에 아예 안 뜨면 안 된다.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// 지정하면 `default_shell()`의 자동 판단보다 우선한다.
    pub default_shell: Option<String>,
    /// 즐겨찾기 폴더(DEV-028). 사람이 직접 고쳐도 되고, 앱도 추가·제거 때
    /// [`save_favorites`]로 이 항목만 고쳐 쓴다(다른 내용·주석은 보존).
    pub favorites: Vec<PathBuf>,
}

/// 앱이 종료 시 덮어쓰는 상태. 예전 버전이 남긴 `tabs`·`active_tab`은 모르는
/// 항목으로 무시되고 다음 저장 때 사라진다(DEV-031).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct State {
    pub window: Option<WindowState>,
    pub ui: UiState,
}

/// 사용자가 UI에서 고른 표시 설정(DEV-021~). 앱이 종료 때 덮어쓰는 값이라
/// `config.toml`이 아니라 여기 둔다. core는 UI를 모르므로(architecture 규칙 1)
/// 값은 문자열로만 들고, 해석은 UI 크레이트가 한다 — 모르는 값이면 UI 쪽이
/// 기본값으로 돌아간다.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct UiState {
    /// "dark" | "light"
    pub theme: String,
    /// 탐색기 보기 방식 "tree" | "icons"
    pub view: String,
    /// 탐색기·터미널 분할 "side" | "stacked"
    pub layout: String,
    /// UI 배율(1.0 = 100%). 없으면 기본값 — DEV-026
    pub ui_scale: Option<f64>,
    /// 터미널 글자 크기(UI 배율 100% 기준 pt). 없으면 기본값 — DEV-026
    pub term_font_size: Option<f64>,
    /// 탐색기 안 미리보기 칸을 켜 뒀는지(DEV-029). 없으면 꺼짐
    pub preview_pane: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// 손상된 파일을 `<원래 이름>.bak`로 남기고 기본값으로 돌아간다. 이미 있던
/// `.bak`는 덮어쓴다 — 여러 번 손상돼도 디스크에 무한정 쌓이지 않게.
fn recover_corrupted(path: &Path) {
    let bak = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.bak"),
        None => "bak".to_string(),
    });
    let _ = std::fs::copy(path, &bak);
}

/// `config.toml`을 읽는다. 없으면(첫 실행 등) 조용히 기본값. 손상됐으면(TOML
/// 파싱 실패) `.bak`로 백업하고 기본값 — 오타 하나 때문에 앱이 안 뜨면 안 된다.
pub fn load_config() -> Config {
    let path = config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    match toml::from_str(&text) {
        Ok(config) => config,
        Err(_) => {
            recover_corrupted(&path);
            Config::default()
        }
    }
}

/// `config.toml`의 `favorites`만 바꿔 쓴다(DEV-028). 사람이 쓰는 파일이라 통째로
/// 다시 직렬화하지 않고 `toml_edit`으로 그 항목만 고친다 — 다른 설정, 주석, 순서는
/// 그대로 남는다. 파일이 지금 TOML로 안 읽히면(사람이 고치다 만 상태 등) 덮어쓰지
/// 않고 오류를 돌려준다.
pub fn save_favorites(favorites: &[PathBuf]) -> std::io::Result<()> {
    ensure_home_dir()?;
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(std::io::Error::other)?;
    let mut array = toml_edit::Array::new();
    for fav in favorites {
        array.push(fav.to_string_lossy().as_ref());
    }
    // 여러 개면 한 줄에 하나씩 — 사람이 읽고 고치기 쉽게.
    for value in array.iter_mut() {
        value.decor_mut().set_prefix("\n    ");
    }
    array.set_trailing("\n");
    array.set_trailing_comma(true);
    doc["favorites"] = toml_edit::value(array);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, &path)
}

/// `state.toml`을 읽는다. `load_config`와 같은 이유로 없거나 손상되면 기본값
/// (빈 상태 — 호출부가 "복원할 세션 없음"으로 해석해 기본 시작 위치를 쓴다).
pub fn load_state() -> State {
    let path = state_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return State::default();
    };
    match toml::from_str(&text) {
        Ok(state) => state,
        Err(_) => {
            recover_corrupted(&path);
            State::default()
        }
    }
}

/// `state.toml`을 원자적으로 쓴다(임시 파일 → rename) — 쓰는 도중 앱이
/// 죽거나 전원이 나가도 절반만 쓰인 파일이 남지 않는다.
pub fn save_state(state: &State) -> std::io::Result<()> {
    ensure_home_dir()?;
    let path = state_path();
    let text = toml::to_string_pretty(state).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 앱을 켤 때 첫 탭이 열릴 폴더(DEV-031): 홈 → 없으면 실행 위치 → 그마저
/// 없으면 파일시스템 루트.
pub fn start_dir() -> PathBuf {
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(home_var)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `SYNCSHELL_HOME`은 프로세스 전역 상태라, cargo test의 기본 스레드 병렬
    /// 실행에서 이 모듈의 테스트끼리 서로 값을 덮어쓸 수 있다 — 뮤텍스로 이
    /// 모듈의 테스트를 순차 실행되게 강제한다(다른 모듈 테스트와는 무관해
    /// 전체 스위트 속도에 영향 없음). zsh를 실제로 띄우는 테스트도 잡는다 —
    /// 띄울 때 통합 파일을 `SYNCSHELL_HOME` 아래에 쓰기 때문(BUG-003).
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_syncshell_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("syncshell-session-test-{}-{}", std::process::id(), fastrand_ish()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: ENV_LOCK이 이 모듈 안에서 SYNCSHELL_HOME을 건드리는 모든
        // 테스트를 순차 실행되게 만들어서, 여기 있는 동안 다른 스레드가 동시에
        // 이 환경변수를 읽거나 쓸 일이 없다.
        unsafe {
            std::env::set_var("SYNCSHELL_HOME", &dir);
        }
        let result = f(&dir);
        unsafe {
            std::env::remove_var("SYNCSHELL_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
        result
    }

    // 테스트 파일명 충돌 방지용 — 진짜 랜덤 크레이트를 새로 끌어오긴 아까워서
    // 시간 기반으로 대충 섞는다(이 용도엔 암호학적 무작위성이 필요 없다).
    fn fastrand_ish() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn syncshell_home_respects_override() {
        with_syncshell_home(|dir| {
            assert_eq!(syncshell_home(), dir);
        });
    }

    #[test]
    fn load_config_missing_file_returns_default() {
        with_syncshell_home(|_| {
            assert_eq!(load_config(), Config::default());
        });
    }

    #[test]
    fn config_round_trips_through_toml_and_preserves_comments_untouched() {
        with_syncshell_home(|dir| {
            let text = "# 이 주석은 보존돼야 한다\ndefault_shell = \"/bin/bash\"\n";
            std::fs::write(dir.join("config.toml"), text).unwrap();

            let config = load_config();
            assert_eq!(config.default_shell.as_deref(), Some("/bin/bash"));

            // load_config는 절대 config.toml에 쓰지 않는다 — 다시 읽어도 주석이 그대로.
            let after = std::fs::read_to_string(dir.join("config.toml")).unwrap();
            assert!(after.contains("이 주석은 보존돼야 한다"), "config.toml을 건드림 — 사람이 쓴 주석이 사라지면 안 됨");
        });
    }

    /// DEV-010 Test plan: "설정 파일 손상 시 기본값으로 복구". 깨진 TOML을
    /// 읽으면 패닉 없이 기본값을 돌려주고, 손상본은 `.bak`로 남아야 한다.
    #[test]
    fn corrupted_config_recovers_to_default_and_backs_up() {
        with_syncshell_home(|dir| {
            std::fs::write(dir.join("config.toml"), "this is not valid toml {{{").unwrap();

            let config = load_config();
            assert_eq!(config, Config::default());
            assert!(dir.join("config.toml.bak").exists(), "손상본이 .bak로 안 남음");
        });
    }

    #[test]
    fn state_round_trips_window_and_ui() {
        with_syncshell_home(|_| {
            let state = State {
                window: Some(WindowState { x: 10.0, y: 20.0, width: 1400.0, height: 800.0 }),
                ui: UiState {
                    theme: "light".to_string(),
                    view: "icons".to_string(),
                    layout: "stacked".to_string(),
                    ui_scale: Some(1.2),
                    term_font_size: Some(14.0),
                    preview_pane: true,
                },
            };
            save_state(&state).unwrap();

            let loaded = load_state();
            assert_eq!(loaded, state);
        });
    }

    /// 쓰기가 원자적인지 — 임시 파일이 최종 이름으로 rename되고, 임시 파일
    /// 자체는 안 남아야 한다.
    #[test]
    fn save_state_does_not_leave_temp_file_behind() {
        with_syncshell_home(|dir| {
            save_state(&State::default()).unwrap();
            assert!(dir.join("state.toml").exists());
            assert!(!dir.join("state.toml.tmp").exists());
        });
    }

    /// 예전 버전이 남긴 state.toml(`[ui]` 없음, 탭 목록 있음)도 손상으로 취급하지
    /// 않고 읽혀야 한다 — 창 크기는 살리고 탭 목록은 무시(DEV-031).
    #[test]
    fn old_state_with_tabs_still_loads() {
        with_syncshell_home(|dir| {
            std::fs::write(
                dir.join("state.toml"),
                "active_tab = 0\n\n[[tabs]]\npath = \"/tmp\"\n\n[window]\nx = 1.0\ny = 2.0\nwidth = 900.0\nheight = 600.0\n",
            )
            .unwrap();
            let state = load_state();
            assert!(!dir.join("state.toml.bak").exists(), "예전 형식을 손상으로 취급함");
            assert_eq!(state.window.map(|w| w.width), Some(900.0), "예전 형식의 창 크기를 못 읽음");
            assert_eq!(state.ui, UiState::default());
            save_state(&state).unwrap();
            let text = std::fs::read_to_string(dir.join("state.toml")).unwrap();
            assert!(!text.contains("tabs"), "다시 저장했는데 탭 목록이 남음: {text}");
        });
    }

    #[test]
    fn save_favorites_keeps_other_settings_and_comments() {
        with_syncshell_home(|dir| {
            let original = "# 내 설정\ndefault_shell = \"/bin/zsh\" # 주석\n";
            std::fs::write(dir.join("config.toml"), original).unwrap();
            let favs = vec![PathBuf::from("/tmp"), PathBuf::from("/Users/x/한글 폴더")];
            save_favorites(&favs).unwrap();

            let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
            assert!(text.starts_with(original), "기존 내용·주석이 바뀜:\n{text}");
            let config = load_config();
            assert_eq!(config.favorites, favs);
            assert_eq!(config.default_shell.as_deref(), Some("/bin/zsh"));

            // 다시 쓰면 교체(중복으로 쌓이지 않음), 비우기도 가능
            save_favorites(&favs[..1]).unwrap();
            assert_eq!(load_config().favorites, favs[..1].to_vec());
            save_favorites(&[]).unwrap();
            assert!(load_config().favorites.is_empty());
            assert!(!dir.join("config.toml.tmp").exists());
        });
    }

    #[test]
    fn save_favorites_creates_missing_config_and_refuses_broken_one() {
        with_syncshell_home(|dir| {
            save_favorites(&[PathBuf::from("/tmp")]).unwrap();
            assert_eq!(load_config().favorites, vec![PathBuf::from("/tmp")]);

            let broken = "favorites = [ \"/tmp\"\n";
            std::fs::write(dir.join("config.toml"), broken).unwrap();
            assert!(save_favorites(&[]).is_err(), "깨진 파일을 덮어씀");
            assert_eq!(std::fs::read_to_string(dir.join("config.toml")).unwrap(), broken);
        });
    }

    #[test]
    fn corrupted_state_recovers_to_default_and_backs_up() {
        with_syncshell_home(|dir| {
            std::fs::write(dir.join("state.toml"), "not valid toml at all }}}").unwrap();

            let state = load_state();
            assert_eq!(state, State::default());
            assert!(dir.join("state.toml.bak").exists());
        });
    }

    #[test]
    fn ensure_home_dir_creates_directory() {
        with_syncshell_home(|dir| {
            std::fs::remove_dir_all(dir).ok(); // with_syncshell_home이 미리 만들어둔 걸 지워서 "없는 상태"부터 시작
            assert!(!dir.exists());
            ensure_home_dir().unwrap();
            assert!(dir.is_dir());
        });
    }

    #[test]
    fn start_dir_is_existing_home() {
        let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let dir = start_dir();
        assert!(dir.is_dir());
        if let Some(home) = std::env::var_os(home_var).map(PathBuf::from).filter(|p| p.is_dir()) {
            assert_eq!(dir, home, "홈이 있는데 다른 곳에서 시작함");
        }
    }
}
