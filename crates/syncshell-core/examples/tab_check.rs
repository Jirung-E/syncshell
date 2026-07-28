// Tab 자동완성 확인: 부분 입력 + 탭 바이트(0x09)를 실제 PowerShell에 보내고,
// PSReadLine이 반응해서 라인이 늘어나는지 렌더링된 그리드를 직접 읽어서 확인한다.
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 커서가 있는 줄의 텍스트를 읽어온다 (trailing 공백 제거).
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

    // C:\Windows 로 이동 — System32 등 유명한 하위 폴더가 있어 자동완성이 잘 걸린다.
    session.write_input(b"cd C:\\Windows\r\n")?;
    pump_for(&mut session, 600);

    let typed = "cd Sys";
    session.write_input(typed.as_bytes())?;
    pump_for(&mut session, 300);
    let before = current_line(&session);
    println!("탭 누르기 전 라인: {before:?}");

    session.write_input(b"\t")?;
    pump_for(&mut session, 400);
    let after = current_line(&session);
    println!("탭 누른 후 라인:   {after:?}");

    // C:\Windows 밑에는 System, System32, SystemApps 등 여러 Sys*로 시작하는 폴더가 있어
    // PSReadLine이 정확히 어느 걸 완성할지는 특정하지 않는다 — "system"을 포함하는
    // 방향으로 라인이 늘어나기만 하면 Tab이 셸까지 도달해 완성이 동작한다는 뜻.
    let ok = after.len() > before.len() && after.to_lowercase().contains("system");
    println!(
        "\n결과(Tab): {}  (라인이 늘어났고 'system'을 포함하면 PASS)",
        if ok { "PASS" } else { "FAIL" }
    );

    // Shift+Tab(CBT, \x1b[Z) — 다음 후보로 한 번 더 순환한 뒤, 역방향으로 돌아오면
    // 원래(첫 Tab 이후) 후보와 같아야 한다. TR-006 피드백: "shift+tab이 안 먹힘".
    session.write_input(b"\t")?; // 다음 후보로
    pump_for(&mut session, 300);
    let after_second_tab = current_line(&session);
    println!("탭 두 번째 후: {after_second_tab:?}");

    session.write_input(b"\x1b[Z")?; // Shift+Tab — 역방향
    pump_for(&mut session, 300);
    let after_shift_tab = current_line(&session);
    println!("Shift+Tab 후: {after_shift_tab:?}");

    let shift_tab_ok = after_shift_tab == after && after_second_tab != after;
    println!(
        "결과(Shift+Tab): {}  (두 번째 탭과 달랐다가, shift+tab으로 첫 후보로 돌아오면 PASS)",
        if shift_tab_ok { "PASS" } else { "FAIL" }
    );

    let ok = ok && shift_tab_ok;
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
