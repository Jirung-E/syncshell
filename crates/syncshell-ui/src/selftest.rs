//! 실제 창 자가 진단(BUG-006). `SYNCSHELL_SELFTEST=keys`로 실행하면 앱이 실제
//! 창을 띄운 채 스스로 키 반복 입력을 흉내 내고(33ms마다 'q'), 프레임 간격·
//! CPU 시간·에코 지연·밀린 입력을 재서 stderr에 보고한 뒤 스스로 종료한다.
//!
//! - `keys`: 앱 내부에서 egui 이벤트로 바로 넣는다(창·렌더러 경로만 잼).
//! - `oskeys`(Windows 전용): 실제 OS 키 입력(SendInput)으로 Q를 누르고 있는 것처럼
//!   33ms마다 keydown을 보낸다 — OS → IME → winit → AccessKit → egui 경로까지 잰다.
//!   다른 OS에선 `keys`로 대신한다.
//!
//! `SYNCSHELL_NO_IME=1`을 같이 주면 측정 중 IME를 끈다(터미널의 IME 출력을 지움) —
//! Windows에서 한국어 IME가 키를 조합 경로로 가져가는지 가른다.
//!
//! `SYNCSHELL_NO_ACCESSKIT=1`을 같이 주면 AccessKit(접근성 트리)을 매 프레임 끈다 —
//! 켜짐/꺼짐을 비교해 AccessKit 비용인지 가른다.
//!
//! 헤드리스 진단(`latency_probe.rs`)은 창·렌더러를 거치지 않아서, Windows에서
//! "실제 창에서만 느림"을 가르려고 넣었다. 프레임 간격이 키 반복 간격(33ms)보다
//! 길면, 실제 키를 누를 때 OS 메시지 큐에 키 반복이 쌓여 "떼도 계속 입력됨"이 된다.

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

const KEY: char = 'q';
const REPEAT: Duration = Duration::from_millis(33);
const WARMUP: Duration = Duration::from_secs(4);
const TYPING: Duration = Duration::from_secs(3);
const DRAIN_LIMIT: Duration = Duration::from_secs(10);
/// OS 키 모드에서 창이 포커스를 받기를 기다리는 최대 시간.
const FOCUS_WAIT_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Warmup,
    /// OS 키 모드: 창이 포커스를 받을 때까지 기다림(사용자 클릭 안내)
    WaitFocus,
    Typing,
    Drain,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// egui 이벤트를 앱 안에서 바로 넣음
    Internal,
    /// OS 키 입력(SendInput)
    OsKeys,
}

pub struct SelfTest {
    mode: Mode,
    no_accesskit: bool,
    /// 키를 "보낸" 시각들 — OS 키 모드에선 별도 스레드가 채운다.
    sent_log: Arc<Mutex<Vec<Instant>>>,
    /// 창이 지금 포커스를 가졌는지 — 키 보내는 스레드가 매번 확인해서, 포커스를
    /// 잃으면 다른 창에 q가 들어가지 않게 즉시 멈춘다.
    focused: Arc<AtomicBool>,
    /// 키 보내는 스레드가 포커스를 잃어 중단했음
    focus_lost: Arc<AtomicBool>,
    /// OS 키 보내기 쪽 기록(SendInput 결과·포그라운드 창 일치 여부)
    os_trace: Arc<Mutex<OsTrace>>,
    /// 앱이 실제로 받은 Q 키 이벤트 / "q" 텍스트 이벤트 수(입력 단계에서 셈) —
    /// OS가 넣었는데 이게 0이면 winit/egui 단계에서 사라진 것이다.
    raw_key_events: usize,
    raw_text_events: usize,
    /// 측정 중 앱이 받은 입력 이벤트 전부를 종류·내용별로 센 것(IME 조합 이벤트 포함)
    raw_histogram: std::collections::BTreeMap<String, usize>,
    no_ime: bool,
    /// 측정이 무효인 이유(포커스 못 받음 등)
    invalid: Option<&'static str>,
    phase: Phase,
    phase_start: Instant,
    last_frame: Option<Instant>,
    seen: usize,
    base_visible: usize,
    intervals: Vec<Duration>,
    cpu: Vec<f32>,
    max_injected_per_frame: usize,
    echo: Vec<Duration>,
    drain_time: Duration,
    accesskit_active: bool,
    adapter: String,
}

