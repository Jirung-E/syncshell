// DEV-003 Test plan 확인용(zsh 대응): "대량 출력 시 UI가 멈추지 않음"을 렌더링
// 계층 없이 헤드리스로 근사 검증한다 — pump() 루프가 대량 출력을 끝까지 소비하고
// (멈추거나 무한정 뒤처지지 않고) content_version이 계속 진행되는지 확인한다.
// 실제 위젯 렌더링(프레임 드랍 등)은 GUI로 직접 봐야 확정된다.
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("/bin/zsh", 120, 30, || {})?;
    let deadline = Instant::now() + Duration::from_millis(700);
    while Instant::now() < deadline {
        session.pump();
        std::thread::sleep(Duration::from_millis(20));
    }

    // cargo build 규모의 대량 출력을 흉내낸다 — /usr 아래 파일 경로 수천 줄.
    session.write_input(b"find /usr -type f 2>/dev/null | head -5000\n")?;

    let start = Instant::now();
    let mut last_version = session.content_version();
    let mut stalled_for;
    let mut last_progress_at = Instant::now();
    let hard_deadline = start + Duration::from_secs(10);
    loop {
        session.pump();
        let now_version = session.content_version();
        if now_version != last_version {
            last_version = now_version;
            last_progress_at = Instant::now();
        }
        stalled_for = last_progress_at.elapsed();
        // 프롬프트가 다시 뜬 신호: 몇 프레임 동안 조용하면(=출력이 끝났다고 봄) 종료.
        if stalled_for > Duration::from_millis(400) && start.elapsed() > Duration::from_millis(200) {
            break;
        }
        if Instant::now() > hard_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = start.elapsed();

    println!("대량 출력(5000줄) 처리 소요 시간: {elapsed:?}");
    println!("마지막 진행 이후 정지 시간: {stalled_for:?}");
    println!("exited = {}", session.exited);

    // "멈추지 않음"의 근사 기준: 하드 데드라인(10초) 안에 끝났고, 셸이 죽지 않았어야 한다.
    let ok = elapsed < Duration::from_secs(10) && !session.exited;
    println!("\n결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
