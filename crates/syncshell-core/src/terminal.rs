use crate::osc7::{powershell_injection_script, Osc7Scanner};
use crate::pty::Pty;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::Processor;
use anyhow::Result;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// PSReadLine 등이 새 입력을 처리하기 직전에 이전 상태 프롬프트를 한 번 더
/// redraw하는 경우가 있어(TR-005/TR-006에서 관찰), OSC7 감지값을 곧바로 반영하지
/// 않고 이 시간 동안 조용해질 때까지 기다린 뒤 확정한다. 실사용에서 탐색기가
/// 옛 폴더로 깜빡였다 새 폴더로 다시 옮겨가는 현상으로 나타났던 것을 없앤다.
const CWD_DEBOUNCE: Duration = Duration::from_millis(150);

enum PtyEvent {
    Data(Vec<u8>),
    /// PTY 읽기에서 EOF(Ok(0))를 받았다 — 셸 프로세스가 종료됐다는 뜻.
    Exited,
}

struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// PTY + alacritty_terminal 상태머신을 함께 묶어서 관리한다.
/// UI 프레임워크는 모른다 — repaint 요청은 생성 시 넘겨준 콜백으로만 한다.
pub struct TerminalSession {
    pub term: Term<VoidListener>,
    /// OSC7으로 감지된 셸의 현재 cwd. architecture 규칙: 주 방식은 OSC7,
    /// 폴링 폴백은 예광탄 범위 밖(TR-004 "생략" 참고).
    /// **동기화 로직에서는 이 필드를 직접 읽지 말고 반드시 `take_cwd_change()`를 쓸 것**
    /// (표시용으로 그냥 읽는 건 안전 — 값이 바뀌었을 때만 한 번 통지받아야 하는
    /// 소비자에게만 문제가 된다. 아래 cwd_dirty 설명 참고).
    pub cwd: Option<PathBuf>,
    /// 마지막 `take_cwd_change()` 이후 `cwd`가 실제로 바뀐 적이 있으면 true.
    cwd_dirty: bool,
    /// 셸 프로세스가 종료됐으면 true(예: 사용자가 `exit` 입력). 한 번 true가 되면
    /// 되돌아가지 않는다 — UI는 이걸 보고 앱을 닫는 등 반응해야 한다. 이전에는
    /// 이 신호가 아예 없어서 셸이 죽어도 터미널 패널이 멈춘 화면인 채로 방치됐다
    /// (TR-006 피드백: "exit 치면 앱이 종료되는게 아니라 쉘만 정지되어버림").
    pub exited: bool,
    /// 사용자가 터미널에 뭔가 입력했지만 아직 Enter로 제출하지 않았을 수 있다.
    /// 탐색기 클릭으로 인한 cd 주입이 이 상태일 때 끼어들면, 사용자가 타이핑
    /// 중이던 줄 한가운데(또는 뒤)에 주입 문자열이 섞여 들어가 명령이 깨진다
    /// (TR-006 피드백: "탭 자동완성 후 엔터 치면 다른 명령이 따라 들어와서 실패함").
    /// `write_keyboard_input()`으로 들어온 실제 사용자 타이핑만 이 플래그를
    /// 갱신한다 — 동기화가 주입하는 `write_input()` 호출은 관여하지 않는다.
    pending_user_input: bool,
    /// 터미널 화면 내용이 바뀔 때마다(= PTY 바이트를 실제로 처리할 때마다)
    /// 증가한다. UI가 "지난번 본 뒤로 화면이 바뀌었나"를 O(1)로 판단하는 데 쓴다 —
    /// 예: 폰트 폴백 사전 스캔은 화면 전체를 훑는 비용(8000셀 기준 프레임당
    /// 0.5~1ms 실측)이 있어서, 안 바뀐 프레임에는 아예 건너뛴다.
    content_version: u64,
    processor: Processor,
    osc7: Osc7Scanner,
    /// 아직 디바운스 중인 cwd 감지값 — 이 시각으로부터 CWD_DEBOUNCE가 지나야 `cwd`에 반영된다.
    pending_cwd: Option<(PathBuf, Instant)>,
    on_data: Arc<dyn Fn() + Send + Sync>,
    pty: Pty,
    rx: mpsc::Receiver<PtyEvent>,
    cols: u16,
    rows: u16,
}

