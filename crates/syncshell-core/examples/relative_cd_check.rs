// 상대경로 cd 주입이 실제 PowerShell에서도 정확히 동작하는지 확인.
use std::path::PathBuf;
use std::time::{Duration, Instant};
use syncshell_core::sync::SyncState;
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("powershell.exe", 100, 24, || {})?;
    let mut sync = SyncState::new();
    pump_for(&mut session, 900);

    // 절대경로로 C:\Windows 로 먼저 이동(첫 이동이라 상대경로 계산 기준이 없음)
    let cmd = sync.user_navigated(PathBuf::from(r"C:\Windows"));
    session.write_input(cmd.as_bytes())?;
    pump_for(&mut session, 600);

    // 자식 폴더로 이동 — 상대경로("System32")로 주입되어야 함
    let cmd = sync.user_navigated(PathBuf::from(r"C:\Windows\System32"));
    println!("자식 이동 명령: {cmd:?}");
    assert_eq!(cmd, "cd \"System32\"\r");
    session.write_input(cmd.as_bytes())?;
    pump_for(&mut session, 600);
    println!("자식 이동 후 cwd: {:?}", session.cwd);
    let child_ok = session.cwd.as_deref() == Some(std::path::Path::new(r"C:\Windows\System32"));

    // 부모로 이동 — "cd .."로 주입되어야 함
    let cmd = sync.user_navigated(PathBuf::from(r"C:\Windows"));
    println!("부모 이동 명령: {cmd:?}");
    assert_eq!(cmd, "cd ..\r");
    session.write_input(cmd.as_bytes())?;
    pump_for(&mut session, 600);
    println!("부모 이동 후 cwd: {:?}", session.cwd);
    let parent_ok = session.cwd.as_deref() == Some(std::path::Path::new(r"C:\Windows"));

    let ok = child_ok && parent_ok;
    println!("\n결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
