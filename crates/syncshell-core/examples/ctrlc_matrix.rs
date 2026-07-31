// BUG-001 원인 좁히기: 0x03이 안 먹는 게 PowerShell 특유의 문제인지, ConPTY
// 전반의 문제인지 가른다. 셸(powershell/cmd) × 대상(셸 내장 루프 / 네이티브 콘솔
// 프로그램)을 조합해 각각 인터럽트가 되는지 본다.
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

/// 셸을 띄우고, `busy_cmd`를 실행한 뒤 0x03을 보내고, 프롬프트가 살아 돌아오는지
/// `marker`를 출력해 확인한다.
fn probe(shell: &str, busy_cmd: &str, marker: &str) -> anyhow::Result<bool> {
    let mut session = TerminalSession::spawn(shell, 120, 30, || {})?;
    pump_for(&mut session, 2000);

    session.write_input(format!("{busy_cmd}\r").as_bytes())?;
    pump_for(&mut session, 2000);

    session.send_interrupt()?;
    pump_for(&mut session, 2500);

    let echo = format!("echo {marker}\r");
    session.write_input(echo.as_bytes())?;
    pump_for(&mut session, 2500);

    let s = screen(&session);
    // 마커가 "명령 줄"이 아니라 "출력"으로도 나타났는지 본다 — 즉 실제로 실행됐는지.
    let executed = s.lines().any(|l| l.trim() == marker);
    if !executed {
        println!("--- [{shell}] {busy_cmd} 실패 시 화면 ---\n{s}\n");
    }
    Ok(executed)
}

fn main() -> anyhow::Result<()> {
    let cases: &[(&str, &str, &str)] = &[
        // 셸 자체의 루프 (셸이 인터럽트를 처리해야 함)
        (
            "powershell.exe",
            "while ($true) { Start-Sleep -Milliseconds 50 }",
            "PS-LOOP-OK",
        ),
        ("cmd.exe", "for /l %i in (1,0,2) do @ping -n 2 127.0.0.1 >nul", "CMD-LOOP-OK"),
        // 네이티브 콘솔 프로그램 (자식 프로세스가 인터럽트를 받아야 함)
        ("powershell.exe", "ping -t 127.0.0.1", "PS-PING-OK"),
        ("cmd.exe", "ping -t 127.0.0.1", "CMD-PING-OK"),
    ];

    // send_interrupt()가 프로세스의 콘솔을 잠깐 떼었다 붙이므로, 이 예제 자신의
    // stdout이 도중에 사라진다. 결과는 파일로 남겨서 콘솔 상태와 무관하게 읽는다.
    let out_path = std::env::temp_dir().join("syncshell-ctrlc-matrix.txt");
    let mut report = String::new();

    let mut results = Vec::new();
    for (shell, cmd, marker) in cases {
        let ok = probe(shell, cmd, marker)?;
        report.push_str(&format!(
            "{:14} {:50} {}\n",
            shell,
            cmd,
            if ok { "OK" } else { "실패" }
        ));
        results.push((*shell, *cmd, ok));
    }

    let all_ok = results.iter().all(|(_, _, ok)| *ok);
    report.push_str(&format!(
        "\n전체: {}\n",
        if all_ok { "전부 인터럽트 동작" } else { "일부 실패" }
    ));
    std::fs::write(&out_path, &report)?;
    Ok(())
}
