// 셸에 exit을 입력하면 TerminalSession.exited가 true가 되는지 확인.
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("powershell.exe", 80, 24, || {})?;
    pump_for(&mut session, 800);
    println!("exit 전: exited = {}", session.exited);
    assert!(!session.exited);

    session.write_input(b"exit\r\n")?;
    pump_for(&mut session, 700);
    println!("exit 후: exited = {}", session.exited);

    println!("\n결과: {}", if session.exited { "PASS" } else { "FAIL" });
    if !session.exited {
        std::process::exit(1);
    }
    Ok(())
}
