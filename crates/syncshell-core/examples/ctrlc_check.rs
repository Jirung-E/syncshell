// ConPTY에 0x03(ETX)을 써넣는 것만으로 PowerShell에서 실행 중인 명령이 실제로
// 끊기는지 헤드리스로 확인한다. UI 레이어를 전부 걷어내고 PTY만 남긴 최소 재현 —
// DEV-012에서 Ctrl+C 동작을 정리하다가 인터럽트가 안 먹는 것을 발견해서, 그게
// 위젯 쪽 문제인지 PTY/ConPTY 레벨의 문제인지 가르려고 만들었다.
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
    let steps = ms / 20;
    for _ in 0..steps {
        session.pump();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("powershell.exe", 120, 30, || {})?;
    pump_for(&mut session, 1500);
    println!("=== 정착 후 ===\n{}\n", screen(&session));

    // 끝나지 않는 명령을 띄운다.
    session.write_input(b"while ($true) { Start-Sleep -Milliseconds 50 }\r")?;
    pump_for(&mut session, 1500);
    println!("=== 무한루프 실행 중 ===\n{}\n", screen(&session));

    // 0x03만 써넣는다 — 이게 Ctrl+C로 해석되는지가 이 실험의 전부.
    println!(">>> 0x03 전송");
    session.write_input(&[0x03])?;
    pump_for(&mut session, 2000);
    println!("=== 0x03 후 ===\n{}\n", screen(&session));

    // 인터럽트가 먹었으면 프롬프트가 돌아왔을 테니 새 명령이 실행된다.
    session.write_input(b"Write-Host 'AFTER-INTERRUPT'\r")?;
    pump_for(&mut session, 2000);
    let after = screen(&session);
    println!("=== 새 명령 시도 후 ===\n{after}\n");

    let running_case = after.contains("AFTER-INTERRUPT");
    if running_case {
        println!("[명령 실행 중] 0x03으로 인터럽트 동작함");
    } else {
        println!("[명령 실행 중] 0x03을 보내도 인터럽트 안 됨 — 셸이 루프에 갇혀 있음");
    }

    // ---- 두 번째 경우: 프롬프트에서 입력 줄을 치던 중의 0x03 ----
    // 이건 실행 중인 명령을 끊는 것과는 다른 경로다(PSReadLine이 stdin에서 직접
    // 읽는다). 여기서도 안 먹으면 0x03이 셸에 아예 도달하지 못한다는 뜻이고,
    // 여기서만 먹으면 "도달은 하는데 CTRL_C_EVENT로 승격되지 않는다"는 뜻이다.
    let mut session2 = TerminalSession::spawn("powershell.exe", 120, 30, || {})?;
    pump_for(&mut session2, 1500);
    session2.write_input(b"Write-Host 'SHOULD-BE-CANCELLED'")?; // Enter 없이 타이핑만
    pump_for(&mut session2, 800);
    println!("\n=== 입력 줄 타이핑 후(Enter 안 침) ===\n{}\n", screen(&session2));

    session2.write_input(&[0x03])?;
    pump_for(&mut session2, 1200);
    println!("=== 프롬프트에서 0x03 후 ===\n{}\n", screen(&session2));

    // 줄이 취소됐으면 그 명령은 실행되지 않은 채 새 프롬프트가 떠 있어야 한다.
    session2.write_input(b"Write-Host 'PROMPT-ALIVE'\r")?;
    pump_for(&mut session2, 2000);
    let after2 = screen(&session2);
    println!("=== 이후 새 명령 ===\n{after2}\n");

    let line_cancelled = after2.contains("PROMPT-ALIVE") && !after2.contains("SHOULD-BE-CANCELLED\r");
    println!(
        "[프롬프트 입력 중] 0x03 후 새 명령 실행됨: {} / 취소한 명령이 실행돼버림: {}",
        after2.contains("PROMPT-ALIVE"),
        after2.contains("SHOULD-BE-CANCELLED") && after2.lines().any(|l| l.trim() == "SHOULD-BE-CANCELLED")
    );
    let _ = line_cancelled;

    println!("\n---- 요약 ----");
    println!("실행 중인 명령 끊기: {}", if running_case { "동작" } else { "안 됨" });
    Ok(())
}
