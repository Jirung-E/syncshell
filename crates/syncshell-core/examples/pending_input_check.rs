// "탭 자동완성 후 엔터 치면 다른 명령이 따라 들어와서 실패함" 재현/검증.
// syncshell-ui의 큐잉 로직과 동일한 패턴을, TerminalSession의 공개 API
// (write_keyboard_input / has_pending_user_input / write_input)만으로 재현한다.
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn current_line(session: &TerminalSession) -> String {
    let content = session.term.renderable_content();
    let cursor_line = content.cursor.point.line;
    let mut s = String::new();
    for indexed in content.display_iter {
        if indexed.point.line == cursor_line {
            s.push(indexed.cell.c);
        }
    }
    s.trim_end().to_string()
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("powershell.exe", 120, 24, || {})?;
    pump_for(&mut session, 900);

    // 사용자가 "Get-Location"을 타이핑 중 (아직 Enter 안 침)
    session.write_keyboard_input(b"Get-Loc")?;
    pump_for(&mut session, 150);
    assert!(session.has_pending_user_input(), "타이핑 중인데 pending_user_input이 false");

    // 탭으로 자동완성 (여전히 제출 전)
    session.write_keyboard_input(b"\t")?;
    pump_for(&mut session, 200);
    println!("탭 자동완성 후 라인: {:?}", current_line(&session));
    assert!(session.has_pending_user_input(), "탭 자동완성 직후에도 아직 제출 전이어야 함");

    // 바로 이 타이밍에 탐색기 클릭이 들어왔다고 가정 — 큐에 쌓여야 하고,
    // 지금 당장 PTY에 쓰이면 안 된다.
    let mut pending_cd: Option<String> = Some("cd \"C:\\Windows\"\r".to_string());
    if session.has_pending_user_input() {
        println!("입력 중이라 cd 주입을 큐에 대기시킴 (아직 안 보냄)");
    } else {
        session.write_input(pending_cd.take().unwrap().as_bytes())?;
    }
    assert!(pending_cd.is_some(), "입력 중인데 즉시 주입돼버림 — 버그 재현 실패");

    // 사용자가 이제 Enter로 자기 명령을 제출
    session.write_keyboard_input(b"\r")?;
    pump_for(&mut session, 700);
    println!("사용자 명령 제출 후 cwd: {:?}", session.cwd);
    assert!(!session.has_pending_user_input(), "제출했는데 아직도 pending 상태");

    // 이제(그리고 오직 이제서야) 큐에 있던 cd를 흘려보낸다 — UI의 매 프레임 재확인과 동일
    if let Some(cmd) = pending_cd.take() {
        session.write_input(cmd.as_bytes())?;
    }
    pump_for(&mut session, 700);
    println!("대기하던 cd 처리 후 cwd: {:?}", session.cwd);

    let ok = session.cwd.as_deref() == Some(std::path::Path::new(r"C:\Windows"));
    println!("\n결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