impl SelfTest {
    pub fn from_env() -> Option<Self> {
        let mode = match std::env::var("SYNCSHELL_SELFTEST").as_deref() {
            Ok("keys") => Mode::Internal,
            Ok("oskeys") if cfg!(windows) => Mode::OsKeys,
            Ok("oskeys") => {
                eprintln!("[selftest] oskeys는 Windows 전용 — keys로 대신함");
                Mode::Internal
            }
            _ => return None,
        };
        let no_accesskit = std::env::var("SYNCSHELL_NO_ACCESSKIT").is_ok_and(|v| !v.is_empty() && v != "0");
        Some(Self {
            mode,
            no_accesskit,
            sent_log: Arc::new(Mutex::new(Vec::new())),
            focused: Arc::new(AtomicBool::new(false)),
            focus_lost: Arc::new(AtomicBool::new(false)),
            invalid: None,
            os_trace: Arc::new(Mutex::new(OsTrace::default())),
            raw_key_events: 0,
            raw_text_events: 0,
            raw_histogram: std::collections::BTreeMap::new(),
            no_ime: std::env::var("SYNCSHELL_NO_IME").is_ok_and(|v| !v.is_empty() && v != "0"),
            phase: Phase::Warmup,
            phase_start: Instant::now(),
            last_frame: None,
            seen: 0,
            base_visible: 0,
            intervals: Vec::new(),
            cpu: Vec::new(),
            max_injected_per_frame: 0,
            echo: Vec::new(),
            drain_time: Duration::ZERO,
            accesskit_active: false,
            adapter: "(알 수 없음)".to_string(),
        })
    }

    fn sent_snapshot(&self) -> Vec<Instant> {
        self.sent_log.lock().map(|v| v.clone()).unwrap_or_default()
    }

    /// `raw_input_hook`에서 부른다 — 입력 중이면 그새 밀린 만큼 키 반복 이벤트를 넣는다.
    pub fn inject(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.no_accesskit {
            // 활성화 요청(UIA 클라이언트 연결)이 오면 egui가 다시 켜므로 매 프레임 끈다.
            ctx.disable_accesskit();
        }
        if matches!(self.phase, Phase::Typing | Phase::Drain) && self.mode == Mode::OsKeys {
            for e in &raw.events {
                match e {
                    egui::Event::Key { key: egui::Key::Q, pressed: true, .. } => self.raw_key_events += 1,
                    egui::Event::Text(t) if t.contains(KEY) => self.raw_text_events += 1,
                    _ => {}
                }
                let label = match e {
                    egui::Event::Key { key, pressed, repeat, .. } => {
                        Some(format!("Key({key:?}, {}{})", if *pressed { "down" } else { "up" }, if *repeat { ", repeat" } else { "" }))
                    }
                    egui::Event::Text(t) => Some(format!("Text({t:?})")),
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => Some(format!("Ime(Preedit {text:?})")),
                    egui::Event::Ime(egui::ImeEvent::Commit(t)) => Some(format!("Ime(Commit {t:?})")),
                    egui::Event::Ime(other) => Some(format!("Ime({other:?})")),
                    _ => None,
                };
                if let Some(label) = label {
                    // 서로 다른 내용이 끝없이 늘어나지 않게 상한을 둔다.
                    if self.raw_histogram.len() < 40 || self.raw_histogram.contains_key(&label) {
                        *self.raw_histogram.entry(label).or_default() += 1;
                    }
                }
            }
        }
        if self.phase != Phase::Typing || self.mode != Mode::Internal {
            return;
        }
        let due = (self.phase_start.elapsed().as_millis() / REPEAT.as_millis()) as usize + 1;
        let mut sent = self.sent_log.lock().unwrap();
        let n = due.saturating_sub(sent.len());
        self.max_injected_per_frame = self.max_injected_per_frame.max(n);
        for _ in 0..n {
            raw.events.push(egui::Event::Key {
                key: egui::Key::Q,
                physical_key: Some(egui::Key::Q),
                pressed: true,
                repeat: !sent.is_empty(),
                modifiers: egui::Modifiers::NONE,
            });
            raw.events.push(egui::Event::Text(KEY.to_string()));
            sent.push(Instant::now());
        }
    }

