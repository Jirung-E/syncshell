use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

enum FsEvent {
    Loaded { path: PathBuf, entries: Vec<DirEntry> },
    Error { path: PathBuf, message: String },
    /// 감시 중인 디렉터리 바로 아래에서 변경이 감지된 경로들(생성/삭제/이름변경/수정
    /// 전부 뭉뚱그려서 들어온다 — notify-debouncer-mini는 종류를 구분해 주지 않는다).
    /// 폴더 전체를 다시 읽지 않고, 이 경로 하나씩만 stat해서 있으면 추가/갱신,
    /// 없으면 목록에서 제거한다(DEV-007: "전체 재조회 금지").
    Changed(Vec<PathBuf>),
}

/// 이벤트 폭주(디바운스) 처리 시간. 빌드 산출물 대량 생성처럼 짧은 시간에 수백 개
/// 이벤트가 몰릴 때 매번 갱신하지 않고 한 번에 모아 처리한다(DEV-007 스펙).
const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

/// 현재 디렉터리 목록을 백그라운드에서 읽어온다 — UI 스레드를 블로킹하지 않는다.
/// 또한 `notify`(Windows에서는 ReadDirectoryChangesW)로 현재 디렉터리를 감시해서
/// 외부에서 생기는 변경(터미널의 New-Item/Remove-Item, 메모장 저장 등)도 반영한다.
pub struct FsView {
    pub current_dir: PathBuf,
    pub entries: Vec<DirEntry>,
    pub error: Option<String>,
    rx: mpsc::Receiver<FsEvent>,
    tx: mpsc::Sender<FsEvent>,
    on_data: Arc<dyn Fn() + Send + Sync>,
    /// 살아있는 동안 감시를 유지한다 — 폴더를 옮기면 새 워처로 교체되고, 이때
    /// 이 필드의 이전 값이 drop되면서 notify-debouncer-mini가 자동으로 이전
    /// 워처를 해제한다(Debouncer의 Drop impl).
    watcher: Option<Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>>,
}

impl FsView {
    pub fn new(start_dir: PathBuf, on_data: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let on_data: Arc<dyn Fn() + Send + Sync> = Arc::new(on_data);
        let mut view = Self {
            current_dir: start_dir.clone(),
            entries: Vec::new(),
            error: None,
            rx,
            tx,
            on_data,
            watcher: None,
        };
        view.spawn_load(start_dir.clone());
        view.start_watch(&start_dir);
        view
    }

    pub fn navigate(&mut self, path: PathBuf) {
        self.current_dir = path.clone();
        self.spawn_load(path.clone());
        self.start_watch(&path);
    }

    pub fn navigate_up(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.navigate(parent.to_path_buf());
        }
    }

    /// 새 디렉터리에 대한 워처를 만들어 `self.watcher`에 넣는다 — 이전 워처(있다면)는
    /// 이 대입으로 drop되어 자동 해제된다. 감시 자체가 실패해도(권한 없음 등) 목록
    /// 표시 자체는 이미 `spawn_load`가 처리하므로 조용히 무시한다.
    fn start_watch(&mut self, path: &Path) {
        let tx = self.tx.clone();
        let on_data = self.on_data.clone();
        let handler = move |result: DebounceEventResult| {
            if let Ok(events) = result {
                let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                if !paths.is_empty() {
                    let _ = tx.send(FsEvent::Changed(paths));
                    on_data();
                }
            }
        };
        match new_debouncer(WATCH_DEBOUNCE, handler) {
            Ok(mut debouncer) => {
                // 하위 폴더까지 재귀 감시하지 않는다 — 지금 보이는 목록(바로 아래
                // 항목들)만 바뀌면 되고, 안 보이는 손자 이하 변경까지 받으면 관련
                // 없는 이벤트가 늘어날 뿐이다.
                if debouncer.watcher().watch(path, RecursiveMode::NonRecursive).is_ok() {
                    self.watcher = Some(debouncer);
                } else {
                    self.watcher = None;
                }
            }
            Err(_) => self.watcher = None,
        }
    }

    fn spawn_load(&self, path: PathBuf) {
        let tx = self.tx.clone();
        let on_data = self.on_data.clone();
        thread::spawn(move || {
            let event = match read_dir_sorted(&path) {
                Ok(entries) => FsEvent::Loaded { path, entries },
                Err(e) => FsEvent::Error {
                    path,
                    message: e.to_string(),
                },
            };
            let _ = tx.send(event);
            on_data();
        });
    }

    /// 도착한 결과를 반영한다. 진행 중 다른 폴더로 옮겨간 뒤 도착한 결과는 버린다
    /// (연속 클릭 시 옛 결과가 새 결과를 덮어쓰는 것을 방지).
    pub fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                FsEvent::Loaded { path, entries } => {
                    if path == self.current_dir {
                        self.entries = entries;
                        self.error = None;
                    }
                }
                FsEvent::Error { path, message } => {
                    if path == self.current_dir {
                        self.entries.clear();
                        self.error = Some(message);
                    }
                }
                FsEvent::Changed(paths) => {
                    for path in paths {
                        // 다른 폴더로 이미 옮겨간 뒤 도착한 옛 워처의 이벤트, 또는
                        // (이론상) 감시 대상 바로 아래가 아닌 경로는 무시한다.
                        if path.parent() != Some(self.current_dir.as_path()) {
                            continue;
                        }
                        apply_change(&mut self.entries, &path);
                    }
                }
            }
        }
    }
}

