//! zsh 셸 통합 파일(BUG-003). OSC 7 훅을 PTY에 "타이핑하듯" 써넣으면 긴 함수
//! 정의가 에코돼 화면에 그대로 보인다. 그래서 zsh는 `ZDOTDIR`을 우리 폴더로
//! 바꿔 띄우고, 그 폴더의 시작 파일이 사용자 원래 시작 파일(`.zshenv`·`.zprofile`·
//! `.zshrc`)을 그대로 읽은 뒤 훅만 등록한다 — 사용자 파일은 건드리지 않는다.
//!
//! 파일은 `~/.syncshell/shell/zsh/`(DEV-010: 앱이 만드는 건 전부 여기)에 두고 셸을
//! 띄울 때마다 덮어쓴다(앱 업데이트로 내용이 바뀌어도 따라가게).

use crate::osc7::zsh_injection_script;
use crate::session::syncshell_home;
use std::path::PathBuf;

/// 사용자 원래 `ZDOTDIR`(있었다면)을 우리 `.zshenv`에 넘기는 환경변수.
pub const USER_ZDOTDIR_VAR: &str = "SYNCSHELL_USER_ZDOTDIR";

/// 사용자 `ZDOTDIR`로 되돌린다(원래 없었으면 unset → zsh가 `$HOME`을 쓴다).
const RESTORE_USER_ZDOTDIR: &str = r#"if [[ -n "$_syncshell_user_zdotdir_set" ]]; then ZDOTDIR="$_syncshell_user_zdotdir"; else unset ZDOTDIR; fi"#;

fn zshenv() -> String {
    format!(
        r#"# syncshell 셸 통합 — 앱이 셸을 띄울 때마다 덮어쓴다. 직접 고치지 마세요.
_syncshell_zdotdir="$ZDOTDIR"
if [[ -n "${{{var}+x}}" ]]; then _syncshell_user_zdotdir="${var}"; _syncshell_user_zdotdir_set=1; else _syncshell_user_zdotdir=""; _syncshell_user_zdotdir_set=""; fi
unset {var}
{restore}
[[ -r "${{ZDOTDIR:-$HOME}}/.zshenv" ]] && source "${{ZDOTDIR:-$HOME}}/.zshenv"
# 사용자 .zshenv가 ZDOTDIR을 정했으면 이후 파일도 그 위치에서 읽는다
if [[ -n "${{ZDOTDIR+x}}" ]]; then _syncshell_user_zdotdir="$ZDOTDIR"; _syncshell_user_zdotdir_set=1; fi
ZDOTDIR="$_syncshell_zdotdir"
"#,
        var = USER_ZDOTDIR_VAR,
        restore = RESTORE_USER_ZDOTDIR,
    )
}

fn zprofile() -> String {
    format!(
        r#"# syncshell 셸 통합 — 앱이 셸을 띄울 때마다 덮어쓴다. 직접 고치지 마세요.
{restore}
[[ -r "${{ZDOTDIR:-$HOME}}/.zprofile" ]] && source "${{ZDOTDIR:-$HOME}}/.zprofile"
ZDOTDIR="$_syncshell_zdotdir"
"#,
        restore = RESTORE_USER_ZDOTDIR,
    )
}

/// 마지막에 ZDOTDIR을 사용자 값으로 둔 채 끝낸다 — 로그인 셸의 `.zlogin`과 이 셸
/// 안에서 띄우는 하위 zsh는 사용자 위치를 그대로 쓴다.
fn zshrc() -> String {
    format!(
        r#"# syncshell 셸 통합 — 앱이 셸을 띄울 때마다 덮어쓴다. 직접 고치지 마세요.
{restore}
# /etc/zshrc는 우리 ZDOTDIR 기준으로 히스토리 파일을 잡는다 — 사용자 위치로 돌린다
[[ "$HISTFILE" == "$_syncshell_zdotdir/.zsh_history" ]] && HISTFILE="${{ZDOTDIR:-$HOME}}/.zsh_history"
[[ -r "${{ZDOTDIR:-$HOME}}/.zshrc" ]] && source "${{ZDOTDIR:-$HOME}}/.zshrc"
{hook}
unset _syncshell_zdotdir _syncshell_user_zdotdir _syncshell_user_zdotdir_set
"#,
        restore = RESTORE_USER_ZDOTDIR,
        hook = zsh_injection_script(),
    )
}

