// TR-005 Test plan 확인용: 실제 TerminalSession + SyncState로 양방향 동기화를
// 헤드리스로 검증한다. (탐색기 렌더링은 없음 — FsView 대신 explorer_moves 벡터로
// "탐색기가 어디로 옮겨졌을지"만 기록해서 확인한다.)
// 실행: cargo run -p syncshell-core --example sync_check
use std::path::PathBuf;
use std::time::{Duration, Instant};
use syncshell_core::sync::SyncState;
use syncshell_core::terminal::TerminalSession;

fn pump_for(session: &mut TerminalSession, sync: &mut SyncState, explorer_moves: &mut Vec<PathBuf>, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        session.pump();
        if let Some(cwd) = session.take_cwd_change() {
            if let Some(target) = sync.terminal_cwd_changed(cwd) {
                explorer_moves.push(target);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn eq_path_ci(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

fn main() -> anyhow::Result<()> {
    let mut session = TerminalSession::spawn("powershell.exe", 80, 24, || {})?;
    let mut sync = SyncState::new();
    let mut explorer_moves: Vec<PathBuf> = Vec::new();
    let mut ok = true;

    // 1) 초기 동기화 — 터미널의 기본 cwd(%USERPROFILE%)로 탐색기가 맞춰져야 한다
    //    (TR-004 댓글에서 밝힌 FsView(current_dir) vs 터미널(%USERPROFILE%) 초기 불일치가
    //    첫 OSC7 감지로 자연히 해소되는지 확인).
    pump_for(&mut session, &mut sync, &mut explorer_moves, 1050);
    let userprofile = PathBuf::from(std::env::var("USERPROFILE")?);
    let pass = explorer_moves.last().map(|p| eq_path_ci(p, &userprofile)).unwrap_or(false);
    println!(
        "[1. 초기 동기화] explorer_moves 마지막 = {:?}  기대값 = {:?}  {}",
        explorer_moves.last(),
        userprofile,
        if pass { "PASS" } else { "FAIL" }
    );
    ok &= pass;

    // 2) 탐색기 → 터미널: "사용자가 클릭"을 시뮬레이션. 에코가 탐색기를 다시 안 옮겨야 함.
    explorer_moves.clear();
    let target = PathBuf::from(r"C:\Windows");
    let cmd = sync.user_navigated(target.clone());
    session.write_input(cmd.as_bytes())?;
    pump_for(&mut session, &mut sync, &mut explorer_moves, 850);
    let pass = session.cwd.as_deref().map(|p| eq_path_ci(p, &target)).unwrap_or(false);
    println!("[2. 탐색기→터미널] session.cwd = {:?}  {}", session.cwd, if pass { "PASS" } else { "FAIL" });
    ok &= pass;
    // take_cwd_change()로 "실제로 바뀐 값"만 받으므로 에코는 완전히 억제되어야 한다
    // (처음엔 PSReadLine 이중 렌더링 탓으로 오판했으나, 진짜 원인은 session.cwd를
    // 매 프레임 재통지하던 레이스였다 — take_cwd_change 도입으로 근본 수정됨).
    let pass_no_echo = explorer_moves.is_empty();
    println!(
        "[2b. 에코 억제] explorer_moves(비어있어야 함) = {:?}  {}",
        explorer_moves,
        if pass_no_echo { "PASS" } else { "FAIL" }
    );
    ok &= pass_no_echo;

    // 3) 터미널 → 탐색기: 사용자가 터미널에 직접 cd (sync.user_navigated 거치지 않고 raw write)
    explorer_moves.clear();
    session.write_input(b"cd ..\r\n")?;
    pump_for(&mut session, &mut sync, &mut explorer_moves, 850);
    // 드라이브 루트는 osc7 디코더가 이제 "C:"가 아니라 "C:\"로 복원한다(TR-006에서
    // 발견한 D: 드라이브 버그 수정 — 드라이브 상대 경로와 드라이브 루트를 구분).
    let pass = explorer_moves.last().map(|p| p.to_string_lossy().to_string()) == Some(r"C:\".to_string());
    println!("[3. 터미널→탐색기] explorer_moves = {:?}  {}", explorer_moves, if pass { "PASS" } else { "FAIL" });
    ok &= pass;

    // 4) 한글+공백 경로 양방향 왕복
    let kr_dir = std::env::temp_dir().join("syncshell-tr005-테스트 폴더");
    std::fs::create_dir_all(&kr_dir)?;
    explorer_moves.clear();
    let cmd = sync.user_navigated(kr_dir.clone());
    session.write_input(cmd.as_bytes())?;
    pump_for(&mut session, &mut sync, &mut explorer_moves, 850);
    let echo_ok = explorer_moves.is_empty();
    let pass = session.cwd.as_deref() == Some(kr_dir.as_path()) && echo_ok;
    println!(
        "[4. 한글+공백 왕복] session.cwd = {:?}  explorer_moves(비어있어야 함) = {:?}  {}",
        session.cwd,
        explorer_moves,
        if pass { "PASS" } else { "FAIL" }
    );
    ok &= pass;
    std::fs::remove_dir_all(&kr_dir).ok();

    // 5) 연속 클릭(A → B, 응답 대기 없이) — 최종 상태(B)에 정확히 안착해야 한다.
    //    (sync.rs의 rapid_double_click_always_converges_to_final_state 회귀 테스트를
    //    실제 PTY로 재현)
    explorer_moves.clear();
    let cmd_a = sync.user_navigated(PathBuf::from(r"C:\Users"));
    session.write_input(cmd_a.as_bytes())?;
    let cmd_b = sync.user_navigated(PathBuf::from(r"C:\Windows"));
    session.write_input(cmd_b.as_bytes())?;
    pump_for(&mut session, &mut sync, &mut explorer_moves, 1050);
    let pass = session
        .cwd
        .as_deref()
        .map(|p| eq_path_ci(p, std::path::Path::new(r"C:\Windows")))
        .unwrap_or(false);
    println!(
        "[5. 연속 클릭 → 최종 안착] session.cwd(최종) = {:?}  explorer_moves(경로) = {:?}  {}",
        session.cwd,
        explorer_moves,
        if pass { "PASS" } else { "FAIL" }
    );
    ok &= pass;

    println!("\n전체 결과: {}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
