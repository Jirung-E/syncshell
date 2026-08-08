// DEV-002 Test plan 확인용(zsh 대응): 리사이즈 후에도 크래시 없이 화면이
// 정상적으로 복구되는지 확인한다(resize_loss_check.rs의 zsh 대응, 완화 버전).
use alacritty_terminal::index::{Column, Line, Point};
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn row_text(session: &TerminalSession, line: i32, cols: usize) -> String {
    let grid = session.term.grid();
    let mut s = String::new();
    for c in 0..cols {
        s.push(grid[Point::new(Line(line), Column(c))].c);
    }
    s.trim_end().to_string()
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("/bin/zsh", 120, 30, || {})?;
    pump_for(&mut session, 700);

    session.write_input(b"ls -1 /bin | head -20\n")?;
    pump_for(&mut session, 400);
    println!("=== resize 전 (120컬럼) ===");
    for l in 0..5 {
        println!("[{l}] {:?}", row_text(&session, l, 120));
    }

    // architecture 규칙 7이 다루는 극단 케이스: 아주 좁게 줄였다가 되돌린다.
    println!("\n--- resize(1,1) 흉내 (MIN_COLS/MIN_ROWS로 clamp됨) ---");
    session.resize(1, 1);
    pump_for(&mut session, 100);
    session.resize(120, 30);
    pump_for(&mut session, 300);
    println!("resize(1,1) 왕복 후 크래시 없음, exited={}", session.exited);

    // 완만한 리사이즈 왕복.
    session.write_input(b"ls -1 /bin | head -20\n")?;
    pump_for(&mut session, 400);
    session.resize(60, 30);
    pump_for(&mut session, 200);
    session.resize(120, 30);
    pump_for(&mut session, 300);
    println!("\n=== 완만한 리사이즈(120->60->120) 왕복 후 ===");
    for l in 0..5 {
        println!("[{l}] {:?}", row_text(&session, l, 120));
    }

    let ok = !session.exited;
    println!("\n결과: {}", if ok { "PASS (크래시/조기종료 없음)" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
