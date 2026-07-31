//! 실행 중인 명령에 Ctrl+C(콘솔 `CTRL_C_EVENT`)를 보내는 경로(BUG-001).
//!
//! # 왜 이게 필요한가
//! PTY에 `0x03`을 써넣는 것만으로는 **실행 중인 명령이 안 끊긴다**. 0x03은 콘솔
//! 입력 버퍼까지 도달해서 셸이 프롬프트에서 줄을 읽고 있을 때는 동작하지만
//! (PSReadLine이 `^C`를 찍고 줄을 버린다), 명령이 실행 중일 때는 셸이 stdin을
//! 읽지 않는다. 그때의 인터럽트는 콘솔 `CTRL_C_EVENT`로 와야 하는데 ConPTY는
//! 입력 바이트를 그 이벤트로 승격해주지 않는다. powershell/cmd × 셸 내장 루프/
//! 네이티브 프로그램(ping) 네 조합 모두에서 재현했다(`examples/ctrlc_matrix.rs`).
//!
//! # 왜 이 프로세스에서 직접 하는가
//! `GenerateConsoleCtrlEvent`는 **호출한 프로세스가 붙어 있는 콘솔**에만 보낼 수
//! 있어서, 대상의 pseudoconsole에 잠깐 붙었다 떼야 한다. 콘솔 붙임/뗌은 프로세스
//! 전역 상태라 별도 헬퍼 프로세스에 맡기고 싶었지만, **헬퍼에서는 이벤트가 셸에
//! 전달되지 않는다** — 헬퍼는 AttachConsole·GenerateConsoleCtrlEvent 모두 성공을
//! 보고하는데 셸은 멀쩡했다. 생성 플래그(없음 / DETACHED_PROCESS / CREATE_NO_WINDOW)와
//! 자기 보호 유무를 모두 바꿔봐도 동일했다. 셸을 실제로 띄운 프로세스에서 호출할
//! 때만 동작한다.
//!
//! # 제약 (중요)
//! 이 조작이 도는 동안 **같은 프로세스의 다른 PTY 세션이 영향을 받는다.** 테스트
//! 스위트로 실측했다 — 인터럽트를 태우는 테스트를 포함시키면 무관한 테스트 3~4개가
//! 매번 다르게 실패하고, 빼면 안정적으로 통과한다. 지금은 터미널이 하나뿐이라
//! 실사용에서 드러나지 않지만, **탭이 생기면(DEV-009) 반드시 다시 봐야 한다.**

/// 콘솔 조작은 프로세스 전역 상태다. 두 곳에서 동시에 들어오면 한쪽이 보호를 푸는
/// 순간 다른 쪽이 쏜 이벤트에 앱이 죽는다 — 직렬화해서 그 창을 없앤다.
#[cfg(windows)]
static CONSOLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `pid`가 붙어 있는 콘솔에 `CTRL_C_EVENT`를 보낸다.
///
/// 순서가 중요하다. 특히 자기 보호(`SetConsoleCtrlHandler(NULL, TRUE)`)를 이벤트
/// 발생보다 **먼저** 해야 한다 — 우리도 그 콘솔에 붙은 상태라, 보호가 늦으면
/// 우리가 쏜 Ctrl+C에 우리 프로세스가 죽는다(실측: 보호를 attach 뒤로 뒀다가
/// STATUS_CONTROL_C_EXIT로 종료됨).
#[cfg(windows)]
pub fn send_ctrl_c_to_console_of(pid: u32) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler, CTRL_C_EVENT,
    };

    let _guard = CONSOLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    unsafe {
        // 1. 우리 프로세스가 Ctrl+C를 무시하도록 먼저 막는다.
        SetConsoleCtrlHandler(None, 1);
        // 2. 기존 콘솔이 있으면 떼고 대상 콘솔에 붙는다.
        FreeConsole();
        let attached = AttachConsole(pid) != 0;
        // 3. 이벤트 발생. 그룹 0 = 이 콘솔에 붙은 모든 프로세스. CTRL_C를 특정
        //    그룹에만 보내는 건 Windows가 지원하지 않는다(호출은 성공하지만
        //    신호가 전달되지 않는다).
        let sent = attached && GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) != 0;
        // 4. 원상복구. 전달이 비동기라, 곧바로 보호를 풀면 아직 배달 중인 Ctrl+C가
        //    우리에게 도착해 **앱이 죽는다**(실측: 대기 0ms면 STATUS_CONTROL_C_EXIT).
        //    50ms는 여유값이고, Ctrl+C는 드문 조작이라 체감되지 않는다.
        if attached {
            std::thread::sleep(std::time::Duration::from_millis(50));
            FreeConsole();
        }
        SetConsoleCtrlHandler(None, 0);

        if !attached {
            anyhow::bail!("대상 콘솔에 연결하지 못했습니다 (pid {pid})");
        }
        if !sent {
            anyhow::bail!("CTRL_C_EVENT 전송에 실패했습니다");
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn send_ctrl_c_to_console_of(_pid: u32) -> anyhow::Result<()> {
    // 2·3단계에서는 `killpg(pgid, SIGINT)`로 훨씬 단순하게 된다.
    anyhow::bail!("이 플랫폼에서는 아직 인터럽트를 지원하지 않습니다")
}
