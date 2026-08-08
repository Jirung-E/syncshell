use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};

/// 탐색기 컨텍스트 메뉴가 부르는 파일 조작(DEV-008). UI 프레임워크를 모른다
/// (architecture 규칙) — 여기서는 순수하게 파일시스템만 다루고, 확인 대화상자나
/// 오류 표시는 호출부(UI)의 몫이다.
///
/// **플랫폼 API를 직접 부르지 않는다**(DEV-008 방침): 휴지통은 `trash` 크레이트가
/// Windows 휴지통 / macOS 휴지통 / Linux XDG trash를 모두 처리한다. 2·3단계에서
/// 이 모듈이 그대로 살아남게 하려는 것.

/// 새 이름으로 바꿀 때 쓸 수 없는 이름인지 검사한다.
///
/// 경로 구분자를 막는 게 핵심이다 — 사용자가 이름 칸에 `..\other\evil`을 넣으면
/// 이름 변경이 아니라 **다른 폴더로의 이동**이 돼버린다. 이름 변경은 같은 폴더
/// 안에서만 일어나야 한다.
fn validate_file_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("이름이 비어 있습니다");
    }
    if trimmed == "." || trimmed == ".." {
        bail!("'{trimmed}'는 이름으로 쓸 수 없습니다");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("이름에 경로 구분자(/ 또는 \\)를 쓸 수 없습니다");
    }
    // Windows에서 파일명에 못 쓰는 문자들. Unix에서는 대부분 쓸 수 있지만,
    // 여기서 만든 파일이 Windows에서 안 열리는 상황을 막으려고 공통으로 막는다.
    const FORBIDDEN: &[char] = &[':', '*', '?', '"', '<', '>', '|'];
    if let Some(c) = name.chars().find(|c| FORBIDDEN.contains(c)) {
        bail!("이름에 '{c}' 문자를 쓸 수 없습니다");
    }
    Ok(())
}

/// 같은 폴더 안에서 이름만 바꾼다. 성공하면 새 경로를 돌려준다.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    validate_file_name(new_name)?;
    let Some(parent) = path.parent() else {
        bail!("최상위 경로는 이름을 바꿀 수 없습니다");
    };
    let new_path = parent.join(new_name.trim());
    if new_path == path {
        return Ok(new_path); // 같은 이름 — 아무것도 안 함
    }
    // 대소문자만 다른 이름으로 바꾸는 건 Windows에서 정상 동작해야 하므로
    // (파일시스템이 대소문자를 구분하지 않아 exists()가 true로 나온다) 제외한다.
    let only_case_differs = new_path
        .file_name()
        .zip(path.file_name())
        .map(|(a, b)| a.eq_ignore_ascii_case(b))
        .unwrap_or(false);
    if new_path.exists() && !only_case_differs {
        bail!("'{}'이(가) 이미 있습니다", new_name.trim());
    }
    std::fs::rename(path, &new_path)?;
    Ok(new_path)
}

/// 휴지통으로 보낸다(영구 삭제 아님). 되돌릴 수 있으므로 UI에서 별도 확인
/// 대화상자 없이 바로 실행해도 되는 조작이다 — 일반 탐색기와 같은 감각.
pub fn move_to_trash(path: &Path) -> Result<()> {
    trash::delete(path)?;
    Ok(())
}

/// `parent` 안에 새 폴더를 만든다. 이름이 이미 있으면 탐색기처럼 뒤에 숫자를
/// 붙여(`새 폴더 (2)`) 비어 있는 이름을 찾는다 — 새로 만들기를 연달아 눌렀을 때
/// 오류가 뜨는 대신 그냥 하나 더 생기는 게 자연스럽다.
pub fn create_dir_unique(parent: &Path, base_name: &str) -> Result<PathBuf> {
    validate_file_name(base_name)?;
    let base = base_name.trim();
    for n in 1..1000 {
        let name = if n == 1 {
            base.to_string()
        } else {
            format!("{base} ({n})")
        };
        let candidate = parent.join(&name);
        if !candidate.exists() {
            std::fs::create_dir(&candidate)?;
            return Ok(candidate);
        }
    }
    bail!("'{base}' 이름으로 만들 수 있는 폴더가 없습니다(같은 이름이 너무 많음)");
}

/// `parent` 안에서 아직 안 쓰인 이름을 찾는다(`create_dir_unique`와 같은 "(n)"
/// 규칙이지만, 파일의 확장자를 보존한다 — "사진 (2).jpg"이지 "사진.jpg (2)"가
/// 아니어야 한다).
fn unique_path_in(parent: &Path, base_name: &str) -> PathBuf {
    let path = parent.join(base_name);
    if !path.exists() {
        return path;
    }
    let stem = Path::new(base_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(base_name);
    let ext = Path::new(base_name).extension().and_then(|s| s.to_str());
    for n in 2..1000 {
        let candidate_name = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(&candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    // 매우 드문 경우(같은 이름이 1000개 가까이) — 나노초 타임스탬프로 무조건 피한다.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    parent.join(format!("{base_name}-{nanos}"))
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_child = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_child)?;
        } else {
            std::fs::copy(entry.path(), &dest_child)?;
        }
    }
    Ok(())
}