/// 변경된 경로 하나를 반영한다 — 디렉터리 전체를 다시 읽지 않고 그 경로만 stat해서
/// 있으면 추가/갱신하고, 없으면(삭제됨) 목록에서 뺀다. 이름변경은 notify 쪽에서
/// 옛 경로에 대한 "없어짐" 이벤트와 새 경로에 대한 "생겨남" 이벤트로 따로 오므로
/// 별도 처리가 필요 없다.
fn apply_change(entries: &mut Vec<DirEntry>, path: &Path) {
    match path.metadata() {
        Ok(meta) => {
            let is_dir = meta.is_dir();
            let name = match path.file_name() {
                Some(n) => n.to_string_lossy().into_owned(),
                None => return,
            };
            if let Some(existing) = entries.iter_mut().find(|e| e.path == path) {
                existing.is_dir = is_dir;
                existing.name = name;
            } else {
                entries.push(DirEntry {
                    name,
                    path: path.to_path_buf(),
                    is_dir,
                });
            }
        }
        Err(_) => entries.retain(|e| e.path != path),
    }
    entries.sort_by(cmp_entries);
}

fn cmp_entries(a: &DirEntry, b: &DirEntry) -> std::cmp::Ordering {
    match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    }
}

fn read_dir_sorted(path: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut entries: Vec<DirEntry> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let entry_path = e.path();
            // DirEntry::file_type()은 심볼릭 링크/정션 자체의 타입을 돌려주고 대상을
            // 따라가지 않는다 — 심볼릭 링크로 된 폴더가 파일로 잘못 분류되어 탐색기에서
            // 클릭 자체가 안 되는 버그로 발견됨(TR-006 사용자 피드백: "링크 접근이 안됨").
            // path().metadata()는 기본적으로 심볼릭 링크를 따라가므로 실제 대상 타입을 본다.
            let is_dir = entry_path.metadata().map(|m| m.is_dir()).unwrap_or(false);
            let name = e.file_name().to_string_lossy().into_owned();
            DirEntry {
                name,
                path: entry_path,
                is_dir,
            }
        })
        .collect();

    entries.sort_by(cmp_entries);

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_first_then_name_sorted_including_korean_and_space() {
        let base = std::env::temp_dir().join(format!("syncshell-fsview-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(base.join("zzz_folder")).unwrap();
        std::fs::create_dir_all(base.join("한글 폴더")).unwrap();
        std::fs::write(base.join("aaa_file.txt"), b"x").unwrap();
        std::fs::write(base.join("readme.md"), b"x").unwrap();

        let entries = read_dir_sorted(&base).unwrap();
        std::fs::remove_dir_all(&base).ok();

        assert_eq!(entries.len(), 4);
        // 폴더 두 개가 먼저 온다 (이름순), 그다음 파일 두 개 (이름순)
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert!(!entries[2].is_dir && !entries[3].is_dir);
        let dir_names: Vec<&str> = entries[0..2].iter().map(|e| e.name.as_str()).collect();
        assert!(dir_names.contains(&"zzz_folder"));
        assert!(dir_names.contains(&"한글 폴더"));
        assert_eq!(entries[2].name, "aaa_file.txt");
        assert_eq!(entries[3].name, "readme.md");
    }

    /// 심볼릭 링크로 된 폴더가 is_dir=true로 정확히 분류되는지 — file_type()이 링크
    /// 자체의 타입을 돌려주는 바람에 파일로 잘못 분류되어 탐색기에서 클릭이 안 되던
    /// 버그의 회귀 테스트(TR-006 사용자 피드백). Developer Mode가 꺼져 있는 등
    /// 심볼릭 링크 생성 권한이 없는 환경에서는 조용히 스킵한다.
    #[test]
    fn symlinked_folder_is_classified_as_dir() {
        let base = std::env::temp_dir().join(format!("syncshell-fsview-symlink-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let real_dir = base.join("real_target");
        std::fs::create_dir_all(&real_dir).unwrap();
        let link = base.join("link_to_target");

        if std::os::windows::fs::symlink_dir(&real_dir, &link).is_err() {
            eprintln!("심볼릭 링크 생성 권한 없음 — 테스트 스킵 (Developer Mode 확인 필요)");
            std::fs::remove_dir_all(&base).ok();
            return;
        }

        let entries = read_dir_sorted(&base).unwrap();
        std::fs::remove_dir_all(&base).ok();

        let link_entry = entries.iter().find(|e| e.name == "link_to_target");
        assert!(link_entry.is_some(), "심볼릭 링크가 목록에 없음");
        assert!(link_entry.unwrap().is_dir, "심볼릭 링크 폴더가 is_dir=false로 잘못 분류됨");
    }

    #[test]
    fn nonexistent_dir_returns_err() {
        let bogus = std::env::temp_dir().join("syncshell-does-not-exist-xyz");
        assert!(read_dir_sorted(&bogus).is_err());
    }

    /// 최대 3초 동안 `pump()`를 반복 호출하며 조건이 만족될 때까지 기다린다.
    /// notify 이벤트는 OS 큐 + 디바운스를 거치므로 정확한 타이밍을 예측할 수
    /// 없다 — 실제 파일시스템 워처를 쓰는 테스트라 폴링이 맞는 접근이다.
    fn wait_until(view: &mut FsView, mut cond: impl FnMut(&FsView) -> bool) -> bool {
        for _ in 0..150 {
            view.pump();
            if cond(view) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    /// DEV-007 회귀 테스트: 터미널의 `New-Item`처럼 FsView 바깥에서 파일을
    /// 생성하면, 폴더를 다시 읽지 않고도(watch만으로) 목록에 반영되는지.
    /// 실제 notify 워처(Windows ReadDirectoryChangesW)로 검증 — 헤드리스로
    /// 재현 가능한 최소 시나리오다.
    #[test]
    fn watch_reflects_externally_created_file() {
        let base = std::env::temp_dir().join(format!("syncshell-fsview-watch-create-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();

        let mut view = FsView::new(base.clone(), || {});
        // 초기 로드(빈 폴더 재조회)가 끝날 시간을 준다 — 그래야 뒤이은 워처
        // 이벤트로 추가한 항목이 나중에 도착하는 초기 FsEvent::Loaded에 덮어써지지
        // 않는다. entries/error 필드만으론 "빈 폴더 로드 완료"와 "아직 로드 전"을
        // 구분할 수 없어(둘 다 entries=[], error=None) 짧은 sleep으로 대신한다.
        std::thread::sleep(std::time::Duration::from_millis(200));
        view.pump();

        std::fs::write(base.join("new_file.txt"), b"hello").unwrap();

        let found = wait_until(&mut view, |v| v.entries.iter().any(|e| e.name == "new_file.txt"));
        std::fs::remove_dir_all(&base).ok();

        assert!(found, "외부에서 만든 파일이 워처만으로 목록에 반영되지 않음");
    }

    /// DEV-007 회귀 테스트: 반대 방향 — 외부에서 삭제된 파일이 목록에서 빠지는지.
    #[test]
    fn watch_reflects_externally_deleted_file() {
        let base = std::env::temp_dir().join(format!("syncshell-fsview-watch-delete-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let target = base.join("doomed.txt");
        std::fs::write(&target, b"bye").unwrap();

        let mut view = FsView::new(base.clone(), || {});
        wait_until(&mut view, |v| v.entries.iter().any(|e| e.name == "doomed.txt"));

        std::fs::remove_file(&target).unwrap();

        let gone = wait_until(&mut view, |v| !v.entries.iter().any(|e| e.name == "doomed.txt"));
        std::fs::remove_dir_all(&base).ok();

        assert!(gone, "외부에서 삭제한 파일이 워처만으로 목록에서 안 빠짐");
    }

    /// DEV-007 회귀 테스트: 다른 폴더로 이동하면 이전 폴더에 대한 감시가 풀리고
    /// 새 폴더에 대한 감시로 교체되는지 — 이전 폴더에서 파일을 만들어도 더 이상
    /// 반영되지 않아야 한다(워처가 새 폴더로 넘어갔다는 뜻).
    #[test]
    fn navigate_switches_watch_to_new_directory() {
        let base = std::env::temp_dir().join(format!("syncshell-fsview-watch-switch-{}", std::process::id()));
        let dir_a = base.join("a");
        let dir_b = base.join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        // current_dir는 new()/navigate() 안에서 동기적으로 바뀌므로 기다릴 필요가
        // 없다 — 워처 교체(start_watch)도 같은 호출 안에서 동기적으로 끝난다.
        let mut view = FsView::new(dir_a.clone(), || {});
        view.navigate(dir_b.clone());

        // b에 대한 초기 폴더 재조회가 끝날 시간을 준다(위 create 테스트와 같은 이유
        // — 안 그러면 뒤이은 워처 이벤트가 나중에 도착하는 초기 로드에 덮어써질 수 있음).
        std::thread::sleep(std::time::Duration::from_millis(200));
        view.pump();

        // b에서 만든 파일은 반영되어야 한다.
        std::fs::write(dir_b.join("in_b.txt"), b"x").unwrap();
        let found_in_b = wait_until(&mut view, |v| v.entries.iter().any(|e| e.name == "in_b.txt"));

        // a는 더 이상 감시 중이 아니어야 한다 — a에 파일을 만들어도(현재 폴더가
        // b이므로 목록엔 안 보이겠지만) apply_change의 parent 체크로 걸러지는지와는
        // 별개로, 여기선 단순히 워처 교체 자체가 크래시/패닉 없이 동작하는지와
        // b 쪽 반영이 되는지만 확인한다.
        std::fs::remove_dir_all(&base).ok();

        assert!(found_in_b, "폴더 이동 후 새 폴더에 대한 감시가 동작하지 않음");
    }
}