    /// 매 프레임 끝에 부른다. 진단이 끝났으면 보고서를 돌려준다(한 번만).
    pub fn frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame, session: Option<&mut TerminalSession>) -> Option<String> {
        let now = Instant::now();
        let interval = self.last_frame.map(|t| now - t);
        self.last_frame = Some(now);
        ctx.request_repaint_after(Duration::from_millis(5));
        if self.no_ime {
            // 터미널 위젯이 매 프레임 IME를 켜달라고 내보내는 걸 지운다 — egui-winit이
            // 그걸 보고 set_ime_allowed(false)를 부른다.
            ctx.output_mut(|o| o.ime = None);
        }

        if ctx.accesskit_node_builder(egui::Id::NULL, |_| ()).is_some() {
            self.accesskit_active = true;
        }
        if let Some(rs) = frame.wgpu_render_state() {
            let info = rs.adapter.get_info();
            self.adapter = format!("{} ({:?})", info.name, info.backend);
        }
        let visible = session.as_deref().map(count_visible).unwrap_or(0);
        let has_focus = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        self.focused.store(has_focus, Ordering::Relaxed);

        match self.phase {
            Phase::Warmup => {
                if self.phase_start.elapsed() >= WARMUP {
                    self.phase_start = now;
                    if self.mode == Mode::OsKeys {
                        // SendInput은 포커스를 가진 창으로 간다. 띄운 쪽이 백그라운드
                        // 프로세스면 Windows 포그라운드 잠금 때문에 새 창이 포커스를
                        // 못 받을 수 있어(실측), 받을 때까지 기다린다.
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        self.phase = Phase::WaitFocus;
                    } else {
                        self.base_visible = visible;
                        self.phase = Phase::Typing;
                    }
                }
            }
            Phase::WaitFocus => {
                if has_focus {
                    self.base_visible = visible;
                    self.phase = Phase::Typing;
                    self.phase_start = now;
                    spawn_os_key_repeat(
                        self.sent_log.clone(),
                        self.focused.clone(),
                        self.focus_lost.clone(),
                        self.os_trace.clone(),
                    );
                } else if self.phase_start.elapsed() >= FOCUS_WAIT_LIMIT {
                    self.invalid = Some("창이 포커스를 받지 못함(30초) — 키를 보내지 않았음");
                    self.phase = Phase::Done;
                    return Some(self.report());
                } else {
                    let left = FOCUS_WAIT_LIMIT.saturating_sub(self.phase_start.elapsed()).as_secs();
                    egui::Area::new(egui::Id::new("selftest_focus_hint"))
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            egui::Frame::popup(ui.style()).inner_margin(16).show(ui, |ui| {
                                ui.heading("자가 진단 대기 중");
                                ui.label(format!("이 창을 한 번 클릭하세요 — 클릭하면 키 입력 측정을 시작합니다 ({left}초)"));
                            });
                        });
                }
            }
            Phase::Typing | Phase::Drain => {
                if let Some(i) = interval {
                    self.intervals.push(i);
                }
                if let Some(c) = frame.info().cpu_usage {
                    self.cpu.push(c);
                }
                let sent = self.sent_snapshot();
                let shown = visible.saturating_sub(self.base_visible).min(sent.len());
                while self.seen < shown {
                    self.echo.push(sent[self.seen].elapsed());
                    self.seen += 1;
                }
                if self.focus_lost.load(Ordering::Relaxed) && self.invalid.is_none() {
                    self.invalid = Some("측정 중 창이 포커스를 잃어 키 보내기를 멈춤 — 결과 일부만 유효");
                }
                if self.phase == Phase::Typing && self.phase_start.elapsed() >= TYPING {
                    self.phase = Phase::Drain;
                    self.phase_start = now;
                } else if self.phase == Phase::Drain
                    && (self.seen >= sent.len() || self.phase_start.elapsed() >= DRAIN_LIMIT)
                {
                    self.drain_time = self.phase_start.elapsed();
                    self.phase = Phase::Done;
                    if let Some(s) = session {
                        let _ = s.write_keyboard_input(&[0x03]); // 입력한 줄 지우기
                    }
                    return Some(self.report());
                }
            }
            Phase::Done => {}
        }
        None
    }

    fn report(&mut self) -> String {
        fn pct(v: &mut Vec<Duration>, p: f64) -> f64 {
            v.sort();
            v.get(((v.len().max(1) - 1) as f64 * p).round() as usize).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
        }
        let mut cpu = self.cpu.clone();
        cpu.sort_by(|a, b| a.total_cmp(b));
        let cpu_mid = cpu.get(cpu.len() / 2).copied().unwrap_or(0.0) * 1000.0;
        let cpu_max = cpu.last().copied().unwrap_or(0.0) * 1000.0;
        let slow = self.intervals.iter().filter(|d| **d > REPEAT).count();
        format!(
            "=== 실제 창 자가 진단 ===\n\
             {}\
             입력 방식: {}\n\
             GPU: {}\n\
             AccessKit(접근성 트리): {}\n\
             프레임 간격: 중앙 {:.1}ms · p95 {:.1}ms · 최대 {:.1}ms · 33ms 넘은 프레임 {}/{}\n\
             CPU(UI+렌더, vsync 대기 제외): 중앙 {:.1}ms · 최대 {:.1}ms\n\
             한 프레임에 몰아 넣은 입력 최대: {}\n\
             보낸 글자 {} · 화면에 보인 글자 {}\n\
             에코 지연: 중앙 {:.1}ms · p95 {:.1}ms · 최대 {:.1}ms\n\
             입력 멈춘 뒤 따라잡기: {:.1}ms{}",
            self.invalid.map(|r| format!("⚠ 측정 무효: {r}\n")).unwrap_or_default(),
            match self.mode {
                Mode::Internal => "앱 내부 egui 이벤트(keys)",
                Mode::OsKeys => "OS 키 입력 SendInput(oskeys)",
            },
            self.adapter,
            match (self.no_accesskit, self.accesskit_active) {
                (true, _) => "강제로 끔(SYNCSHELL_NO_ACCESSKIT)",
                (false, true) => "켜짐",
                (false, false) => "꺼짐",
            },
            pct(&mut self.intervals, 0.5),
            pct(&mut self.intervals, 0.95),
            pct(&mut self.intervals, 1.0),
            slow,
            self.intervals.len(),
            cpu_mid,
            cpu_max,
            self.max_injected_per_frame,
            self.sent_snapshot().len(),
            self.seen,
            pct(&mut self.echo, 0.5),
            pct(&mut self.echo, 0.95),
            pct(&mut self.echo, 1.0),
            self.drain_time.as_secs_f64() * 1000.0,
            self.os_trace_report(),
        )
    }
}

