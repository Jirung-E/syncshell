//! 파일 미리보기 내용 읽기(DEV-029). UI 스레드 밖(배경 스레드)에서 부르는 걸
//! 전제로 한다 — 큰 파일·느린 드라이브에서도 화면이 멈추지 않게 UI는 결과만 받는다.
//!
//! 텍스트는 앞부분만 읽고(상한 [`TEXT_LIMIT`]), 이미지는 축소본으로 디코딩해
//! RGBA로 넘긴다. 그 밖(바이너리·너무 큰 이미지)은 이름·크기·수정 시각만 보여 준다.
//!
//! 원격 경로(DEV-030)를 붙일 때는 이 읽기를 경로 추상화(DEV-013) 위로 옮긴다 —
//! 지금은 로컬 파일만 읽는다.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 텍스트 미리보기로 읽는 최대 바이트.
pub const TEXT_LIMIT: usize = 64 * 1024;
/// 텍스트 미리보기 최대 줄 수 — 짧은 줄이 아주 많은 파일에서 그리기 비용을 묶는다.
pub const TEXT_LINE_LIMIT: usize = 1000;
/// 이보다 큰 이미지 파일은 디코딩하지 않는다.
pub const IMAGE_FILE_LIMIT: u64 = 50 * 1024 * 1024;
/// 디코딩한 이미지를 이 크기(긴 변, 픽셀) 안으로 줄여 넘긴다.
pub const IMAGE_MAX_SIDE: u32 = 1600;
/// 디코딩 중 메모리 상한 — 헤더에 거대한 크기를 적은 파일로 메모리를 다 쓰지 않게.
const IMAGE_MAX_ALLOC: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Preview {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub content: PreviewContent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Text {
        text: String,
        /// 판별한 인코딩 이름("UTF-8", "UTF-16LE", "CP949" 등)
        encoding: &'static str,
        /// 앞부분만 보여 주는지(바이트·줄 상한에 걸림)
        truncated: bool,
    },
    Image {
        /// 축소한 크기
        width: u32,
        height: u32,
        /// `width * height * 4` 바이트, 알파 미리곱 아님
        rgba: Vec<u8>,
        /// 원본 크기
        original: (u32, u32),
    },
    /// 텍스트도 이미지도 아니어서 정보만 보여 준다.
    Info(InfoReason),
    /// 읽지 못함(권한 등). 메시지는 OS 오류 그대로.
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoReason {
    /// 텍스트로 읽을 수 없는 파일
    Binary,
    /// 이미지지만 [`IMAGE_FILE_LIMIT`]보다 큼
    ImageTooLarge,
    /// 이미지 형식이지만 디코딩 실패(손상·지원 안 하는 변형)
    ImageUnreadable,
    /// 폴더 등 파일이 아님
    NotFile,
}

/// `path`를 읽어 미리보기를 만든다. 실패해도 `Preview`를 돌려준다(내용이 `Error`).
pub fn load(path: &Path) -> Preview {
    let mut preview = Preview { path: path.to_path_buf(), size: 0, modified: None, content: PreviewContent::Info(InfoReason::NotFile) };
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            preview.content = PreviewContent::Error(e.to_string());
            return preview;
        }
    };
    preview.size = meta.len();
    preview.modified = meta.modified().ok();
    if !meta.is_file() {
        return preview;
    }
    preview.content = match read_content(path, meta.len()) {
        Ok(c) => c,
        Err(e) => PreviewContent::Error(e.to_string()),
    };
    preview
}

fn read_content(path: &Path, size: u64) -> std::io::Result<PreviewContent> {
    let mut file = std::fs::File::open(path)?;
    let mut head = Vec::with_capacity(TEXT_LIMIT.min(size as usize));
    (&mut file).take(TEXT_LIMIT as u64).read_to_end(&mut head)?;
    let at_eof = (head.len() as u64) >= size;

    // 이미지 판별은 확장자가 아니라 파일 앞부분(매직 넘버)으로 한다.
    if let Ok(format) = image::guess_format(&head) {
        if is_supported_image(format) {
            if size > IMAGE_FILE_LIMIT {
                return Ok(PreviewContent::Info(InfoReason::ImageTooLarge));
            }
            return Ok(decode_image(path, format));
        }
    }

    Ok(match decode_text(&head, at_eof) {
        Some((text, encoding)) => {
            let (text, cut_lines) = limit_lines(text, TEXT_LINE_LIMIT);
            PreviewContent::Text { text, encoding, truncated: !at_eof || cut_lines }
        }
        None => PreviewContent::Info(InfoReason::Binary),
    })
}

