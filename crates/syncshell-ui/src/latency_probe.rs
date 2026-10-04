//! 키 반복 입력 지연 자동 진단(BUG-006). 사람이 키를 꾹 누르지 않아도, 실제
//! 기본 셸(Windows는 PowerShell)에 키 반복 속도로 글자를 보내면서 어디가 밀리는지
//! 수치로 가른다. 창을 띄우지 않는다(헤드리스 egui).
//!
//! 실행: `cargo test --release -p syncshell-ui input_latency_probe -- --ignored --nocapture`
//!
//! 결과 읽는 법:
//! - 프레임 최대가 길다(수십 ms) → 우리 프레임 코드(그리기·파싱)가 무겁다
//! - PTY 쓰기 최대가 길다 → UI 스레드의 PTY 쓰기가 막힌다(ConPTY 입력 파이프)
//! - 에코 지연·떼고 나서 따라잡는 시간이 길다 → 셸/ConPTY가 입력을 늦게 처리
//!   (같은 셸이 Windows Terminal에선 정상이었으므로, 그렇다면 OS 기본 ConPTY 의심)
//! - 전부 짧다 → 남는 건 실제 창의 렌더러(present/vsync) — 그때는
//!   `SYNCSHELL_PROFILE=1`로 앱을 띄워 프레임 간격을 본다

use crate::terminal_widget::TerminalWidget;
use eframe::egui;
use std::time::{Duration, Instant};
use syncshell_core::terminal::{default_shell, TerminalSession};

/// 화면(뷰포트)에 보이는 `needle` 글자 수.
fn count_visible(session: &TerminalSession, needle: char) -> usize {
    session
        .term
        .renderable_content()
        .display_iter
        .filter(|cell| cell.cell.c == needle)
        .count()
}

fn frame(ctx: &egui::Context, widget: &mut TerminalWidget, session: &mut TerminalSession) -> Duration {
    let start = Instant::now();
    session.pump();
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 700.0)));
    let out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| widget.show(ui, session));
    });
    let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
    start.elapsed()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore]
fn input_latency_probe() {
    const KEY: char = 'q';
    const REPEAT: Duration = Duration::from_millis(33); // OS 키 반복(≈30회/초)과 비슷하게
    const TYPING: Duration = Duration::from_secs(3);
    const FRAME_GAP: Duration = Duration::from_millis(4); // 렌더러 없이 프레임을 촘촘히 돌림

    let ctx = egui::Context::default();
    let mut widget = TerminalWidget::new();
    widget.install_fonts(&ctx);
    let shell = default_shell();
    let mut session = TerminalSession::spawn(&shell, 120, 36, || {}).expect("셸을 띄우지 못함");

    // 셸이 뜨고(초기화 스크립트 주입 포함) 조용해질 때까지 기다린다.
    let settle_until = Instant::now() + Duration::from_secs(6);
    while Instant::now() < settle_until {
        frame(&ctx, &mut widget, &mut session);
        std::thread::sleep(Duration::from_millis(20));
    }
    let base = count_visible(&session, KEY);

    let mut frame_times = Vec::new();
    let mut write_times = Vec::new();
    let mut sent: Vec<Instant> = Vec::new();
    let mut echo_latency = Vec::new();
    let mut seen = 0usize;

    let observe = |session: &TerminalSession, sent: &[Instant], seen: &mut usize, echo: &mut Vec<Duration>| {
        let now_visible = count_visible(session, KEY).saturating_sub(base);
        while *seen < now_visible.min(sent.len()) {
            echo.push(sent[*seen].elapsed());
            *seen += 1;
        }
    };

    let start = Instant::now();
    let mut next_key = start;
    while start.elapsed() < TYPING {
        if Instant::now() >= next_key {
            let t = Instant::now();
            let _ = session.write_keyboard_input(KEY.to_string().as_bytes());
            write_times.push(t.elapsed());
            sent.push(t);
            next_key += REPEAT;
        }
        frame_times.push(frame(&ctx, &mut widget, &mut session));
        observe(&session, &sent, &mut seen, &mut echo_latency);
        std::thread::sleep(FRAME_GAP);
    }
    let typed_at_release = seen;

    // "키를 뗀 뒤" — 보낸 글자가 다 보일 때까지 얼마나 걸리나.
    let release = Instant::now();
    while seen < sent.len() && release.elapsed() < Duration::from_secs(15) {
        frame_times.push(frame(&ctx, &mut widget, &mut session));
        observe(&session, &sent, &mut seen, &mut echo_latency);
        std::thread::sleep(FRAME_GAP);
    }
    let catch_up = release.elapsed();

    // 입력한 줄을 지워 셸을 깨끗이 둔다.
    let _ = session.write_keyboard_input(&[0x03]);

    frame_times.sort();
    write_times.sort();
    echo_latency.sort();
    println!("=== 입력 지연 진단 ({shell}) ===");
    println!(
        "보낸 글자 {} · 화면에 보인 글자 {} (입력 멈춘 시점 {})",
        sent.len(),
        seen,
        typed_at_release
    );
    println!(
        "프레임 처리: 중앙 {:.2}ms · p95 {:.2}ms · 최대 {:.2}ms ({}프레임)",
        ms(percentile(&frame_times, 0.5)),
        ms(percentile(&frame_times, 0.95)),
        ms(*frame_times.last().unwrap_or(&Duration::ZERO)),
        frame_times.len()
    );
    println!(
        "PTY 쓰기: 중앙 {:.3}ms · 최대 {:.3}ms",
        ms(percentile(&write_times, 0.5)),
        ms(*write_times.last().unwrap_or(&Duration::ZERO))
    );
    println!(
        "에코 지연(보냄→화면): 중앙 {:.1}ms · p95 {:.1}ms · 최대 {:.1}ms",
        ms(percentile(&echo_latency, 0.5)),
        ms(percentile(&echo_latency, 0.95)),
        ms(*echo_latency.last().unwrap_or(&Duration::ZERO))
    );
    println!("입력 멈춘 뒤 다 따라잡기까지: {:.1}ms", ms(catch_up));
}