/// OS 키 보내기 쪽 기록(BUG-006): 키가 어디서 사라지는지 단계별로 가른다.
#[derive(Debug, Default, Clone)]
struct OsTrace {
    /// SendInput이 1을 돌려준(= OS 입력 큐에 들어간) 횟수
    sendinput_ok: usize,
    /// SendInput이 0을 돌려준 횟수와 마지막 GetLastError — UIPI 등으로 막힘
    sendinput_fail: usize,
    last_error: u32,
    /// 보내는 순간 포그라운드 창이 우리 프로세스의 창이었던 횟수 / 아니었던 횟수
    foreground_ours: usize,
    foreground_other: usize,
    /// Q의 하드웨어 스캔코드(MapVirtualKeyW)
    scan_code: u32,
    /// 첫 키를 보낼 때 포그라운드 창의 IME 상태(열림 여부, 한글/영문 변환 모드)
    ime_state: String,
}

impl SelfTest {
    fn os_trace_report(&self) -> String {
        if self.mode != Mode::OsKeys {
            return String::new();
        }
        let t = self.os_trace.lock().map(|t| t.clone()).unwrap_or_default();
        format!(
            "\n--- OS 키 경로 ---\n\
             SendInput 성공 {} · 실패 {} (마지막 오류 {})\n\
             보낼 때 포그라운드 창: 우리 창 {} · 다른 창 {}\n\
             Q 스캔코드 {:#x}\n\
             보낼 때 IME: {}\n\
             IME 출력(터미널이 IME 켜달라고 함): {}\n\
             앱이 받은 이벤트: Q 키 {} · \"q\" 텍스트 {}\n\
             받은 입력 이벤트 전체(종류별):\n{}",
            t.sendinput_ok,
            t.sendinput_fail,
            t.last_error,
            t.foreground_ours,
            t.foreground_other,
            t.scan_code,
            if t.ime_state.is_empty() { "(확인 못 함)" } else { t.ime_state.as_str() },
            if self.no_ime { "강제로 끔(SYNCSHELL_NO_IME)" } else { "켬(기본)" },
            self.raw_key_events,
            self.raw_text_events,
            if self.raw_histogram.is_empty() {
                "  (없음)".to_string()
            } else {
                self.raw_histogram.iter().map(|(k, v)| format!("  {k} × {v}")).collect::<Vec<_>>().join("\n")
            },
        )
    }
}

