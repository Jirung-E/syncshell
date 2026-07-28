// "화면 밖으로 나가는 출력이 아랫줄로 안 가고 짤림" 재현: PowerShell 자체
// 포맷팅(Get-ChildItem 등의 컬럼 정렬)과 무관하게, 폭보다 긴 순수 텍스트 한 줄이
// alacritty_terminal 모델 레벨에서 실제로 자동 줄바꿈(WRAPLINE)되는지 확인한다.
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn row_text_and_wrapped(session: &TerminalSession, line: i32, cols: usize) -> (String, bool) {
    let grid = session.term.grid();
    let mut s = String::new();
    let mut wrapped = false;
    for c in 0..cols {
        let cell = &grid[Point::new(Line(line), Column(c))];
        s.push(cell.c);
        if c == cols - 1 && cell.flags.contains(Flags::WRAPLINE) {
            wrapped = true;
        }
    }
    (s.trim_end().to_string(), wrapped)
}

fn main() -> anyhow::Result<()> {
    let cols: u16 = 40;
    let mut session = TerminalSession::spawn("powershell.exe", cols, 24, || {})?;
    pump_for(&mut session, 900);

    // 40컬럼짜리 터미널에 80글자짜리 순수 텍스트 한 줄을 출력 — PowerShell의
    // 스마트 포맷팅이 아니라 Write-Host 리터럴이라 wrap 여부는 순전히
    // 터미널(alacritty_terminal)의 몫이다.
    let long_line = "A".repeat(80);
    session.write_input(format!("Write-Host \"{long_line}\"\r").as_bytes())?;
    pump_for(&mut session, 1200);

    println!("=== 40컬럼 터미널, 화면 전체(0~23행) ===");
    let mut a_row: Option<i32> = None;
    for l in 0..24 {
        let (text, wrapped) = row_text_and_wrapped(&session, l, cols as usize);
        if !text.is_empty() {
            println!("[{l:>2}] len={:>3} wrapped={:5} {:?}", text.len(), wrapped, text);
        }
        if text.chars().filter(|&c| c == 'A').count() > 10 && a_row.is_none() {
            a_row = Some(l);
        }
    }

    let Some(first_a_row) = a_row else {
        println!("\n결과: FAIL — 'A' 반복 텍스트를 화면에서 아예 못 찾음");
        std::process::exit(1);
    };

    let (row0, row0_wrapped) = row_text_and_wrapped(&session, first_a_row, cols as usize);
    let a_count_row0 = row0.chars().filter(|&c| c == 'A').count();
    let ok_row0 = a_count_row0 == cols as usize && row0_wrapped;

    let (row1, _) = row_text_and_wrapped(&session, first_a_row + 1, cols as usize);
    let a_count_row1 = row1.chars().filter(|&c| c == 'A').count();
    let ok_row1 = a_count_row1 == 40; // 나머지 40개

    println!(
        "\n첫 A 줄=[{first_a_row}] row0: A={} wrapped={}  row1: A={}",
        a_count_row0, row0_wrapped, a_count_row1
    );
    let ok = ok_row0 && ok_row1;
    println!("결과: {}", if ok { "PASS (모델은 정상적으로 wrap함)" } else { "FAIL (모델 레벨에서부터 안 됨)" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
