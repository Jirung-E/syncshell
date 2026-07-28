// DEV-004 스파이크에서 검증한 방식의 자체 OSC7 스캐너.
// alacritty_terminal(vte)은 OSC 7을 처리하지 않고 조용히 버리므로 (DEV-004 댓글 #1 참고),
// 같은 PTY 바이트 스트림을 별도로 여기서도 스캔한다 — 필터링/분기 불필요, 화면에도 안전.
use std::path::PathBuf;

pub struct Osc7Scanner {
    buf: Vec<u8>,
}

impl Osc7Scanner {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// ESC ] 7 ; <payload> (BEL | ESC \) 를 찾아 감지된 cwd들을 순서대로 반환한다.
    pub fn feed(&mut self, data: &[u8]) -> Vec<PathBuf> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            const START: &[u8] = b"\x1b]7;";
            let Some(start) = find_sub(&self.buf, START) else {
                if self.buf.len() > 8 {
                    let keep = self.buf.len() - 8;
                    self.buf.drain(0..keep);
                }
                break;
            };
            let payload_start = start + START.len();
            let rest = &self.buf[payload_start..];
            let bel = find_sub(rest, b"\x07");
            let st = find_sub(rest, b"\x1b\\");
            let term = match (bel, st) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            let Some(term_rel) = term else {
                self.buf.drain(0..start);
                break;
            };
            let payload = String::from_utf8_lossy(&rest[..term_rel]).into_owned();
            if let Some(path) = decode_payload(&payload) {
                out.push(path);
            }
            let term_len = if bel == Some(term_rel) { 1 } else { 2 };
            let consumed = payload_start + term_rel + term_len;
            self.buf.drain(0..consumed);
        }
        out
    }
}

impl Default for Osc7Scanner {
    fn default() -> Self {
        Self::new()
    }
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// payload 형식: file://<host>/<percent-encoded 세그먼트>/<...>
fn decode_payload(payload: &str) -> Option<PathBuf> {
    let without_scheme = payload.strip_prefix("file://")?;
    let slash = without_scheme.find('/')?;
    let path_part = &without_scheme[slash + 1..];
    let decoded = percent_decode(path_part);
    let mut windows_path = decoded.replace('/', "\\");

    // 드라이브 루트("C:", "D:" 등, 세그먼트 분리 과정에서 trailing separator가 빠짐)는
    // 반드시 "C:\"처럼 백슬래시를 붙여야 한다. Windows에서 "D:"(백슬래시 없음)는
    // 드라이브 루트가 아니라 "이 프로세스가 D 드라이브에서 마지막에 있던 위치"를
    // 가리키는 전혀 다른(드라이브 상대) 경로다 — 실사용 중 `cd D:\`가 탐색기에서
    // 앱을 실행한 위치로 잘못 열리는 버그로 발견됨(TR-006 사용자 피드백).
    if windows_path.len() == 2 && windows_path.as_bytes()[1] == b':' {
        windows_path.push('\\');
    }

    Some(PathBuf::from(windows_path))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// TerminalSession이 셸 기동 직후 PTY에 그대로 써넣는 PowerShell 초기화 스크립트.
/// `$PROFILE` 파일은 건드리지 않는다 — PTY에 "타이핑하듯" 쓰기만 한다(DEV-004 결론).
/// 경로 세그먼트를 percent-encode해서 `%`, `#` 등 특수문자 포함 경로도 안전하게 왕복시킨다.
pub fn powershell_injection_script() -> &'static str {
    r#"function prompt { $p = (Get-Location).Path; $segs = $p -split '\\' | Where-Object { $_ -ne '' } | ForEach-Object { [uri]::EscapeDataString($_) }; $u = '/' + ($segs -join '/'); Write-Host -NoNewline ([char]27 + "]7;file://" + $env:COMPUTERNAME + $u + [char]7); return "PS " + $p + "> " }"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_basic_path() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/C%3A/Users/User\x07";
        let found = scanner.feed(seq);
        assert_eq!(found, vec![PathBuf::from(r"C:\Users\User")]);
    }

    #[test]
    fn decodes_korean_and_space_and_special_chars() {
        let mut scanner = Osc7Scanner::new();
        // "테스트 폴더 100%#1" 을 세그먼트별로 percent-encode했다고 가정
        let encoded = "%ED%85%8C%EC%8A%A4%ED%8A%B8%20%ED%8F%B4%EB%8D%94%20100%25%231";
        let seq = format!("\x1b]7;file://HOST/C%3A/Temp/{encoded}\x1b\\");
        let found = scanner.feed(seq.as_bytes());
        assert_eq!(found, vec![PathBuf::from("C:\\Temp\\테스트 폴더 100%#1")]);
    }

    /// 드라이브 루트로 이동하면 세그먼트 분리 과정에서 trailing separator가 사라져
    /// "D:"(드라이브 상대 경로 — 드라이브 루트가 아니라 프로세스가 그 드라이브에서
    /// 마지막에 있던 위치)로 잘못 디코드되던 버그. 실사용 중 `cd D:\`가 탐색기를
    /// 엉뚱한 곳(앱 실행 위치)으로 여는 것으로 발견됨(TR-006 사용자 피드백).
    #[test]
    fn drive_root_decodes_with_trailing_backslash() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/D%3A\x07";
        let found = scanner.feed(seq);
        assert_eq!(found, vec![PathBuf::from(r"D:\")]);
    }

    #[test]
    fn handles_split_across_multiple_feeds() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/C%3A/Windows\x07";
        let mid = seq.len() / 2;
        let mut found = scanner.feed(&seq[..mid]);
        assert!(found.is_empty());
        found = scanner.feed(&seq[mid..]);
        assert_eq!(found, vec![PathBuf::from(r"C:\Windows")]);
    }

    #[test]
    fn ignores_unrelated_bytes_and_finds_multiple_in_one_feed() {
        let mut scanner = Osc7Scanner::new();
        let mut seq = b"plain output before\r\n".to_vec();
        seq.extend_from_slice(b"\x1b]7;file://HOST/C%3A/A\x07");
        seq.extend_from_slice(b"more output\r\n");
        seq.extend_from_slice(b"\x1b]7;file://HOST/C%3A/B\x07");
        let found = scanner.feed(&seq);
        assert_eq!(found, vec![PathBuf::from(r"C:\A"), PathBuf::from(r"C:\B")]);
    }
}
