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
///
/// 디코드 결과의 경로 문법은 빌드 타깃을 따른다 — 이 스캐너가 상대하는 페이로드는
/// 항상 같은 프로세스가 주입한 셸 초기화 스크립트(`powershell_injection_script`/
/// `zsh_injection_script`)가 찍은 것이므로, 인코딩·디코딩 양쪽이 같은 플랫폼
/// 가정을 공유해도 안전하다(원격 셸을 상대하게 되면 이 가정이 깨진다 — 그때는
/// 페이로드 자체에서 판별해야 함).
fn decode_payload(payload: &str) -> Option<PathBuf> {
    let without_scheme = payload.strip_prefix("file://")?;
    let slash = without_scheme.find('/')?;
    let path_part = &without_scheme[slash + 1..];
    let decoded = percent_decode(path_part);

    #[cfg(windows)]
    {
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

    #[cfg(not(windows))]
    {
        // 유닉스 절대경로는 세그먼트 분리 과정에서 사라진 맨 앞 "/"만 되돌려주면 된다
        // ("/" 자체(루트)는 path_part가 빈 문자열인 경우로 들어온다).
        Some(PathBuf::from(format!("/{decoded}")))
    }
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

/// zsh용 초기화 스크립트. PowerShell 쪽과 동일하게 $PROFILE(.zshrc)은 건드리지
/// 않고 PTY 기동 직후 "타이핑하듯" 써넣는다 — precmd 훅으로 매 프롬프트 렌더링
/// 직전에 OSC7을 찍는다.
///
/// `unsetopt MULTIBYTE`가 핵심이다: 기본(멀티바이트 인식) 상태에서 zsh의 문자
/// 인덱싱은 코드포인트 단위로 도는데, percent-encoding은 UTF-8 **바이트** 단위여야
/// 한다(디코더 쪽 `percent_decode`도 바이트 단위). 이걸 끄지 않으면 한글 등 비ASCII
/// 경로에서 `printf '%02X' "'$ch"`가 코드포인트 값을 찍어버려 디코더가 기대하는
/// UTF-8 바이트 시퀀스와 어긋난다 — 함수 스코프 안에서만 끄고(`LOCAL_OPTIONS`)
/// 빠져나오면 원래대로 돌아온다.
pub fn zsh_injection_script() -> &'static str {
    r#"_syncshell_osc7() { emulate -L zsh; setopt LOCAL_OPTIONS; unsetopt MULTIBYTE; local __p="$PWD" __e="" __i __c; for (( __i = 1; __i <= ${#__p}; __i++ )); do __c="$__p[__i]"; case "$__c" in [A-Za-z0-9_.~-]) __e+="$__c" ;; /) __e+="$__c" ;; *) __e+=$(printf '%%%02X' "'$__c") ;; esac; done; printf '\e]7;file://%s%s\a' "$HOST" "$__e" }; autoload -Uz add-zsh-hook; add-zsh-hook precmd _syncshell_osc7"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn decodes_basic_path() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/C%3A/Users/User\x07";
        let found = scanner.feed(seq);
        assert_eq!(found, vec![PathBuf::from(r"C:\Users\User")]);
    }

    #[cfg(not(windows))]
    #[test]
    fn decodes_basic_path() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/Users/User\x07";
        let found = scanner.feed(seq);
        assert_eq!(found, vec![PathBuf::from("/Users/User")]);
    }

    #[cfg(windows)]
    #[test]
    fn decodes_korean_and_space_and_special_chars() {
        let mut scanner = Osc7Scanner::new();
        // "테스트 폴더 100%#1" 을 세그먼트별로 percent-encode했다고 가정
        let encoded = "%ED%85%8C%EC%8A%A4%ED%8A%B8%20%ED%8F%B4%EB%8D%94%20100%25%231";
        let seq = format!("\x1b]7;file://HOST/C%3A/Temp/{encoded}\x1b\\");
        let found = scanner.feed(seq.as_bytes());
        assert_eq!(found, vec![PathBuf::from("C:\\Temp\\테스트 폴더 100%#1")]);
    }

    #[cfg(not(windows))]
    #[test]
    fn decodes_korean_and_space_and_special_chars() {
        let mut scanner = Osc7Scanner::new();
        // "테스트 폴더 100%#1" 을 percent-encode했다고 가정 (zsh 주입 스크립트가
        // 실제로 이렇게 바이트 단위 UTF-8을 percent-encode해서 찍는다).
        let encoded = "%ED%85%8C%EC%8A%A4%ED%8A%B8%20%ED%8F%B4%EB%8D%94%20100%25%231";
        let seq = format!("\x1b]7;file://HOST/tmp/{encoded}\x1b\\");
        let found = scanner.feed(seq.as_bytes());
        assert_eq!(found, vec![PathBuf::from("/tmp/테스트 폴더 100%#1")]);
    }

    /// 드라이브 루트로 이동하면 세그먼트 분리 과정에서 trailing separator가 사라져
    /// "D:"(드라이브 상대 경로 — 드라이브 루트가 아니라 프로세스가 그 드라이브에서
    /// 마지막에 있던 위치)로 잘못 디코드되던 버그. 실사용 중 `cd D:\`가 탐색기를
    /// 엉뚱한 곳(앱 실행 위치)으로 여는 것으로 발견됨(TR-006 사용자 피드백).
    #[cfg(windows)]
    #[test]
    fn drive_root_decodes_with_trailing_backslash() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/D%3A\x07";
        let found = scanner.feed(seq);
        assert_eq!(found, vec![PathBuf::from(r"D:\")]);
    }

    /// 유닉스 쪽 대응: 루트("/")로 이동하면 세그먼트가 아예 없어 path_part가
    /// 빈 문자열이 된다 — 이때도 "/"(빈 경로가 아니라 루트) 하나로 복원돼야 한다.
    #[cfg(not(windows))]
    #[test]
    fn root_decodes_as_slash() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/\x07";
        let found = scanner.feed(seq);
        assert_eq!(found, vec![PathBuf::from("/")]);
    }

    #[cfg(windows)]
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

    #[cfg(not(windows))]
    #[test]
    fn handles_split_across_multiple_feeds() {
        let mut scanner = Osc7Scanner::new();
        let seq = b"\x1b]7;file://HOST/usr/bin\x07";
        let mid = seq.len() / 2;
        let mut found = scanner.feed(&seq[..mid]);
        assert!(found.is_empty());
        found = scanner.feed(&seq[mid..]);
        assert_eq!(found, vec![PathBuf::from("/usr/bin")]);
    }

    #[cfg(windows)]
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

    #[cfg(not(windows))]
    #[test]
    fn ignores_unrelated_bytes_and_finds_multiple_in_one_feed() {
        let mut scanner = Osc7Scanner::new();
        let mut seq = b"plain output before\r\n".to_vec();
        seq.extend_from_slice(b"\x1b]7;file://HOST/A\x07");
        seq.extend_from_slice(b"more output\r\n");
        seq.extend_from_slice(b"\x1b]7;file://HOST/B\x07");
        let found = scanner.feed(&seq);
        assert_eq!(found, vec![PathBuf::from("/A"), PathBuf::from("/B")]);
    }
}