impl TerminalSession {
    pub fn spawn(shell: &str, cols: u16, rows: u16, on_data: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let on_data: Arc<dyn Fn() + Send + Sync> = Arc::new(on_data);
        // -NoLogo: 저작권 배너를 안 띄운다. 배너 + 뒤이어 "타이핑되듯" 들어가는
        // 초기화 스크립트(아래)가 겹쳐 보여서 첫 실행 화면이 뭔가 잘못된 것처럼
        // (예: "엔터가 이미 쳐진 것 같다") 보였던 것의 원인 중 하나였다(TR-006 피드백).
        let args: &[&str] = if shell.eq_ignore_ascii_case("powershell.exe") {
            &["-NoLogo"]
        } else {
            &[]
        };
        let (mut pty, mut reader) = Pty::spawn(shell, args, cols, rows)?;

        // DEV-004 결론: $PROFILE은 건드리지 않고, PTY 기동 직후 지연 없이
        // "타이핑하듯" 스크립트를 써넣는다. PowerShell 외 셸은 예광탄 범위 밖(생략).
        if shell.eq_ignore_ascii_case("powershell.exe") {
            let mut script = powershell_injection_script().as_bytes().to_vec();
            // 실제 키보드 Enter는 \r만 보낸다(terminal_widget.rs 참고) — 여기서 \r\n을
            // 쓰면 \r로 제출된 직후 남는 \n이 빈 줄에서 또 한 번의 Enter처럼 처리돼
            // PSReadLine이 헷갈려하는 것으로 확인됨(스폰 직후 화면에 ">>" 연속줄
            // 프롬프트가 떠 있던 원인 — \r\n → \r로 바꾸자 재현했던 아티팩트가
            // 사라짐. TR-006 "첫 실행시 엔터가 이미 쳐진 것 같다" 피드백).
            script.push(b'\r');
            let _ = pty.write(&script);
        }

        let (tx, rx) = mpsc::channel::<PtyEvent>();
        let reader_on_data = on_data.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(PtyEvent::Exited);
                        reader_on_data();
                        break;
                    }
                    Ok(n) => {
                        if tx.send(PtyEvent::Data(buf[..n].to_vec())).is_err() {
                            break;
                        }
                        reader_on_data();
                    }
                    Err(_) => {
                        let _ = tx.send(PtyEvent::Exited);
                        reader_on_data();
                        break;
                    }
                }
            }
        });

        let size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        let term = Term::new(TermConfig::default(), &size, VoidListener);

        Ok(Self {
            term,
            cwd: None,
            cwd_dirty: false,
            exited: false,
            pending_user_input: false,
            content_version: 0,
            processor: Processor::new(),
            osc7: Osc7Scanner::new(),
            pending_cwd: None,
            on_data,
            pty,
            rx,
            cols,
            rows,
        })
    }

    /// 도착한 바이트를 전부 term + osc7 스캐너에 밀어넣는다. 뭔가 처리했으면 true.
    /// osc7 감지는 alacritty_terminal과 별개로 같은 바이트를 태핑한다(DEV-004 댓글 #1 참고 —
    /// alacritty_terminal은 OSC7을 처리하지 않고 조용히 버리므로 필터링 없이 그대로 먹여도 안전).
    ///
    /// cwd 감지값은 곧바로 `self.cwd`에 반영하지 않고 `pending_cwd`에 잠깐 담아둔다 —
    /// CWD_DEBOUNCE 동안 더 새로운 감지가 없으면 그때 확정한다(위 CWD_DEBOUNCE 설명 참고).
    pub fn pump(&mut self) -> bool {
        // ConPTY는 자식 종료 시 read() EOF를 신뢰성 있게 주지 않는다(실측 확인,
        // pty.rs의 has_exited() 참고) — PtyEvent::Exited(아래 루프)는 보조 경로일 뿐,
        // 실제로는 이 폴링이 종료 감지의 주 경로다.
        if !self.exited && self.pty.has_exited() {
            self.exited = true;
        }

        let mut any = false;
        while let Ok(event) = self.rx.try_recv() {
            any = true;
            let data = match event {
                PtyEvent::Data(data) => data,
                PtyEvent::Exited => {
                    self.exited = true;
                    continue;
                }
            };
            if let Some(latest) = self.osc7.feed(&data).into_iter().last() {
                // 우리 프롬프트 함수는 매 프롬프트 렌더링마다 OSC7을 찍으므로,
                // 이 감지 자체가 "새 프롬프트가 떴다 = 이전 줄이 어떤 식으로든
                // 끝났다"는 신호이기도 하다(제출/취소(Ctrl+C)/에러 전부 포함).
                // 그래서 여기서 pending_user_input도 같이 내린다 — Enter가 아닌
                // 다른 방식(Ctrl+C 등)으로 줄이 끝났을 때도 놓치지 않기 위함.
                self.pending_user_input = false;
                self.pending_cwd = Some((latest, Instant::now()));
                let on_data = self.on_data.clone();
                thread::spawn(move || {
                    thread::sleep(CWD_DEBOUNCE + Duration::from_millis(20));
                    on_data();
                });
            }
            for byte in data {
                self.processor.advance(&mut self.term, byte);
            }
            self.content_version = self.content_version.wrapping_add(1);
        }

        if let Some((path, detected_at)) = &self.pending_cwd {
            if detected_at.elapsed() >= CWD_DEBOUNCE {
                if self.cwd.as_ref() != Some(path) {
                    self.cwd = Some(path.clone());
                    self.cwd_dirty = true;
                }
                self.pending_cwd = None;
            }
        }

        any
    }

    /// cwd가 마지막 호출 이후 실제로 바뀌었으면 새 값을 한 번만 돌려준다.
    ///
    /// **동기화 로직(SyncState)에 넘길 값은 반드시 이걸로 얻어야 한다.** `session.cwd`를
    /// 직접 매 프레임 읽어서 넘기면, 아직 안 바뀐 옛 값이 계속 재통지되고
    /// `SyncState`가 그걸 "다른 값"(자신의 낙관적 업데이트와 비교해서)으로 오인해
    /// 탐색기가 잠깐 옛 폴더로 튀는 버그가 생긴다 — TR-006에서 실사용 중 발견,
    /// 처음엔 PSReadLine의 이중 렌더링 탓으로 오판했었다(원시 PTY 레벨 재현으로
    /// 중복 이벤트가 없음을 확인하고서야 진짜 원인이 이 레이스였음을 알아냄).
    pub fn take_cwd_change(&mut self) -> Option<PathBuf> {
        if self.cwd_dirty {
            self.cwd_dirty = false;
            self.cwd.clone()
        } else {
            None
        }
    }

    /// 동기화 등이 프로그램적으로 주입하는 입력(예: 탐색기 클릭으로 인한 cd 명령).
    /// `pending_user_input` 상태에 영향을 주지 않는다 — 실제 사용자 타이핑이
    /// 아니기 때문. 주입 전에 `has_pending_user_input()`으로 안전한지 먼저 확인할 것.
    pub fn write_input(&mut self, data: &[u8]) -> Result<()> {
        self.pty.write(data)
    }

    /// 실제 키보드 입력 경로(`TerminalWidget::show`)에서만 호출한다. 순수 Enter(`\r`
    /// 단독)가 아니면 "아직 제출 안 한 입력이 있을 수 있다"로 표시하고, 순수
    /// Enter면 제출됐다고 보고 플래그를 내린다.
    pub fn write_keyboard_input(&mut self, data: &[u8]) -> Result<()> {
        self.pending_user_input = data != b"\r";
        self.pty.write(data)
    }

    /// 지금 탐색기 클릭으로 인한 cd를 주입하면 사용자가 타이핑 중인 줄과 섞일 수
    /// 있는지. true면 호출부는 주입을 미루고 이 값이 false가 될 때까지 기다려야 한다.
    pub fn has_pending_user_input(&self) -> bool {
        self.pending_user_input
    }

    /// Ctrl+C 처리. 두 경로를 **모두** 태워야 실제 터미널처럼 동작한다(BUG-001):
    ///
    /// - `0x03` 바이트: 셸이 프롬프트에서 줄을 읽고 있을 때 그 줄을 취소한다
    ///   (PSReadLine이 `^C`를 찍는다). 콘솔 이벤트로는 이게 안 된다.
    /// - 콘솔 `CTRL_C_EVENT`: 실행 중인 명령을 끊는다. 0x03으로는 이게 안 된다 —
    ///   명령 실행 중엔 셸이 stdin을 읽지 않기 때문(`pty.rs::send_interrupt` 참고).
    ///
    /// 콘솔 이벤트 전송이 실패해도 0x03은 이미 나갔으므로 줄 취소는 계속 동작한다 —
    /// 그래서 실패를 치명적으로 다루지 않고 삼킨다.
    pub fn send_interrupt(&mut self) -> Result<()> {
        self.pending_user_input = false;
        self.pty.write(&[0x03])?;
        let _ = self.pty.send_interrupt();
        Ok(())
    }

    /// 셸 프로세스의 PID(진단·인터럽트 경로 확인용).
    pub fn child_process_id(&self) -> Option<u32> {
        self.pty.child_process_id()
    }

    /// 화면 내용이 바뀔 때마다 증가하는 값. 값이 지난번과 같으면 그리드 내용도
    /// 그대로라는 뜻이므로, 화면 전체를 훑는 작업을 통째로 건너뛸 수 있다.
    pub fn content_version(&self) -> u64 {
        self.content_version
    }

    /// 아주 좁은 컬럼 수로 줄어들면 alacritty_terminal이 기존 스크롤백의 넓은 줄들을
    /// 전부 그 좁은 폭으로 리플로우해야 하는데, 줄 수가 폭발적으로 늘어나 수 GB
    /// 메모리 할당 후 크래시하는 것을 실측으로 확인했다(cols=1일 때 ~5GB 할당 시도,
    /// STATUS_STACK_BUFFER_OVERRUN으로 크래시). egui가 첫 프레임에 위젯 크기를
    /// 일시적으로 아주 작게 보고하는 경우가 있어(레이아웃이 아직 안 정착함) 이게
    /// 실사용 중 트리거될 수 있다 — 그래서 호출부(UI)가 얼마나 작은 값을 넘기든
    /// 여기서 안전한 최소값으로 강제한다.
    const MIN_COLS: u16 = 20;
    const MIN_ROWS: u16 = 4;

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(Self::MIN_COLS);
        let rows = rows.max(Self::MIN_ROWS);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        self.term.resize(size);
        let _ = self.pty.resize(cols, rows);
    }
}
