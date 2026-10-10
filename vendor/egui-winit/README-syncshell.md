# egui-winit 0.35.0 + syncshell 패치

crates.io의 egui-winit 0.35.0 원본에 **한 군데**만 고친 사본이다(루트 `Cargo.toml`의
`[patch.crates-io]`로 연결). 고친 곳은 `src/lib.rs`에서 `syncshell 패치(BUG-006)` 주석으로
찾을 수 있다.

## 무엇을, 왜

원본은 IME 허용 중 입력 이벤트가 있는 프레임마다(IME 영역이 그대로여도)
`Window::set_ime_cursor_area`를 부른다. Windows에서 이건 `ImmSetCompositionWindow` +
`ImmSetCandidateWindow`로 이어지고, TSF 기반 한국어 IME에서는 키 하나마다 이 처리가 끼어든다.
syncshell 터미널은 IME를 늘 허용해 두므로(DEV-012 한글 조합 입력), 키를 꾹 누르면
반복이 OS 속도를 못 따라가고(실측 IME 켬 ≈21회/초 vs 끔 ≈30회/초) 키를 뗀 뒤에도 밀린 입력이
계속 들어갔다(BUG-006). 패치는 영역이 실제로 바뀔 때만 알리게 한다.

## 업데이트할 때

eframe/egui를 올리면 새 egui-winit에 같은 조건(`|| ... !i.events.is_empty()`)이 남아 있는지
보고, 남아 있으면 새 버전을 다시 복사해 같은 한 줄을 고친다. 없어졌으면 이 폴더와
`[patch.crates-io]`를 지운다.
