use crate::osc7::{powershell_injection_script, zsh_injection_script, Osc7Scanner};
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

/// PTY 입출력 계측(BUG-006 진단). `take_io_stats()`로 꺼내면 0으로 돌아간다.
#[derive(Debug, Clone, Copy, Default)]
pub struct IoStats {
    /// PTY 쓰기 한 번에 걸린 가장 긴 시간 — UI 스레드에서 쓰므로, 이게 길면
    /// 쓰기가 막혀서 앱 전체가 멈추는 것이다.
    pub max_write: Duration,
    pub writes: u32,
    /// PTY에서 읽어 처리한 바이트 수와, 그 처리(VT 파싱)에 쓴 시간.
    pub bytes_read: u64,
    pub parse_time: Duration,
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
    /// 셸이 지금 명령을 실행 중인지 — Enter로 (빈 줄이 아닌) 명령을 제출한
    /// 순간부터 다음 프롬프트가 뜨기 전까지 true. `pending_user_input`과는
    /// 다른 신호다: Enter를 누르는 순간 `pending_user_input`은 곧바로 false가
    /// 되지만(더 이상 "제출 안 한 줄"이 아니므로), 셸은 그 명령을 아직 실행
    /// 중일 수 있다. 이 사이 구간에 cd를 곧장 주입하면 그 바이트가 PTY 입력
    /// 큐에 쌓였다가, 지금 실행 중인 프로그램이 stdin을 읽는 종류(REPL·페이저·
    /// 대화형 프롬프트 등)라면 엉뚱하게 그 프로그램에 들어가버릴 수 있다 —
    /// 그래서 cd 주입은 이 플래그도 확인해서 미뤄야 한다.
    command_running: bool,
    /// 이 셸이 Ctrl+A(0x01)를 "줄 맨 앞으로 이동"(이맥스 기본 키바인딩 —
    /// bash/zsh의 readline/zle 기본값)으로 해석하는지. true인 셸에서만 타이핑
    /// 중인 줄 앞에 cd를 안전하게 끼워 넣을 수 있다 — PowerShell(PSReadLine
    /// 기본 "Windows" 모드)은 Ctrl+A가 "전체 선택"이라, 그 상태에서 새로
    /// 타이핑하면 선택된(=사용자가 치던 전체 내용) 걸 지워버린다. 실측 확인
    /// (zsh: `\x01` 보낸 뒤 타이핑하면 정확히 줄 앞에 끼워짐).
    supports_line_prefix_insert: bool,
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
    io_stats: IoStats,
}

/// 셸 실행 파일 이름만으로 종류를 가른다(전체 경로가 와도 마지막 조각만 본다 —
/// 예: "/opt/homebrew/bin/zsh"도 "zsh"로 인식). 인자 선택과 초기화 스크립트 선택
/// 둘 다 여기 기준을 공유한다.
fn shell_basename(shell: &str) -> &str {
    std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
}

/// 플랫폼 기본 셸의 실행 파일 경로. Windows는 항상 PowerShell, 그 외에는 로그인
/// 셸(`$SHELL`)을 따르고 없으면 zsh로 폴백한다(macOS 10.15+ 기본 로그인 셸).
pub fn default_shell() -> String {
    #[cfg(windows)]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
    }
}

