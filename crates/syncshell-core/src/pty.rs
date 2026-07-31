use anyhow::Result;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Pty {
    pub fn spawn(shell: &str, args: &[&str], cols: u16, rows: u16) -> Result<(Self, Box<dyn Read + Send>)> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(shell);
        for a in args {
            cmd.arg(a);
        }
        let child = pair.slave.spawn_command(cmd)?;
        // slave 핸들은 자식 프로세스에 넘긴 뒤 여기서는 곧바로 drop —
        // 안 그러면 자식이 종료돼도 read()가 EOF를 못 받는다 (핸들 누수).
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        Ok((
            Self {
                master: pair.master,
                writer,
                child,
            },
            reader,
        ))
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// 실행 중인 명령에 인터럽트(Ctrl+C)를 보낸다.
    ///
    /// **PTY에 0x03을 써넣는 것으로는 이게 안 된다**(BUG-001, `examples/ctrlc_matrix.rs`로
    /// 실측). 0x03은 콘솔 입력 버퍼까지는 도달해서 셸이 프롬프트에서 줄을 읽고 있을
    /// 때는 동작하지만(PSReadLine이 `^C`를 찍고 줄을 버린다), **명령이 실행 중일 때는
    /// 셸이 stdin을 읽지 않는다** — 그때의 인터럽트는 콘솔 `CTRL_C_EVENT`로 와야 하고
    /// ConPTY는 입력 바이트를 그 이벤트로 승격해주지 않는다. powershell/cmd,
    /// 셸 내장 루프/네이티브 프로그램(ping) 네 조합 모두에서 재현했다.
    ///
    /// 호출자는 0x03 전송과 이 함수를 **둘 다** 해야 한다 — 프롬프트에서 줄을
    /// 취소하는 건 0x03 쪽이 담당한다(`TerminalSession::send_interrupt` 참고).
    ///
    /// # 알려진 한계
    /// `while ($true) { Start-Sleep -Milliseconds 50 }` 처럼 **아주 짧은 sleep을
    /// 도는 PowerShell 루프**는 한 번에 안 잡힐 때가 있다(실측 5회 중 4회 성공).
    /// 이벤트가 iteration 사이에 떨어지면 PowerShell이 놓치는 것으로, 콘솔 쪽
    /// 타이밍이 아니라 PowerShell 내부 경합이다. 한 번 더 누르면 잡힌다. 그 외 —
    /// 네이티브 프로그램(ping), cmd 루프, 순수 스핀 루프, 긴 단일 sleep,
    /// 파이프라인 — 은 전부 한 번에 잡힌다.
    #[cfg(windows)]
    pub fn send_interrupt(&mut self) -> Result<()> {
        let Some(pid) = self.child.process_id() else {
            anyhow::bail!("자식 프로세스 ID를 알 수 없습니다(이미 종료됨?)");
        };
        crate::interrupt::send_ctrl_c_to_console_of(pid)
    }

    /// Windows 외 플랫폼(2·3단계)에서는 프로세스 그룹에 SIGINT를 보내면 된다 —
    /// 지금은 대상 플랫폼이 아니라 미구현으로 두고, 호출부가 실패를 무시하게 한다.
    #[cfg(not(windows))]
    pub fn send_interrupt(&mut self) -> Result<()> {
        anyhow::bail!("이 플랫폼에서는 아직 인터럽트를 지원하지 않습니다")
    }

    /// 셸 프로세스의 PID.
    pub fn child_process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// 자식 프로세스가 종료됐는지 논블로킹으로 확인한다.
    ///
    /// **PTY 읽기의 EOF(Ok(0))로는 셸 종료를 감지할 수 없다** — ConPTY는 자식이
    /// 종료돼도 master 쪽 read가 EOF를 돌려주지 않는 것으로 실측 확인됨
    /// (`exit` 입력 후 5초를 기다려도 EOF가 안 옴). `Child::try_wait()`으로 직접
    /// 폴링하는 게 유일하게 신뢰할 수 있는 방법이다.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // 앱을 여러 번 껐다 켜며 테스트할 때 셸 프로세스가 좀비로 남지 않도록.
        let _ = self.child.kill();
    }
}