fn is_supported_image(format: image::ImageFormat) -> bool {
    use image::ImageFormat::*;
    matches!(format, Png | Jpeg | Gif | WebP | Bmp | Ico)
}

fn decode_image(path: &Path, format: image::ImageFormat) -> PreviewContent {
    let decoded = (|| -> image::ImageResult<image::DynamicImage> {
        let mut reader = image::ImageReader::open(path)?;
        reader.set_format(format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(IMAGE_MAX_ALLOC);
        reader.limits(limits);
        reader.decode()
    })();
    let Ok(img) = decoded else {
        return PreviewContent::Info(InfoReason::ImageUnreadable);
    };
    let original = (img.width(), img.height());
    let img = if img.width() > IMAGE_MAX_SIDE || img.height() > IMAGE_MAX_SIDE {
        img.thumbnail(IMAGE_MAX_SIDE, IMAGE_MAX_SIDE)
    } else {
        img
    };
    let rgba = img.into_rgba8();
    PreviewContent::Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw(), original }
}

/// 바이트 앞부분을 텍스트로 해석한다. 텍스트가 아니라고 보면 `None`.
/// `at_eof`가 false면 끝이 글자 중간에서 잘렸을 수 있어 그만큼은 버린다.
///
/// 순서: BOM(UTF-8/UTF-16) → NUL이 있으면 바이너리 → UTF-8 → CP949(한국어
/// Windows에서 흔한 옛 인코딩). 어느 쪽도 아니면 바이너리.
pub fn decode_text(bytes: &[u8], at_eof: bool) -> Option<(String, &'static str)> {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return utf8_prefix(rest, at_eof).map(|s| (s, "UTF-8 BOM"));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return Some((utf16(rest, u16::from_le_bytes), "UTF-16LE"));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return Some((utf16(rest, u16::from_be_bytes), "UTF-16BE"));
    }
    if bytes.contains(&0) {
        return None;
    }
    let (text, encoding) = match utf8_prefix(bytes, at_eof) {
        Some(s) => (s, "UTF-8"),
        None => (cp949_prefix(bytes, at_eof)?, "CP949"),
    };
    if looks_binary(&text) {
        return None;
    }
    Some((text, encoding))
}

fn utf8_prefix(bytes: &[u8], at_eof: bool) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s.to_string()),
        // 끝에서 글자가 잘린 경우만(error_len이 None) 그 앞까지 받아들인다.
        Err(e) if !at_eof && e.error_len().is_none() => {
            Some(std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()?.to_string())
        }
        Err(_) => None,
    }
}

fn cp949_prefix(bytes: &[u8], at_eof: bool) -> Option<String> {
    let enc = encoding_rs::EUC_KR; // encoding_rs의 EUC-KR은 windows-949(CP949) 상위 집합
    if let Some(s) = enc.decode_without_bom_handling_and_without_replacement(bytes) {
        return Some(s.into_owned());
    }
    // 2바이트 글자가 끝에서 잘렸으면 마지막 바이트를 빼고 한 번 더.
    if !at_eof && bytes.len() > 1 {
        if let Some(s) = enc.decode_without_bom_handling_and_without_replacement(&bytes[..bytes.len() - 1]) {
            return Some(s.into_owned());
        }
    }
    None
}

fn utf16(bytes: &[u8], from: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|c| from([c[0], c[1]]));
    char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
}

/// 탭·줄바꿈·폼피드·ESC를 뺀 제어 문자가 5%를 넘으면 바이너리로 본다.
fn looks_binary(text: &str) -> bool {
    let mut total = 0usize;
    let mut control = 0usize;
    for c in text.chars() {
        total += 1;
        if c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c' | '\x1b') {
            control += 1;
        }
    }
    total > 0 && control * 20 > total
}