impl TerminalSession {
    pub fn spawn(shell: &str, cols: u16, rows: u16, on_data: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let on_data: Arc<dyn Fn() + Send + Sync> = Arc::new(on_data);
        let name = shell_basename(shell);
        let is_powershell = name.eq_ignore_ascii_case("powershell.exe");
        let is_zsh = name.eq_ignore_ascii_case("zsh");

        // -NoLogo: 저작권 배너를 안 띄운다. 배너 + 뒤이어 "타이핑되듯" 들어가는
        // 초기화 스크립트(아래)가 겹쳐 보여서 첫 실행 화면이 뭔가 잘못된 것처럼
        // (예: "엔터가 이미 쳐진 것 같다") 보였던 것의 원인 중 하나였다(TR-006 피드백).
        let args: &[&str] = if is_powershell { &["-NoLogo"] } else { &[] };
        let (mut pty, mut reader) = Pty::spawn(shell, args, cols, rows)?;

        // DEV-004 결론: $PROFILE은 건드리지 않고, PTY 기동 직후 지연 없이
        // "타이핑하듯" 스크립트를 써넣는다. PowerShell/zsh 외 셸은 예광탄 범위 밖(생략).
        let injection_script = if is_powershell {
            Some(powershell_injection_script())
        } else if is_zsh {
            Some(zsh_injection_script())
        } else {
            None
        };
        if let Some(script) = injection_script {
            let mut script = script.as_bytes().to_vec();
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
            command_running: false,
            // 지금은 zsh만 해당(이맥스 기본 키바인딩). bash를 나중에 정식
            // 지원하게 되면 마찬가지로 true여야 한다(같은 readline 계열).
            supports_line_prefix_insert: is_zsh,
            content_version: 0,
            processor: Processor::new(),
            osc7: Osc7Scanner::new(),
            pending_cwd: None,
            on_data,
            pty,
            rx,
            cols,
            rows,
            io_stats: IoStats::default(),
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
                // 새 프롬프트가 떴다 = 셸이 다시 입력을 기다리는 상태 = 방금
                // 실행 중이던 명령(있었다면)이 끝났다는 뜻이다.
                self.command_running = false;
                self.pending_cwd = Some((latest, Instant::now()));
                let on_data = self.on_data.clone();
                thread::spawn(move || {
                    thread::sleep(CWD_DEBOUNCE + Duration::from_millis(20));
                    on_data();
                });
            }
            let parse_start = Instant::now();
            self.io_stats.bytes_read += data.len() as u64;
            for byte in data {
                self.processor.advance(&mut self.term, byte);
            }
            self.io_stats.parse_time += parse_start.elapsed();
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
        self.timed_write(data)
    }

    /// 실제 키보드 입력 경로(`TerminalWidget::show`)에서만 호출한다. 순수 Enter(`\r`
    /// 단독)가 아니면 "아직 제출 안 한 입력이 있을 수 있다"로 표시하고, 순수
    /// Enter면 제출됐다고 보고 플래그를 내린다 — 그리고 셸이 그 명령을 실행하기
    /// 시작했다고 낙관적으로 표시한다(`command_running`, 실제 종료는 다음
    /// OSC7 프롬프트로 확인).
    pub fn write_keyboard_input(&mut self, data: &[u8]) -> Result<()> {
        if data == b"\r" {
            self.command_running = true;
        }
        self.pending_user_input = data != b"\r";
        self.timed_write(data)
    }

    /// PTY에 쓰고 걸린 시간을 계측한다(BUG-006 진단).
    fn timed_write(&mut self, data: &[u8]) -> Result<()> {
        let start = Instant::now();
        let result = self.pty.write(data);
        let elapsed = start.elapsed();
        self.io_stats.max_write = self.io_stats.max_write.max(elapsed);
        self.io_stats.writes += 1;
        result
    }

    /// 마지막으로 꺼낸 뒤부터 쌓인 입출력 계측값을 꺼내고 0으로 되돌린다.
    pub fn take_io_stats(&mut self) -> IoStats {
        std::mem::take(&mut self.io_stats)
    }

    /// 지금 탐색기 클릭으로 인한 cd를 주입하면 사용자가 타이핑 중인 줄과 섞일 수
    /// 있는지. true면 호출부는 주입을 미루고 이 값이 false가 될 때까지 기다려야 한다.
    pub fn has_pending_user_input(&self) -> bool {
        self.pending_user_input
    }

    /// 셸이 지금 명령을 실행 중인지(`command_running` 필드 설명 참고). cd 주입은
    /// 이것도 확인해야 한다 — 안 그러면 그 바이트가 지금 실행 중인 프로그램에
    /// (stdin을 읽는 종류라면) 잘못 들어갈 수 있다.
    pub fn is_command_running(&self) -> bool {
        self.command_running
    }

