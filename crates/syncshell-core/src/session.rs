//! 세션(탭/창) 복원과 앱 설정 저장(DEV-010).
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
//! └── state.toml     ← 앱이 덮어쓰는 것 (탭, 창 위치/크기)
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
}

/// 앱이 종료 시 덮어쓰는 상태.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct State {
    pub tabs: Vec<TabState>,
    pub active_tab: usize,
    pub window: Option<WindowState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabState {
    pub path: PathBuf,
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

/// 세션 복원 시 저장된 경로가 이제 없으면(폴더가 지워졌거나 이동함) 존재하는
/// 조상 폴더를 찾을 때까지 조용히 상위로 올라간다. 그마저 없으면(드라이브가
/// 통째로 사라짐 등) `fallback`을 쓴다.
pub fn resolve_existing_or_fallback(path: &Path, fallback: &Path) -> PathBuf {
    let mut candidate = path.to_path_buf();
    loop {
        if candidate.is_dir() {
            return candidate;
        }
        match candidate.parent() {
            Some(parent) => candidate = parent.to_path_buf(),
            None => return fallback.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SYNCSHELL_HOME`은 프로세스 전역 상태라, cargo test의 기본 스레드 병렬
    /// 실행에서 이 모듈의 테스트끼리 서로 값을 덮어쓸 수 있다 — 뮤텍스로 이
    /// 모듈의 테스트를 순차 실행되게 강제한다(다른 모듈 테스트와는 무관해
    /// 전체 스위트 속도에 영향 없음).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn state_round_trips_tabs_and_window() {
        with_syncshell_home(|_| {
            let state = State {
                tabs: vec![TabState { path: PathBuf::from("/tmp") }, TabState { path: PathBuf::from("/") }],
                active_tab: 1,
                window: Some(WindowState { x: 10.0, y: 20.0, width: 1400.0, height: 800.0 }),
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
    fn resolve_existing_or_fallback_walks_up_to_existing_ancestor() {
        let base = std::env::temp_dir().join(format!("syncshell-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let missing = base.join("deleted_child/deleted_grandchild");

        let resolved = resolve_existing_or_fallback(&missing, Path::new("/"));

        assert_eq!(resolved, base, "존재하지 않는 경로면 존재하는 조상까지 올라가야 함");
        std::fs::remove_dir_all(&base).ok();
    }

    /// 절대경로는 결국 파일시스템 루트("/", "C:\" 등)에 닿는데, 루트는 사실상
    /// 항상 존재해서 실제로는 fallback까지 갈 일이 드물다(그건
    /// walks_up_to_existing_ancestor 테스트가 이미 확인함). fallback 분기가
    /// 실제로 타지는 경우는 **상대경로**처럼 절대 루트에 안 닿는 경로 — 유닉스에서
    /// `Path::parent()`는 "foo" → `Some("")` → `None`으로 끝나고, 빈 경로는
    /// `is_dir()`가 false라 결국 조상을 못 찾는다.
    #[test]
    fn resolve_existing_or_fallback_uses_fallback_when_nothing_exists() {
        let bogus = PathBuf::from("does/not/exist/anywhere/xyz123");
        let fallback = std::env::temp_dir();

        let resolved = resolve_existing_or_fallback(&bogus, &fallback);

        assert_eq!(resolved, fallback);
    }
}