fn count_visible(session: &TerminalSession) -> usize {
    session.term.renderable_content().display_iter.filter(|c| c.cell.c == KEY).count()
}

/// OS 키 반복 흉내(Windows): 3초간 33ms마다 Q keydown을 SendInput으로 보내고 끝에
/// keyup. 키를 꾹 누르고 있을 때처럼 keyup 없이 keydown만 이어진다. 보낼 때마다
/// SendInput 결과와 포그라운드 창이 우리 것인지 기록한다.
#[cfg(windows)]
fn spawn_os_key_repeat(
    log: Arc<Mutex<Vec<Instant>>>,
    focused: Arc<AtomicBool>,
    focus_lost: Arc<AtomicBool>,
    trace: Arc<Mutex<OsTrace>>,
) {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC,
    };
    use windows_sys::Win32::UI::Input::Ime::{ImmGetDefaultIMEWnd, IME_CMODE_NATIVE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SendMessageW, WM_IME_CONTROL};

    const VK_Q: u16 = 0x51;
    // 실제 키보드처럼 스캔코드도 채운다 — 0이면 winit이 물리 키를 못 정해 버릴 수 있다.
    // SAFETY: 단순 값 변환 API.
    let scan = unsafe { MapVirtualKeyW(VK_Q as u32, MAPVK_VK_TO_VSC) };
    if let Ok(mut t) = trace.lock() {
        t.scan_code = scan;
    }
    let key = move |up: bool, trace: &Mutex<OsTrace>| {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_Q,
                    wScan: scan as u16,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: 스택의 INPUT 하나를 크기와 함께 넘긴다. GetLastError는 바로 이어서 읽는다.
        let (n, err) = unsafe { (SendInput(1, &input, std::mem::size_of::<INPUT>() as i32), GetLastError()) };
        if !up {
            if let Ok(mut t) = trace.lock() {
                if n == 1 {
                    t.sendinput_ok += 1;
                } else {
                    t.sendinput_fail += 1;
                    t.last_error = err;
                }
            }
        }
    };
    let foreground_is_ours = || {
        // SAFETY: 핸들/PID를 읽기만 한다.
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            pid == std::process::id()
        }
    };
    // 포그라운드 창의 IME 상태 — 기본 IME 창에 WM_IME_CONTROL을 보내 묻는다(다른
    // 스레드에서도 동작하는 방법). IMC_GETOPENSTATUS=5, IMC_GETCONVERSIONMODE=1.
    let ime_state = || -> String {
        // SAFETY: 핸들을 읽고 메시지로 상태를 묻기만 한다.
        unsafe {
            let ime_wnd = ImmGetDefaultIMEWnd(GetForegroundWindow());
            if ime_wnd.is_null() {
                return "IME 창 없음(IME 미사용)".to_string();
            }
            let open = SendMessageW(ime_wnd, WM_IME_CONTROL, 5, 0) != 0;
            let mode = SendMessageW(ime_wnd, WM_IME_CONTROL, 1, 0) as u32;
            let native = mode & IME_CMODE_NATIVE != 0;
            format!(
                "{} · 변환 모드 {mode:#x} ({})",
                if open { "열림" } else { "닫힘" },
                if native { "한글(네이티브)" } else { "영문" }
            )
        }
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100)); // 포커스가 확인된 직후라 짧게만 둔다
        if let Ok(mut t) = trace.lock() {
            t.ime_state = ime_state();
        }
        let start = Instant::now();
        let mut next = start;
        while start.elapsed() < TYPING {
            // 포커스를 잃었으면 즉시 멈춘다 — 안 그러면 q가 다른 창에 들어간다.
            let ours = foreground_is_ours();
            if let Ok(mut t) = trace.lock() {
                if ours {
                    t.foreground_ours += 1;
                } else {
                    t.foreground_other += 1;
                }
            }
            if !focused.load(Ordering::Relaxed) || !ours {
                focus_lost.store(true, Ordering::Relaxed);
                break;
            }
            if let Ok(mut v) = log.lock() {
                v.push(Instant::now());
            }
            key(false, &trace);
            next += REPEAT;
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        key(true, &trace);
    });
}

#[cfg(not(windows))]
fn spawn_os_key_repeat(
    _log: Arc<Mutex<Vec<Instant>>>,
    _focused: Arc<AtomicBool>,
    _focus_lost: Arc<AtomicBool>,
    _trace: Arc<Mutex<OsTrace>>,
) {
}
