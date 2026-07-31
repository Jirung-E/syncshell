// BUG-001 후속: 콘솔 CTRL_C_EVENT를 보내면 네이티브 프로그램(ping)과 cmd 루프는
// 끊기는데 **PowerShell 자체 스크립트 루프**만 안 끊긴다. 어떤 종류의 PowerShell
// 작업이 끊기고 안 끊기는지, 이벤트를 여러 번 보내면 달라지는지 좁힌다.
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

fn probe(busy_cmd: &str, marker: &str, repeats: usize) -> anyhow::Result<bool> {
    let mut session = TerminalSession::spawn("powershell.exe", 120, 30, || {})?;
    pump_for(&mut session, 2000);
    session.write_input(format!("{busy_cmd}\r").as_bytes())?;
    pump_for(&mut session, 2000);

    for _ in 0..repeats {
        session.send_interrupt()?;
        pump_for(&mut session, 700);
    }
    pump_for(&mut session, 1500);

    session.write_input(format!("echo {marker}\r").as_bytes())?;
    pump_for(&mut session, 2500);

    let s = screen(&session);
    Ok(s.lines().any(|l| l.trim() == marker))
}

fn main() -> anyhow::Result<()> {
    let cases: &[(&str, &str, usize)] = &[
        ("while ($true) { Start-Sleep -Milliseconds 50 }", "A-SLEEP-LOOP", 1),
        ("while ($true) { Start-Sleep -Milliseconds 50 }", "B-SLEEP-LOOP-X3", 3),
        ("while ($true) { }", "C-SPIN-LOOP", 1),
        ("Start-Sleep -Seconds 60", "D-LONG-SLEEP", 1),
        ("1..100000000 | ForEach-Object { $_ * 2 } | Out-Null", "E-PIPELINE", 1),
    ];

    let mut report = String::new();
    for (cmd, marker, repeats) in cases {
        let ok = probe(cmd, marker, *repeats)?;
        report.push_str(&format!(
            "{:52} 반복{} {}\n",
            cmd,
            repeats,
            if ok { "OK" } else { "실패" }
        ));
    }
    std::fs::write(std::env::temp_dir().join("syncshell-ctrlc-psloop.txt"), &report)?;
    Ok(())
}
