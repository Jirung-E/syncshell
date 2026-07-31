// BUG-001 후속: `while ($true) { Start-Sleep -Milliseconds 50 }`만 한 번의
// CTRL_C_EVENT로 안 잡히는 게 확정적인 실패인지 경합(flake)인지 확인하고,
// 이벤트 전송 후 콘솔을 떼기까지의 대기 시간이 신뢰도에 영향을 주는지 본다.
//
// 대기 시간은 SYNCSHELL_CTRLC_SETTLE_MS 환경변수로 pty.rs가 읽는다.
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use syncshell_core::terminal::TerminalSession;

fn screen(session: &TerminalSession) -> String {
    let cols = session.term.columns();
    let rows = session.term.screen_lines();
    let grid = session.term.grid();
    (0..rows as i32)
        .map(|l| {
            (0..cols)
                .map(|c| grid[Point::new(Line(l), Column(c))].c)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn pump_for(session: &mut TerminalSession, ms: u64) {
    for _ in 0..(ms / 20) {
        session.pump();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn probe(marker: &str) -> anyhow::Result<bool> {
    let mut session = TerminalSession::spawn("powershell.exe", 120, 30, || {})?;
    pump_for(&mut session, 2000);
    session.write_input(b"while ($true) { Start-Sleep -Milliseconds 50 }\r")?;
    pump_for(&mut session, 2000);

    session.send_interrupt()?;
    pump_for(&mut session, 2000);

    session.write_input(format!("echo {marker}\r").as_bytes())?;
    pump_for(&mut session, 2500);
    Ok(screen(&session).lines().any(|l| l.trim() == marker))
}

fn main() -> anyhow::Result<()> {
    const N: usize = 5;
    let settle = std::env::var("SYNCSHELL_CTRLC_SETTLE_MS").unwrap_or_else(|_| "(기본)".into());
    let mut ok_count = 0;
    for i in 0..N {
        if probe(&format!("FLAKE-{i}"))? {
            ok_count += 1;
        }
    }
    let report = format!("settle={settle} — {N}회 중 {ok_count}회 성공\n");
    let path = std::env::temp_dir().join("syncshell-ctrlc-flake.txt");
    // 이어붙여서 여러 설정의 결과를 한 파일에서 비교한다.
    let prev = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, format!("{prev}{report}"))?;
    Ok(())
}
