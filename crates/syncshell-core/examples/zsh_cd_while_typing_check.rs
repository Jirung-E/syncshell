// 실사용 버그 리포트 확인(zsh): "터미널에 명령어를 입력해둔 채로 탐색기에서
// 위치를 바꾸고 엔터를 치면(입력해뒀던 명령을 실행하면) 이전 위치에서 명령이
// 실행됨" — 그리고 뒤이은 요청("기존 입력되어있던 내용을 다음 폴더로 그대로
// 가지고가게 할 수는 없냐"). lib.rs::Tab::inject_cd의 동작(zsh처럼
// supports_line_prefix_insert()가 true인 셸에서는 Ctrl+A로 줄 맨 앞으로 이동한
// 뒤 "cd 새경로; "를 끼워 넣어, 타이핑하던 내용을 지우지 않고 새 위치에서
// 이어서 실행되게 함)을 core 레벨에서 그대로 흉내내 검증한다.
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn full_screen_text(session: &TerminalSession, cols: usize) -> String {
    use alacritty_terminal::index::{Column, Line, Point};
    let grid = session.term.grid();
    let mut out = String::new();
    for l in 0..24i32 {
        for c in 0..cols {
            out.push(grid[Point::new(Line(l), Column(c))].c);
        }
        out.push('\n');
    }
    out
}

fn main() -> anyhow::Result<()> {
    let base = std::env::temp_dir().join(format!("syncshell-cdtyping-{}", std::process::id()));
    let sub = base.join("target_dir");
    std::fs::create_dir_all(&sub)?;

    let mut session = TerminalSession::spawn("/bin/zsh", 220, 24, || {})?;
    pump_for(&mut session, 700);
    assert!(session.supports_line_prefix_insert(), "zsh는 줄 앞 삽입을 지원해야 함 — 테스트 전제가 깨짐");

    // 사용자가 "echo hello"를 타이핑하지만 아직 Enter는 안 눌렀다.
    session.write_keyboard_input(b"echo hello")?;
    pump_for(&mut session, 100);
    assert!(session.has_pending_user_input(), "타이핑 중인데 pending_user_input이 false — 테스트 전제가 깨짐");

    // lib.rs::Tab::inject_cd의 새 동작을 그대로 흉내낸다: Ctrl+A로 줄 맨 앞으로,
    // "cd 새경로; "를 끼워 넣는다(제출 안 함 — 사용자가 직접 Enter를 눌러야 함).
    session.write_input(b"\x01")?;
    session.write_input(format!("cd \"{}\"; ", sub.display()).as_bytes())?;
    pump_for(&mut session, 300);

    // 아직 제출 전이라 아무것도 실행되면 안 된다 — 여전히 타이핑 중인 상태.
    assert!(session.has_pending_user_input(), "삽입 직후에도 아직 제출 전이어야 함(우리가 \\r을 안 보냈으므로)");

    // 사용자가 이제 Enter를 눌러 "cd 새경로; echo hello" 전체를 제출한다.
    session.write_keyboard_input(b"\r")?;
    pump_for(&mut session, 500);

    let screen = full_screen_text(&session, 220);
    let hello_ran = screen.lines().any(|l| l.trim() == "hello");
    let shows_new_dir = screen.contains(&sub.display().to_string());
    println!("'echo hello'가 실행됨(새 위치에서): {hello_ran} (true여야 정상 — 타이핑한 내용이 살아남아 실행됨)");
    println!("새 위치(target_dir)가 화면에 나옴: {shows_new_dir} (true여야 정상)");
    println!("\n=== 최종 화면 ===\n{screen}");

    std::fs::remove_dir_all(&base).ok();

    let ok = hello_ran && shows_new_dir;
    println!("\n전체 결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
