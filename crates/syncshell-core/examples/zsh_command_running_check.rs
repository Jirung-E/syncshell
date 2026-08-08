// 명령 실행 중 cd 주입을 막아야 하는 이유를 검증(zsh): Enter를 눌러 명령을 제출한
// 순간부터 다음 프롬프트가 뜨기 전까지 `is_command_running()`이 true를 유지하고,
// 프롬프트가 돌아오면 false로 내려가는지 실제 셸로 확인한다.
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("/bin/zsh", 80, 24, || {})?;
    pump_for(&mut session, 700);

    let mut ok = true;
    let check = |label: &str, got: bool, want: bool, ok: &mut bool| {
        let pass = got == want;
        println!("[{label}] is_command_running={got} (기대값={want}) {}", if pass { "PASS" } else { "FAIL" });
        if !pass {
            *ok = false;
        }
    };

    check("실행 전", session.is_command_running(), false, &mut ok);

    // 2초짜리 명령을 제출한다. 실제 위젯(terminal_widget.rs)은 Enter를 다른
    // 글자와 분리된 별도 프레임/호출로 보낸다 — `command_running`은 정확히
    // "입력이 `\r` 단독인 호출"만 보고 표시를 켜므로, 텍스트와 Enter를 나눠서
    // 써야 실제 사용 패턴과 맞는다.
    session.write_keyboard_input(b"sleep 2")?;
    session.write_keyboard_input(b"\r")?;
    pump_for(&mut session, 100);
    check("Enter 직후(아직 실행 중이어야 함)", session.is_command_running(), true, &mut ok);

    pump_for(&mut session, 900);
    check("1초 경과(여전히 실행 중이어야 함)", session.is_command_running(), true, &mut ok);

    // sleep이 끝나고 새 프롬프트가 뜰 때까지 기다린다.
    pump_for(&mut session, 1600);
    check("명령 종료 후(새 프롬프트 — running 아니어야 함)", session.is_command_running(), false, &mut ok);

    println!("\n전체 결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
