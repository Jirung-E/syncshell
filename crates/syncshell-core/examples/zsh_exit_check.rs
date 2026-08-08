// DEV-002 Test plan 확인용(zsh 대응): 셸에 exit을 입력하면 TerminalSession.exited가
// true가 되는지, 그리고 자식 프로세스가 좀비로 안 남는지(OS에 물어봐서) 확인한다.
// 실행: cargo run -p syncshell-core --example zsh_exit_check
use std::process::Command;
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `ps -o state=`로 프로세스 상태를 물어본다. 완전히 사라졌으면 빈 문자열,
/// 좀비로 남아있으면 상태 문자에 'Z'가 포함된다.
fn process_state(pid: u32) -> String {
    Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("/bin/zsh", 80, 24, || {})?;
    pump_for(&mut session, 800);
    println!("exit 전: exited = {}", session.exited);
    assert!(!session.exited);

    let pid = session.child_process_id().expect("pid를 못 얻음");
    println!("자식 pid = {pid}, 상태 = {:?}", process_state(pid));

    session.write_input(b"exit\n")?;
    pump_for(&mut session, 700);
    println!("exit 후: exited = {}", session.exited);

    // has_exited()가 try_wait()로 이미 자식을 reap했어야 한다 — 조금 더 기다려도
    // 상태가 그대로면(빈 문자열 = 완전히 사라짐) 좀비가 아니라는 뜻.
    std::thread::sleep(Duration::from_millis(200));
    let state = process_state(pid);
    println!("정리 후 프로세스 상태 = {:?} (빈 문자열이어야 정상 — 좀비 아님)", state);

    let ok = session.exited && !state.contains('Z');
    println!("\n결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