    /// `supports_line_prefix_insert` 필드 설명 참고 — Ctrl+A로 안전하게 줄 맨
    /// 앞에 텍스트를 끼워 넣을 수 있는 셸인지.
    pub fn supports_line_prefix_insert(&self) -> bool {
        self.supports_line_prefix_insert
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_basename_strips_path_and_keeps_only_file_name() {
        assert_eq!(shell_basename("zsh"), "zsh");
        assert_eq!(shell_basename("/bin/zsh"), "zsh");
        assert_eq!(shell_basename("/opt/homebrew/bin/zsh"), "zsh");
        assert_eq!(shell_basename("powershell.exe"), "powershell.exe");
    }

    /// 백슬래시 경로 구분은 Windows에서만 성립한다(`std::path::Path`가 플랫폼별로
    /// 다르게 구분자를 인식함 — 유닉스에서는 `\`가 그냥 평범한 문자다). 그래서 이
    /// 케이스는 Windows 빌드에서만 의미가 있다.
    #[test]
    #[cfg(windows)]
    fn shell_basename_strips_windows_backslash_path() {
        assert_eq!(
            shell_basename(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            "powershell.exe"
        );
    }

    fn pump_for(session: &mut TerminalSession, ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            session.pump();
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// 실제 zsh로 `command_running`/`pending_user_input`/`supports_line_prefix_insert`의
    /// 전체 수명주기를 확인한다 — 지금까지는 `examples/zsh_*.rs`로만 수동 확인했고
    /// `cargo test`로 자동 검증되는 테스트가 없었다.
    #[test]
    #[cfg_attr(windows, ignore = "이 테스트는 zsh 전용 — Windows에는 없음")]
    fn zsh_session_tracks_input_and_command_state_transitions() {
        let mut session = TerminalSession::spawn("/bin/zsh", 80, 24, || {}).expect("zsh spawn");
        pump_for(&mut session, 700);

        assert!(session.supports_line_prefix_insert(), "zsh는 줄 앞 삽입을 지원해야 함");
        assert!(!session.is_command_running(), "시작 직후인데 명령이 실행 중이라고 나옴");
        assert!(!session.has_pending_user_input());

        // 타이핑 중(제출 전) — pending_user_input만 true, command_running은 아직 false.
        session.write_keyboard_input(b"sleep 0.3").unwrap();
        assert!(session.has_pending_user_input(), "타이핑했는데 pending_user_input이 안 뜸");
        assert!(!session.is_command_running(), "제출도 안 했는데 command_running이 뜸");

        // Enter로 제출 — 그 즉시 pending_user_input은 내려가고 command_running이 뜬다
        // (write_keyboard_input 안에서 동기적으로 갱신되므로 pump() 없이도 바로 반영됨).
        session.write_keyboard_input(b"\r").unwrap();
        assert!(!session.has_pending_user_input(), "Enter를 눌렀는데 pending_user_input이 여전히 true");
        assert!(session.is_command_running(), "Enter로 명령을 제출했는데 command_running이 안 뜸");

        // 명령이 끝나고 새 프롬프트(OSC7)가 뜰 때까지 기다린다 — command_running이
        // 다시 내려가야 한다.
        pump_for(&mut session, 1200);
        assert!(!session.is_command_running(), "명령이 끝났는데 command_running이 안 내려감");
    }

    /// `supports_line_prefix_insert`는 지금은 zsh만 true다(필드 설명 참고 —
    /// bash는 같은 readline 계열이라 나중에 지원 대상이지만 아직은 아님).
    #[test]
    #[cfg_attr(windows, ignore = "이 테스트는 bash 전용 — Windows에는 없음")]
    fn bash_does_not_support_line_prefix_insert_yet() {
        let mut session = TerminalSession::spawn("/bin/bash", 80, 24, || {}).expect("bash spawn");
        pump_for(&mut session, 500);
        assert!(!session.supports_line_prefix_insert(), "지금은 zsh만 지원 대상 — bash는 아직 아님");
    }
}
