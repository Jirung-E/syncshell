// macOS 이식 확인용: 실제 zsh를 TerminalSession으로 띄워서 cd 후 session.cwd가
// 정확히 갱신되는지 헤드리스로 검증한다(osc7_check.rs의 zsh 대응).
// 실행: cargo run -p syncshell-core --example zsh_osc7_check
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn check(session: &TerminalSession, label: &str, want: &str, ok: &mut bool) {
    let got = session.cwd.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    let pass = got == want;
    println!("[{label}] cwd = {got:?}  기대값 = {want:?}  {}", if pass { "PASS" } else { "FAIL" });
    if !pass {
        *ok = false;
    }
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("/bin/zsh", 80, 24, || {})?;
    let mut ok = true;

    // portable-pty는 cwd를 명시하지 않으면 $HOME을 기본값으로 쓴다
    // (osc7_check.rs의 %USERPROFILE% 코멘트와 동일한 이유 — cmdbuilder.rs의
    // cwd.or(home)). 실제 zsh 스타트업(oh-my-zsh 등)이 느릴 수 있어 여유 있게 기다린다.
    let home = std::env::var("HOME")?;
    pump_for(&mut session, 3000);
    check(&session, "초기($HOME 기본값)", &home, &mut ok);

    session.write_input(b"cd /tmp\n")?;
    pump_for(&mut session, 400);
    check(&session, "cd /tmp", "/tmp", &mut ok);

    session.write_input(b"cd /\n")?;
    pump_for(&mut session, 400);
    check(&session, "cd / (루트)", "/", &mut ok);

    let kr_dir = std::path::PathBuf::from("/tmp/syncshell-테스트 폴더 100%#1");
    std::fs::create_dir_all(&kr_dir)?;
    session.write_input(format!("cd \"{}\"\n", kr_dir.display()).as_bytes())?;
    pump_for(&mut session, 400);
    check(&session, "cd <한글+공백+특수문자 경로>", &kr_dir.display().to_string(), &mut ok);
    std::fs::remove_dir_all(&kr_dir).ok();

    session.write_input(b"cd /usr\n")?;
    session.write_input(b"cd /bin\n")?;
    pump_for(&mut session, 500);
    check(&session, "연속 cd (응답 대기 없이)", "/bin", &mut ok);

    println!("\n전체 결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
