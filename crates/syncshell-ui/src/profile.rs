//! 성능 진단 표시(BUG-006). `SYNCSHELL_PROFILE=1`로 실행하면 1초마다 프레임
//! 간격·UI 처리 시간·PTY 쓰기 시간·PTY 출력량을 상태바와 stderr에 보여준다.
//!
//! Windows에서 "키를 꾹 누르면 입력이 밀리고 앱 전체가 느려짐"의 원인을 가르려고
//! 넣었다. 숫자를 이렇게 읽는다:
//! - UI 처리 시간은 짧은데 프레임 간격이 길다 → 렌더러(present/vsync) 쪽에서 막힘
//! - PTY 쓰기 최대 시간이 길다 → UI 스레드에서 PTY 쓰기가 막힘(ConPTY 입력 파이프)
//! - UI 처리 시간 자체가 길다 → 우리 프레임 코드(레이아웃·파싱)가 무거움
//!   (PTY 파싱 시간은 따로 보여준다)

use std::time::{Duration, Instant};
use syncshell_core::terminal::IoStats;

pub struct Profiler {
    last_frame_start: Option<Instant>,
    window_start: Instant,
    frames: u32,
    interval_sum: Duration,
    interval_max: Duration,
    ui_sum: Duration,
    ui_max: Duration,
    events_max: usize,
    io: IoStats,
    /// 상태바에 보여줄 마지막 1초 요약.
    pub summary: String,
}

impl Profiler {
    /// `SYNCSHELL_PROFILE`이 "0"이 아닌 값으로 설정돼 있으면 켠다.
    pub fn from_env() -> Option<Self> {
        match std::env::var("SYNCSHELL_PROFILE") {
            Ok(v) if !v.is_empty() && v != "0" => Some(Self::new()),
            _ => None,
        }
    }

    fn new() -> Self {
        Self {
            last_frame_start: None,
            window_start: Instant::now(),
            frames: 0,
            interval_sum: Duration::ZERO,
            interval_max: Duration::ZERO,
            ui_sum: Duration::ZERO,
            ui_max: Duration::ZERO,
            events_max: 0,
            io: IoStats::default(),
            summary: "측정 중…".to_string(),
        }
    }

    /// 프레임 시작 — 직전 프레임 시작부터의 간격(렌더·대기 포함)을 잰다.
    pub fn frame_start(&mut self, events_this_frame: usize) -> Instant {
        let now = Instant::now();
        if let Some(prev) = self.last_frame_start {
            let interval = now - prev;
            self.interval_sum += interval;
            self.interval_max = self.interval_max.max(interval);
        }
        self.last_frame_start = Some(now);
        self.events_max = self.events_max.max(events_this_frame);
        now
    }

    /// 프레임 끝 — UI 처리 시간과 이번 프레임의 PTY 계측을 더하고, 1초가
    /// 지났으면 요약을 만든다.
    pub fn frame_end(&mut self, start: Instant, io: IoStats) {
        let ui = start.elapsed();
        self.ui_sum += ui;
        self.ui_max = self.ui_max.max(ui);
        self.frames += 1;
        self.io.max_write = self.io.max_write.max(io.max_write);
        self.io.writes += io.writes;
        self.io.bytes_read += io.bytes_read;
        self.io.parse_time += io.parse_time;

        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.summary = self.summarize();
            eprintln!("[profile] {}", self.summary);
            let summary = std::mem::take(&mut self.summary);
            *self = Self { last_frame_start: self.last_frame_start, summary, ..Self::new() };
        }
    }

    fn summarize(&self) -> String {
        let n = self.frames.max(1);
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        format!(
            "{}fps · 간격 평균 {:.1}/최대 {:.1}ms · UI 평균 {:.1}/최대 {:.1}ms · 이벤트 최대 {}/프레임 · PTY 쓰기 {}회 최대 {:.1}ms · 출력 {}B 파싱 {:.1}ms",
            self.frames,
            ms(self.interval_sum) / n as f64,
            ms(self.interval_max),
            ms(self.ui_sum) / n as f64,
            ms(self.ui_max),
            self.events_max,
            self.io.writes,
            ms(self.io.max_write),
            self.io.bytes_read,
            ms(self.io.parse_time),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_frames_and_worst_write_after_one_second() {
        let mut p = Profiler::new();
        p.window_start = Instant::now() - Duration::from_secs(2);
        let start = p.frame_start(3);
        let io = IoStats { max_write: Duration::from_millis(40), writes: 2, bytes_read: 128, parse_time: Duration::ZERO };
        p.frame_end(start, io);
        assert!(p.summary.contains("1fps"), "{}", p.summary);
        assert!(p.summary.contains("이벤트 최대 3"), "{}", p.summary);
        assert!(p.summary.contains("PTY 쓰기 2회 최대 40.0ms"), "{}", p.summary);
        assert!(p.summary.contains("출력 128B"), "{}", p.summary);
    }
}