/// 통합 파일을 쓰고 그 폴더(`ZDOTDIR`로 넘길 값)를 돌려준다.
pub fn prepare_zsh() -> std::io::Result<PathBuf> {
    crate::session::ensure_home_dir()?;
    let dir = syncshell_home().join("shell").join("zsh");
    write_zsh_files(&dir)?;
    Ok(dir)
}

fn write_zsh_files(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(".zshenv"), zshenv())?;
    std::fs::write(dir.join(".zprofile"), zprofile())?;
    std::fs::write(dir.join(".zshrc"), zshrc())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_files_source_user_files_and_register_hook() {
        assert!(zshenv().contains(r#"source "${ZDOTDIR:-$HOME}/.zshenv""#));
        assert!(zshenv().contains(r#"${SYNCSHELL_USER_ZDOTDIR+x}"#));
        assert!(zprofile().contains(r#"source "${ZDOTDIR:-$HOME}/.zprofile""#));
        let rc = zshrc();
        let user = rc.find(r#"source "${ZDOTDIR:-$HOME}/.zshrc""#).expect("사용자 .zshrc를 안 읽음");
        let hook = rc.find("add-zsh-hook precmd _syncshell_osc7").expect("훅 등록이 없음");
        assert!(user < hook, "사용자 .zshrc보다 훅이 먼저 등록되면 사용자 설정이 훅을 지울 수 있음");
    }

    /// 실제 zsh로: 사용자 ZDOTDIR의 시작 파일을 읽고, 훅이 등록되고, 히스토리는
    /// 사용자 위치이며, 끝나면 ZDOTDIR이 사용자 값으로 돌아와 있어야 한다.
    #[test]
    #[cfg_attr(windows, ignore = "zsh 전용")]
    fn real_zsh_loads_user_files_through_integration_dir() {
        let base = std::env::temp_dir().join(format!("syncshell-zdot-{}", std::process::id()));
        let ours = base.join("ours");
        let user = base.join("user");
        write_zsh_files(&ours).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join(".zshenv"), "SYNC_ENV=env\n").unwrap();
        std::fs::write(user.join(".zshrc"), "SYNC_RC=rc\n").unwrap();

        let out = std::process::Command::new("/bin/zsh")
            .args(["-i", "-c", r#"print -r -- "$SYNC_ENV|$SYNC_RC|$HISTFILE|$ZDOTDIR|${SYNCSHELL_USER_ZDOTDIR-none}|$+functions[_syncshell_osc7]""#])
            .env("ZDOTDIR", &ours)
            .env(USER_ZDOTDIR_VAR, &user)
            .env("TERM_PROGRAM", "syncshell")
            .env_remove("TERM_SESSION_ID")
            .output()
            .expect("zsh 실행");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let line = stdout.lines().last().unwrap_or_default().to_string();
        std::fs::remove_dir_all(&base).ok();

        let f: Vec<&str> = line.split('|').collect();
        assert_eq!(f.len(), 6, "zsh 출력: {stdout}");
        assert_eq!((f[0], f[1]), ("env", "rc"), "사용자 .zshenv/.zshrc를 안 읽음: {stdout}");
        // macOS는 /etc/zshrc가 HISTFILE을 정한다 — 우리 폴더가 아니라 사용자 위치여야 함
        assert!(!f[2].starts_with(ours.to_str().unwrap()), "히스토리가 통합 폴더로 감: {stdout}");
        if cfg!(target_os = "macos") {
            assert_eq!(f[2], user.join(".zsh_history").to_str().unwrap());
        }
        assert_eq!(f[3], user.to_str().unwrap(), "ZDOTDIR이 사용자 값으로 안 돌아옴");
        assert_eq!(f[4], "none", "넘겨주기용 환경변수가 남음");
        assert_eq!(f[5], "1", "OSC 7 훅이 등록 안 됨");
    }
}
