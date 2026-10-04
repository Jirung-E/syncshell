//! 실제 창 자가 진단(BUG-006). `SYNCSHELL_SELFTEST=keys`로 실행하면 앱이 실제
//! 창을 띄운 채 스스로 키 반복 입력을 흉내 내고(33ms마다 'q'), 프레임 간격·
//! CPU 시간·에코 지연·밀린 입력을 재서 stderr에 보고한 뒤 스스로 종료한다.
//!
//! 헤드리스 진단(`latency_probe.rs`)은 창·렌더러를 거치지 않아서, Windows에서
//! "실제 창에서만 느림"을 가르려고 넣었다. 프레임 간격이 키 반복 간격(33ms)보다
//! 길면, 실제 키를 누를 때 OS 메시지 큐에 키 반복이 쌓여 "떼도 계속 입력됨"이 된다.

use eframe::egui;
use std::time::{Duration, Instant};
use syncshell_core::terminal::TerminalSession;

const KEY: char = 'q';
const REPEAT: Duration = Duration::from_millis(33);
const WARMUP: Duration = Duration::from_secs(4);
const TYPING: Duration = Duration::from_secs(3);
const DRAIN_LIMIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Warmup,
    Typing,
    Drain,
    Done,
}

pub struct SelfTest {
    phase: Phase,
    phase_start: Instant,
    last_frame: Option<Instant>,
    sent: Vec<Instant>,
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
        match std::env::var("SYNCSHELL_SELFTEST") {
            Ok(v) if v == "keys" => Some(Self {
                phase: Phase::Warmup,
                phase_start: Instant::now(),
                last_frame: None,
                sent: Vec::new(),
                seen: 0,
                base_visible: 0,
                intervals: Vec::new(),
                cpu: Vec::new(),
                max_injected_per_frame: 0,
                echo: Vec::new(),
                drain_time: Duration::ZERO,
                accesskit_active: false,
                adapter: "(알 수 없음)".to_string(),
            }),
            _ => None,
        }
    }

    /// `raw_input_hook`에서 부른다 — 입력 중이면 그새 밀린 만큼 키 반복 이벤트를 넣는다.
    pub fn inject(&mut self, raw: &mut egui::RawInput) {
        if self.phase != Phase::Typing {
            return;
        }
        let due = (self.phase_start.elapsed().as_millis() / REPEAT.as_millis()) as usize + 1;
        let n = due.saturating_sub(self.sent.len());
        self.max_injected_per_frame = self.max_injected_per_frame.max(n);
        for _ in 0..n {
            raw.events.push(egui::Event::Key {
                key: egui::Key::Q,
                physical_key: Some(egui::Key::Q),
                pressed: true,
                repeat: !self.sent.is_empty(),
                modifiers: egui::Modifiers::NONE,
            });
            raw.events.push(egui::Event::Text(KEY.to_string()));
            self.sent.push(Instant::now());
        }
    }

    /// 매 프레임 끝에 부른다. 진단이 끝났으면 보고서를 돌려준다(한 번만).
    pub fn frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame, session: Option<&mut TerminalSession>) -> Option<String> {
        let now = Instant::now();
        let interval = self.last_frame.map(|t| now - t);
        self.last_frame = Some(now);
        ctx.request_repaint_after(Duration::from_millis(5));

        if ctx.accesskit_node_builder(egui::Id::NULL, |_| ()).is_some() {
            self.accesskit_active = true;
        }
        if let Some(rs) = frame.wgpu_render_state() {
            let info = rs.adapter.get_info();
            self.adapter = format!("{} ({:?})", info.name, info.backend);
        }
        let visible = session.as_deref().map(count_visible).unwrap_or(0);

        match self.phase {
            Phase::Warmup => {
                if self.phase_start.elapsed() >= WARMUP {
                    self.base_visible = visible;
                    self.phase = Phase::Typing;
                    self.phase_start = now;
                }
            }
            Phase::Typing | Phase::Drain => {
                if let Some(i) = interval {
                    self.intervals.push(i);
                }
                if let Some(c) = frame.info().cpu_usage {
                    self.cpu.push(c);
                }
                let shown = visible.saturating_sub(self.base_visible).min(self.sent.len());
                while self.seen < shown {
                    self.echo.push(self.sent[self.seen].elapsed());
                    self.seen += 1;
                }
                if self.phase == Phase::Typing && self.phase_start.elapsed() >= TYPING {
                    self.phase = Phase::Drain;
                    self.phase_start = now;
                } else if self.phase == Phase::Drain
                    && (self.seen >= self.sent.len() || self.phase_start.elapsed() >= DRAIN_LIMIT)
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
             GPU: {}\n\
             AccessKit(접근성 트리): {}\n\
             프레임 간격: 중앙 {:.1}ms · p95 {:.1}ms · 최대 {:.1}ms · 33ms 넘은 프레임 {}/{}\n\
             CPU(UI+렌더, vsync 대기 제외): 중앙 {:.1}ms · 최대 {:.1}ms\n\
             한 프레임에 몰아 넣은 입력 최대: {}\n\
             보낸 글자 {} · 화면에 보인 글자 {}\n\
             에코 지연: 중앙 {:.1}ms · p95 {:.1}ms · 최대 {:.1}ms\n\
             입력 멈춘 뒤 따라잡기: {:.1}ms",
            self.adapter,
            if self.accesskit_active { "켜짐" } else { "꺼짐" },
            pct(&mut self.intervals, 0.5),
            pct(&mut self.intervals, 0.95),
            pct(&mut self.intervals, 1.0),
            slow,
            self.intervals.len(),
            cpu_mid,
            cpu_max,
            self.max_injected_per_frame,
            self.sent.len(),
            self.seen,
            pct(&mut self.echo, 0.5),
            pct(&mut self.echo, 0.95),
            pct(&mut self.echo, 1.0),
            self.drain_time.as_secs_f64() * 1000.0,
        )
    }
}

fn count_visible(session: &TerminalSession) -> usize {
    session.term.renderable_content().display_iter.filter(|c| c.cell.c == KEY).count()
}
