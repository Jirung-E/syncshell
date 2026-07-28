// TR-004 Test plan 확인용: 실제 PowerShell을 TerminalSession으로 띄워서
// cd 후 session.cwd가 정확히 갱신되는지 헤드리스로 검증한다.
// 실행: cargo run -p syncshell-core --example osc7_check
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
    let pass = got.eq_ignore_ascii_case(want);
    println!("[{label}] cwd = {got:?}  기대값 = {want:?}  {}", if pass { "PASS" } else { "FAIL" });
    if !pass {
        *ok = false;
    }
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("powershell.exe", 80, 24, || {})?;
    let mut ok = true;

    // portable-pty는 cwd를 명시하지 않으면 %USERPROFILE%을 기본값으로 쓴다
    // (cmdbuilder.rs의 current_directory(), cwd.or(home) — home = %USERPROFILE%).
    let userprofile = std::env::var("USERPROFILE")?;
    pump_for(&mut session, 950);
    check(&session, "초기(%USERPROFILE% 기본값)", &userprofile, &mut ok);

    session.write_input(b"cd $env:WINDIR\r\n")?;
    pump_for(&mut session, 650);
    check(&session, "cd $env:WINDIR", r"C:\Windows", &mut ok);

    session.write_input(b"cd ..\r\n")?;
    pump_for(&mut session, 650);
    // 드라이브 루트는 osc7 디코더가 "C:\"로 복원한다(TR-006에서 발견한 D: 드라이브
    // 버그 수정 — "C:"는 드라이브 루트가 아니라 드라이브 상대 경로라는 별개의 의미).
    check(&session, "cd ..", r"C:\", &mut ok);

    let kr_dir = std::env::temp_dir().join("syncshell-tr004-테스트 폴더");
    std::fs::create_dir_all(&kr_dir)?;
    session.write_input(format!("cd \"{}\"\r\n", kr_dir.display()).as_bytes())?;
    pump_for(&mut session, 650);
    check(&session, "cd <한글+공백 경로>", &kr_dir.display().to_string(), &mut ok);
    std::fs::remove_dir_all(&kr_dir).ok();

    // 응답을 기다리지 않고 연달아 cd — DEV-004에서 검증한 상황을 실제 TerminalSession으로 재현
    session.write_input(b"cd $env:TEMP\r\n")?;
    session.write_input(b"cd $env:WINDIR\r\n")?;
    pump_for(&mut session, 850);
    check(&session, "연속 cd (응답 대기 없이)", r"C:\Windows", &mut ok);

    println!("\n전체 결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