fn limit_lines(text: String, max: usize) -> (String, bool) {
    match text.match_indices('\n').nth(max - 1) {
        Some((i, _)) => (text[..i].to_string(), true),
        None => (text, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_text_handles_encodings_and_rejects_binary() {
        assert_eq!(decode_text("안녕 hello".as_bytes(), true), Some(("안녕 hello".into(), "UTF-8")));
        assert_eq!(decode_text(b"\xEF\xBB\xBFabc", true), Some(("abc".into(), "UTF-8 BOM")));
        let le: Vec<u8> = [0xFF, 0xFE].into_iter().chain("가a".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(decode_text(&le, true), Some(("가a".into(), "UTF-16LE")));
        let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain("가a".encode_utf16().flat_map(|u| u.to_be_bytes())).collect();
        assert_eq!(decode_text(&be, true), Some(("가a".into(), "UTF-16BE")));
        // "한글"을 CP949로
        assert_eq!(decode_text(b"\xC7\xD1\xB1\xDB", true), Some(("한글".into(), "CP949")));
        assert_eq!(decode_text(b"PK\x03\x04\x00\x00", true), None, "NUL이 있으면 바이너리");
        assert_eq!(decode_text(b"\x01\x02\x03\x04abc", true), None, "제어 문자가 많으면 바이너리");
        assert_eq!(decode_text(b"\xFF\xFF\xFF", true), None);
    }

    #[test]
    fn decode_text_drops_char_cut_at_read_limit() {
        let bytes = "가나".as_bytes();
        // 앞부분만 읽어서 "나"의 마지막 바이트가 잘린 경우
        assert_eq!(decode_text(&bytes[..5], false), Some(("가".into(), "UTF-8")));
        // 파일 끝인데 잘려 있으면 UTF-8이 아님 — 다른 해석(CP949)도 실패해야 바이너리
        assert_ne!(decode_text(&bytes[..5], true).map(|(_, e)| e), Some("UTF-8"));
        // CP949 2바이트 글자가 잘림
        assert_eq!(decode_text(b"\xC7\xD1\xB1", false), Some(("한".into(), "CP949")));
    }

    #[test]
    fn limit_lines_cuts_after_max_lines() {
        assert_eq!(limit_lines("a\nb\nc\n".into(), 2), ("a\nb".into(), true));
        assert_eq!(limit_lines("a\nb".into(), 2), ("a\nb".into(), false));
    }

    #[test]
    fn load_reads_text_image_binary_and_large_text_prefix() {
        let dir = std::env::temp_dir().join(format!("syncshell-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let txt = dir.join("a.txt");
        std::fs::write(&txt, "hello\n").unwrap();
        let p = load(&txt);
        assert_eq!(p.size, 6);
        assert!(p.modified.is_some());
        assert_eq!(p.content, PreviewContent::Text { text: "hello\n".into(), encoding: "UTF-8", truncated: false });

        let big = dir.join("big.log");
        std::fs::write(&big, "x".repeat(TEXT_LIMIT * 2)).unwrap();
        match load(&big).content {
            PreviewContent::Text { text, truncated, .. } => {
                assert_eq!(text.len(), TEXT_LIMIT);
                assert!(truncated);
            }
            other => panic!("큰 텍스트가 텍스트로 안 읽힘: {other:?}"),
        }

        // 확장자와 무관하게 내용으로 이미지 판별, 큰 이미지는 축소
        let png = dir.join("pic.dat");
        image::RgbaImage::from_pixel(IMAGE_MAX_SIDE * 2, 10, image::Rgba([255, 0, 0, 255])).save_with_format(&png, image::ImageFormat::Png).unwrap();
        match load(&png).content {
            PreviewContent::Image { width, height, rgba, original } => {
                assert_eq!(original, (IMAGE_MAX_SIDE * 2, 10));
                assert_eq!(width, IMAGE_MAX_SIDE);
                assert_eq!(rgba.len(), (width * height * 4) as usize);
                assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
            }
            other => panic!("이미지로 안 읽힘: {other:?}"),
        }

        let bad_png = dir.join("bad.png");
        std::fs::write(&bad_png, b"\x89PNG\r\n\x1a\n\x00\x00garbage").unwrap();
        assert_eq!(load(&bad_png).content, PreviewContent::Info(InfoReason::ImageUnreadable));

        let bin = dir.join("x.bin");
        std::fs::write(&bin, [0u8, 1, 2, 3, 255]).unwrap();
        assert_eq!(load(&bin).content, PreviewContent::Info(InfoReason::Binary));

        assert_eq!(load(&dir).content, PreviewContent::Info(InfoReason::NotFile));
        assert!(matches!(load(&dir.join("없음")).content, PreviewContent::Error(_)));

        std::fs::remove_dir_all(&dir).ok();
    }
}
