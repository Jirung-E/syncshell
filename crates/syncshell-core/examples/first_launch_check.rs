// "첫 실행시 엔터가 쳐져있는거 같음" 재현/확인: 스폰 직후 화면에 실제로 뭐가
// 찍히는지 grid 전체를 덤프해서 배너/에코가 얼마나 지저분하게 보이는지 확인한다.
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn dump_grid(session: &TerminalSession, rows: i32) {
    let content = session.term.renderable_content();
    let cursor = content.cursor.point;
    let mut lines: Vec<String> = vec![String::new(); rows as usize];
    for indexed in content.display_iter {
        let l = indexed.point.line.0;
        if l >= 0 && l < rows {
            lines[l as usize].push(indexed.cell.c);
        }
    }
    for (i, line) in lines.iter().enumerate() {
        let marker = if i as i32 == cursor.line.0 { " <- 커서 줄" } else { "" };
        println!("[{i:>2}] {:?}{}", line.trim_end(), marker);
    }
    println!("커서 위치: line={} col={}", cursor.line.0, cursor.column.0);
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("powershell.exe", 100, 24, || {})?;
    pump_for(&mut session, 900);

    println!("=== 스폰 900ms 후 화면 상태 (-NoLogo 적용됨) ===");
    dump_grid(&session, 10);
    Ok(())
}
