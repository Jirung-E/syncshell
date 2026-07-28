// "ls 결과가 잘리고 창을 늘려도 안 돌아온다" 재현: 넓은 출력을 만든 뒤 아주 작은
// 크기로 resize했다가 다시 크게 되돌리면, alacritty_terminal의 Term::resize가
// 좁아졌을 때 내용을 파괴적으로 잘라내고 다시 넓혀도 복구가 안 되는지 확인한다.
// 가설: 첫 프레임에 위젯 rect가 일시적으로 아주 작게(혹은 0) 측정되면서
// session.resize(작은 값)이 호출되어 이런 손실이 생긴다.
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
    // 넓게(120컬럼) 스폰해서 여러 컬럼짜리 ls 결과가 한 줄에 다 들어가게 한다.
    let mut session = TerminalSession::spawn("powershell.exe", 120, 30, || {})?;
    pump_for(&mut session, 900);

    session.write_input(b"Get-ChildItem C:\\Windows | Format-Wide -Column 6\r\n")?;
    pump_for(&mut session, 700);

    println!("=== resize 전 (120컬럼) ===");
    for l in 0..8 {
        println!("[{l}] {:?}", row_text(&session, l, 120));
    }

    // 가설 재현: 첫 프레임에 위젯 rect가 일시적으로 아주 작게 측정된 상황을 흉내낸다.
    println!("\n--- resize(1,1) 흉내 (MIN_COLS/MIN_ROWS로 clamp됨) ---");
    session.resize(1, 1);
    pump_for(&mut session, 100);

    println!("\n--- 다시 120x30으로 복구 ---");
    session.resize(120, 30);
    pump_for(&mut session, 300);

    println!("\n=== resize(1,1) 왕복 후 (120컬럼로 복구했음) ===");
    for l in 0..8 {
        println!("[{l}] {:?}", row_text(&session, l, 120));
    }

    // 훨씬 완만한 범위(120 -> 60 -> 120) — 실제 창 크기 조절과 비슷한 시나리오
    session.write_input(b"Get-ChildItem C:\\Windows | Format-Wide -Column 6\r\n")?;
    pump_for(&mut session, 700);
    println!("\n--- 완만한 리사이즈: 120 -> 60 -> 120 ---");
    session.resize(60, 30);
    pump_for(&mut session, 200);
    session.resize(120, 30);
    pump_for(&mut session, 200);
    println!("\n=== 완만한 리사이즈 왕복 후 ===");
    for l in 0..8 {
        println!("[{l}] {:?}", row_text(&session, l, 120));
    }

    Ok(())
}
