// HOME/END/Delete가 PSReadLine까지 도달해서 실제로 커서를 옮기는지 헤드리스 확인.
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

    // "World" 입력 → Home → "Hello " 입력 → 커서가 맨 앞이었다면 "Hello World"가 된다
    session.write_input(b"World")?;
    pump_for(&mut session, 200);
    session.write_input(b"\x1b[H")?; // Home
    pump_for(&mut session, 200);
    session.write_input(b"Hello ")?;
    pump_for(&mut session, 300);
    let after_home = current_line(&session);
    println!("Home 후: {after_home:?}");
    let home_ok = after_home.ends_with("Hello World");

    // End → "!" 입력 → 맨 끝에 붙어야 함
    session.write_input(b"\x1b[F")?; // End
    pump_for(&mut session, 200);
    session.write_input(b"!")?;
    pump_for(&mut session, 300);
    let after_end = current_line(&session);
    println!("End 후:  {after_end:?}");
    let end_ok = after_end.ends_with("Hello World!");

    // Home → Delete → 맨 앞 글자(H)가 지워져야 함
    session.write_input(b"\x1b[H")?;
    pump_for(&mut session, 200);
    session.write_input(b"\x1b[3~")?; // Delete
    pump_for(&mut session, 300);
    let after_delete = current_line(&session);
    println!("Delete 후: {after_delete:?}");
    let delete_ok = after_delete.ends_with("ello World!");

    let ok = home_ok && end_ok && delete_ok;
    println!(
        "\nHome: {}  End: {}  Delete: {}  전체: {}",
        if home_ok { "PASS" } else { "FAIL" },
        if end_ok { "PASS" } else { "FAIL" },
        if delete_ok { "PASS" } else { "FAIL" },
        if ok { "PASS" } else { "FAIL" }
    );
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