/// 파일 또는 폴더(재귀적으로)를 복사해서 `dest_dir` 안에 붙여넣는다. 이름이
/// 이미 있으면 `unique_path_in`으로 "(2)" 같은 번호를 붙인다 — 덮어쓰지 않는다
/// (덮어쓰기는 사용자가 명시적으로 선택해야 하는 별도 조작이라는 게 architecture
/// 방침의 일반 원칙과 일치).
pub fn copy_into(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let name = src.file_name().and_then(|n| n.to_str()).ok_or_else(|| anyhow!("잘못된 경로입니다"))?;
    let dest = unique_path_in(dest_dir, name);
    if src.is_dir() {
        copy_dir_recursive(src, &dest)?;
    } else {
        std::fs::copy(src, &dest)?;
    }
    Ok(dest)
}

/// 잘라내기 → 붙여넣기 = 이동. 같은 볼륨이면 `rename`(원자적, 빠름)으로 처리하고,
/// 실패하면(다른 드라이브/네트워크 위치 등 `rename`이 볼륨을 못 건너는 경우)
/// 복사 후 원본을 지우는 방식으로 폴백한다.
pub fn move_into(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    if src.parent() == Some(dest_dir) {
        bail!("같은 폴더로는 이동할 수 없습니다");
    }
    let name = src.file_name().and_then(|n| n.to_str()).ok_or_else(|| anyhow!("잘못된 경로입니다"))?;
    let dest = unique_path_in(dest_dir, name);
    // 폴더를 자기 자신의 하위 폴더로 옮기려는 시도 — 허용하면 무한 재귀/데이터
    // 손상으로 이어진다.
    if dest.starts_with(src) {
        bail!("폴더를 자기 자신의 하위 폴더로 이동할 수 없습니다");
    }
    if std::fs::rename(src, &dest).is_ok() {
        return Ok(dest);
    }
    if src.is_dir() {
        copy_dir_recursive(src, &dest)?;
        std::fs::remove_dir_all(src)?;
    } else {
        std::fs::copy(src, &dest)?;
        std::fs::remove_file(src)?;
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("syncshell-fsops-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn rename_changes_name_within_same_folder() {
        let base = temp_base("rename");
        let file = base.join("before.txt");
        std::fs::write(&file, b"x").unwrap();

        let new_path = rename(&file, "after.txt").unwrap();

        assert_eq!(new_path, base.join("after.txt"));
        assert!(new_path.exists());
        assert!(!file.exists());
        std::fs::remove_dir_all(&base).ok();
    }

    /// 이름 칸에 경로를 넣어서 다른 폴더로 빠져나가는 걸 막는다 — 이름 변경은
    /// 같은 폴더 안에서만 일어나야 한다.
    #[test]
    fn rename_rejects_path_separators() {
        let base = temp_base("rename-escape");
        let file = base.join("victim.txt");
        std::fs::write(&file, b"x").unwrap();

        for bad in [r"..\escaped.txt", "../escaped.txt", "sub/child.txt"] {
            let err = rename(&file, bad).unwrap_err();
            assert!(
                err.to_string().contains("경로 구분자"),
                "{bad:?}를 막지 못함: {err}"
            );
        }
        assert!(file.exists(), "거부됐는데도 원본이 사라짐");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn rename_rejects_empty_and_dot_names() {
        let base = temp_base("rename-bad");
        let file = base.join("a.txt");
        std::fs::write(&file, b"x").unwrap();

        for bad in ["", "   ", ".", ".."] {
            assert!(rename(&file, bad).is_err(), "{bad:?}를 막지 못함");
        }
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn rename_rejects_existing_target() {
        let base = temp_base("rename-dup");
        let a = base.join("a.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(base.join("b.txt"), b"b").unwrap();

        let err = rename(&a, "b.txt").unwrap_err();
        assert!(err.to_string().contains("이미 있습니다"), "예상과 다른 오류: {err}");
        assert_eq!(std::fs::read(&a).unwrap(), b"a", "거부됐는데 원본이 바뀜");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn create_dir_unique_appends_number_on_collision() {
        let base = temp_base("mkdir");

        let first = create_dir_unique(&base, "새 폴더").unwrap();
        let second = create_dir_unique(&base, "새 폴더").unwrap();
        let third = create_dir_unique(&base, "새 폴더").unwrap();

        assert_eq!(first, base.join("새 폴더"));
        assert_eq!(second, base.join("새 폴더 (2)"));
        assert_eq!(third, base.join("새 폴더 (3)"));
        assert!(first.is_dir() && second.is_dir() && third.is_dir());
        std::fs::remove_dir_all(&base).ok();
    }

    /// 휴지통 이동이 실제로 파일을 원래 자리에서 없애는지. 휴지통이 없는
    /// 환경(일부 CI, 네트워크 드라이브 등)에서는 조용히 스킵한다 — 그런
    /// 환경에서 실패하는 건 우리 코드 문제가 아니다.
    #[test]
    fn move_to_trash_removes_file_from_original_location() {
        let base = temp_base("trash");
        let file = base.join("doomed.txt");
        std::fs::write(&file, b"bye").unwrap();

        match move_to_trash(&file) {
            Ok(()) => assert!(!file.exists(), "휴지통으로 보냈는데 원래 자리에 남아있음"),
            Err(e) => eprintln!("휴지통을 쓸 수 없는 환경 — 테스트 스킵 ({e})"),
        }
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn copy_into_copies_file_and_preserves_original() {
        let base = temp_base("copy-file");
        let src_dir = base.join("src");
        let dest_dir = base.join("dest");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        let file = src_dir.join("a.txt");
        std::fs::write(&file, b"hello").unwrap();

        let copied = copy_into(&file, &dest_dir).unwrap();

        assert_eq!(copied, dest_dir.join("a.txt"));
        assert_eq!(std::fs::read(&copied).unwrap(), b"hello");
        assert!(file.exists(), "복사인데 원본이 사라짐");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn copy_into_recurses_into_directories() {
        let base = temp_base("copy-dir");
        let src_dir = base.join("src_folder");
        std::fs::create_dir_all(src_dir.join("nested")).unwrap();
        std::fs::write(src_dir.join("top.txt"), b"top").unwrap();
        std::fs::write(src_dir.join("nested/deep.txt"), b"deep").unwrap();
        let dest_dir = base.join("dest");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let copied = copy_into(&src_dir, &dest_dir).unwrap();

        assert_eq!(std::fs::read(copied.join("top.txt")).unwrap(), b"top");
        assert_eq!(std::fs::read(copied.join("nested/deep.txt")).unwrap(), b"deep");
        assert!(src_dir.join("nested/deep.txt").exists(), "복사인데 원본이 사라짐");
        std::fs::remove_dir_all(&base).ok();
    }

    /// 같은 이름이 이미 있으면 확장자를 보존한 채 "(2)"를 붙인다 —
    /// "사진.jpg" → "사진 (2).jpg"여야지 "사진.jpg (2)"이면 안 된다.
    #[test]
    fn copy_into_appends_number_preserving_extension_on_collision() {
        let base = temp_base("copy-collision");
        let src_dir = base.join("src");
        let dest_dir = base.join("dest");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(src_dir.join("photo.jpg"), b"1").unwrap();
        std::fs::write(dest_dir.join("photo.jpg"), b"existing").unwrap();

        let copied = copy_into(&src_dir.join("photo.jpg"), &dest_dir).unwrap();

        assert_eq!(copied, dest_dir.join("photo (2).jpg"));
        assert_eq!(std::fs::read(dest_dir.join("photo.jpg")).unwrap(), b"existing", "기존 파일을 덮어씀");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn move_into_relocates_file_and_removes_original() {
        let base = temp_base("move-file");
        let src_dir = base.join("src");
        let dest_dir = base.join("dest");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        let file = src_dir.join("a.txt");
        std::fs::write(&file, b"hello").unwrap();

        let moved = move_into(&file, &dest_dir).unwrap();

        assert_eq!(moved, dest_dir.join("a.txt"));
        assert_eq!(std::fs::read(&moved).unwrap(), b"hello");
        assert!(!file.exists(), "이동인데 원본이 남아있음");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn move_into_rejects_same_folder() {
        let base = temp_base("move-same");
        let file = base.join("a.txt");
        std::fs::write(&file, b"x").unwrap();

        let err = move_into(&file, &base).unwrap_err();
        assert!(err.to_string().contains("같은 폴더"), "예상과 다른 오류: {err}");
        assert!(file.exists());
        std::fs::remove_dir_all(&base).ok();
    }

    /// 폴더를 자기 자신의 하위 폴더로 옮기려는 시도는 막아야 한다 — 허용하면
    /// rename이 실패하거나(대개), 폴백 경로(복사 후 원본 삭제)를 타면 자기
    /// 자신을 지우면서 복사하는 무한 재귀/데이터 손상으로 이어질 수 있다.
    #[test]
    fn move_into_rejects_moving_into_own_subdirectory() {
        let base = temp_base("move-into-self");
        let parent_dir = base.join("parent");
        let child_dir = parent_dir.join("child");
        std::fs::create_dir_all(&child_dir).unwrap();

        let err = move_into(&parent_dir, &child_dir).unwrap_err();
        assert!(err.to_string().contains("하위 폴더"), "예상과 다른 오류: {err}");
        assert!(parent_dir.exists() && child_dir.exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
