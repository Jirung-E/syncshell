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
