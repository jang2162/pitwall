//! `pitwall <path>` 을 창으로 바꾼다(docs/design/04-features.md F2 흐름).
//!
//! 첫 실행(콜드 스타트)과 두 번째 실행(단일 인스턴스 플러그인이 넘긴 인자)이 **같은 [`dispatch`]** 를 지난다 — 플러그인은
//! 첫 실행의 인자를 콜백으로 주지 않으므로 첫 실행은 [`invocation_from_env`] 로 같은 모양의 인자를 만들어 넘긴다.
//!
//! - 레지스트리(`project.resolve`, 워크트리 → 메인 저장소 포함)가 찾으면: 그 프로젝트 창을 열거나 앞으로 가져오고
//!   워크트리 · 파일 · 줄 · 열을 넘긴다([`windows::open_project`]).
//! - 못 찾았는데 그 파일(실제 경로)의 단일 파일 창(M1-24)이 열려 있으면: 그 창을 앞으로 가져오고 줄 · 열을 넘긴다.
//! - 못 찾으면: 「프로젝트 찾기」(S2) 요청을 보류해 두고 웰컴 창을 열어 [`FIND_PROJECT_EVENT`] 를 보낸다. 웰컴 창은
//!   마운트할 때와 그 이벤트를 받을 때 [`take_pending_find_project`] 로 꺼낸다 — 갓 만든 창이 이벤트를 놓쳐도 잃지 않는다.
//! - 경로가 없으면 웰컴 창을 열거나 앞으로 가져온다.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use pw_core::registry::Registry;
use pw_core::registry::path::normalize;
use serde::Serialize;
use tauri::{AppHandle, Emitter, EventTarget, Manager, Runtime, State, WebviewWindow};

use crate::cli::{self, Command, OpenTarget, Skipped};
use crate::windows::{self, ProjectOpen};

/// 웰컴 창에 「보류된 프로젝트 찾기 요청이 있다」를 알리는 이벤트. payload 는 없다.
pub const FIND_PROJECT_EVENT: &str = "find-project-requested";

/// 레지스트리에서 못 찾은 경로. 웰컴 창의 S2 가 루트 후보를 고르게 한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindProjectRequest {
    /// 절대 경로(정규화).
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
}

/// 앱 상태. 보류 요청은 하나만 둔다 — S2 는 한 번에 하나를 보여 주므로 새 요청이 옛 것을 바꾼다(로그에 남긴다).
pub struct Launch {
    registry: Option<Registry>,
    pending: Mutex<Option<FindProjectRequest>>,
}

impl Launch {
    pub fn new(registry: Option<Registry>) -> Self {
        Self {
            registry,
            pending: Mutex::default(),
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Option<FindProjectRequest>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 첫 실행의 인자와 cwd. 단일 인스턴스 플러그인이 두 번째 실행에서 넘기는 것과 같은 모양이다(`args_os` 를 손실 변환,
/// 첫 요소는 실행 파일, cwd 를 모르면 빈 문자열).
pub fn invocation_from_env() -> (Vec<String>, String) {
    let args = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    (args, cwd)
}

/// `argv`(첫 요소는 실행 파일)를 처리한다. 레지스트리는 잠긴 파일을 기다릴 수 있고 창 만들기는 메인 스레드를 거치므로
/// 블로킹 스레드에서 한다 — 콜드 스타트(setup, 메인 스레드)와 플러그인 콜백(비동기 태스크) 어느 쪽도 막지 않는다.
pub fn dispatch<R: Runtime>(app: &AppHandle<R>, argv: Vec<String>, cwd: String) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || handle(&app, argv.get(1..).unwrap_or(&[]), &cwd));
}

/// [`dispatch`] 의 본체. `args` 는 실행 파일을 뺀 인자.
pub fn handle<R: Runtime>(app: &AppHandle<R>, args: &[String], cwd: &str) {
    let cwd = (!cwd.is_empty()).then(|| Path::new(cwd));
    let (command, skipped) = cli::parse(args, cwd, Path::exists);
    for skip in skipped {
        match skip {
            Skipped::Flag(arg) => tracing::warn!(%arg, "ignoring an unknown option"),
            Skipped::NoCwd(arg) => {
                tracing::warn!(%arg, "cannot resolve a relative path without the caller's cwd")
            }
        }
    }
    let targets = match command {
        // main 이 앱을 띄우기 전에 가른다. 두 번째 실행이 `mcp` 로 왔다면 그 경로가 깨진 것이다.
        Command::Mcp => {
            tracing::error!("`pitwall mcp` reached the running app");
            return;
        }
        Command::Open(targets) => targets,
    };
    tracing::info!(?targets, cwd = ?cwd, "open request");
    if targets.is_empty() {
        log_err("open welcome window", windows::open_welcome(app).map(drop));
        return;
    }
    for target in targets {
        open_target(app, target);
    }
}

fn open_target<R: Runtime>(app: &AppHandle<R>, target: OpenTarget) {
    let launch = app.state::<Launch>();
    let resolved = match &launch.registry {
        Some(registry) => registry.resolve(&target.path).unwrap_or_else(|e| {
            tracing::warn!(path = %target.path.display(), %e, "cannot resolve the path; asking to find the project");
            None
        }),
        None => None,
    };
    let Some(resolution) = resolved else {
        let path = normalize(&target.path).unwrap_or(target.path);
        let files = app.state::<windows::FileWindows>();
        if files.label_of(&path).is_some() {
            let open = ProjectOpen {
                file: Some(path.display().to_string()),
                line: target.line,
                col: target.col,
                ..ProjectOpen::default()
            };
            log_err(
                "focus the file window",
                windows::open_file(app, &path, &open).map(drop),
            );
            return;
        }
        request_find_project(
            app,
            FindProjectRequest {
                path: path.display().to_string(),
                line: target.line,
                col: target.col,
            },
        );
        return;
    };
    let open = ProjectOpen {
        worktree: resolution.worktree.map(|w| w.top),
        file: Some(resolution.path),
        line: target.line,
        col: target.col,
    };
    log_err(
        "open project window",
        open_project(app, &resolution.project.id, &open),
    );
}

/// 디렉터리는 「그 프로젝트를 연다」로 끝난다. 파일(또는 아직 없는 경로)만 에디터로 넘기고, 파일이 없으면 줄 · 열도 버린다.
/// CLI 와 웰컴 창(S2 → S3 가 Orca 의 워크트리 디렉터리를 `file` 로 넘기기도 한다)이 모두 [`open_project`] 에서 이것을 지난다.
fn without_directory_file(mut open: ProjectOpen) -> ProjectOpen {
    if open.file.as_deref().is_some_and(|f| Path::new(f).is_dir()) {
        open.file = None;
    }
    if open.file.is_none() {
        open.line = None;
        open.col = None;
    }
    open
}

/// 등록된 프로젝트의 창을 열거나 앞으로 가져오고 「열었음」(`project.markOpened`)을 기록한다.
pub fn open_project<R: Runtime>(
    app: &AppHandle<R>,
    project_id: &str,
    open: &ProjectOpen,
) -> Result<(), String> {
    let launch = app.state::<Launch>();
    let registry = launch
        .registry
        .as_ref()
        .ok_or_else(|| "the project registry is not available".to_owned())?;
    // 없는 id 로 창을 만들지 않는다. label 에 들어가므로 웹뷰가 넘긴 아무 문자열이 창이 되지 않게 하는 검사이기도 하다.
    registry.get(project_id).map_err(|e| e.to_string())?;
    let open = without_directory_file(open.clone());
    windows::open_project(app, project_id, &open).map_err(|e| e.to_string())?;
    // 읽기 전용 레지스트리 등으로 기록하지 못해도 창은 열렸다.
    if let Err(e) = registry.mark_opened(project_id) {
        tracing::warn!(project_id, %e, "cannot record the project as opened");
    }
    tracing::info!(project_id, ?open, "project window opened");
    Ok(())
}

fn request_find_project<R: Runtime>(app: &AppHandle<R>, request: FindProjectRequest) {
    tracing::info!(
        ?request,
        "no project contains the path; asking the welcome window to find one"
    );
    if let Some(replaced) = app.state::<Launch>().pending().replace(request) {
        tracing::info!(?replaced, "replaced an unclaimed find-project request");
    }
    log_err("open welcome window", windows::open_welcome(app).map(drop));
    log_err(
        "notify the welcome window",
        app.emit_to(
            EventTarget::webview_window(windows::WELCOME_LABEL),
            FIND_PROJECT_EVENT,
            (),
        ),
    );
}

fn log_err<E: std::fmt::Display>(what: &str, result: Result<(), E>) {
    if let Err(error) = result {
        tracing::error!(%error, "{what} failed");
    }
}

/// 웰컴 창(M0-12)이 부른다. 인자는 `{ projectId, worktree?, file?, line?, col? }` — `worktree` 는 `ActiveWorktree.top`,
/// `file` 은 절대 경로. 창을 만들므로 async command 다(windows.rs 의 open_* 주석). 레지스트리가 잠긴 파일을 기다릴 수
/// 있어 블로킹 스레드에서 한다.
#[tauri::command]
pub async fn open_project_window<R: Runtime>(
    app: AppHandle<R>,
    project_id: String,
    worktree: Option<String>,
    file: Option<String>,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<(), String> {
    let open = ProjectOpen {
        worktree,
        file,
        line,
        col,
    };
    tauri::async_runtime::spawn_blocking(move || open_project(&app, &project_id, &open))
        .await
        .map_err(|e| e.to_string())?
}

/// S2 의 「프로젝트 없이 파일만 열기」(웰컴 창, M1-24). `path` 는 S2 요청의 절대 경로다. 정규화한 실제 경로의 단일 파일 창을
/// 열거나 앞으로 가져온다 — 파일이 아니면(폴더 · 없음) 거절한다. 창을 만들므로 async command 다(windows.rs 의 open_* 주석).
#[tauri::command]
pub async fn open_file_window<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<(), String> {
    let real =
        normalize(Path::new(&path)).map_err(|_| format!("`{path}` is not an absolute path"))?;
    if path_kind_of(&real) != PathKind::File {
        return Err(format!("`{}` is not a file", real.display()));
    }
    let open = ProjectOpen {
        file: Some(real.display().to_string()),
        line,
        col,
        ..ProjectOpen::default()
    };
    windows::open_file(&app, &real, &open).map_err(|e| e.to_string())?;
    tracing::info!(file = %real.display(), "file window opened");
    Ok(())
}

/// 단일 파일 창의 「프로젝트로 열기」(M1-24). 부른 창의 파일(창을 만들 때 적은 것 — 웹뷰가 보낸 값이 아니다)을 `pitwall <파일>` 처럼
/// 다시 해석하되 단일 파일 창으로 돌아가지 않는다: 그새 그 파일을 담는 프로젝트가 생겼으면 그 창을, 아니면 S2 를 연다. 단일 파일
/// 창은 그대로 둔다(미저장 편집이 있을 수 있다).
#[tauri::command]
pub async fn open_file_as_project<R: Runtime>(window: WebviewWindow<R>) -> Result<(), String> {
    let app = window.app_handle().clone();
    let file = app
        .state::<windows::FileWindows>()
        .file_of(window.label())
        .ok_or_else(|| "this window has no file".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || open_as_project(&app, &file))
        .await
        .map_err(|e| e.to_string())
}

fn open_as_project<R: Runtime>(app: &AppHandle<R>, file: &Path) {
    let launch = app.state::<Launch>();
    let resolved = launch
        .registry
        .as_ref()
        .and_then(|registry| registry.resolve(file).ok().flatten());
    match resolved {
        Some(resolution) => {
            let open = ProjectOpen {
                worktree: resolution.worktree.map(|w| w.top),
                file: Some(resolution.path),
                ..ProjectOpen::default()
            };
            log_err(
                "open project window",
                open_project(app, &resolution.project.id, &open),
            );
        }
        None => request_find_project(
            app,
            FindProjectRequest {
                path: file.display().to_string(),
                line: None,
                col: None,
            },
        ),
    }
}

/// 경로가 가리키는 것. 웰컴 창이 끌어다 놓은 경로를 폴더(새 프로젝트) · 파일(F2 갈래)로 가른다 — 드롭 payload 에는
/// 경로만 온다(src/welcome/native.ts 의 `pathKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathKind {
    File,
    Dir,
    Missing,
}

/// 심링크는 따라간다(폴더를 가리키는 링크는 폴더). 상대 경로 · 읽을 수 없는 경로는 `Missing` — 드롭은 절대 경로를 준다.
pub fn path_kind_of(path: &Path) -> PathKind {
    if !path.is_absolute() {
        return PathKind::Missing;
    }
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => PathKind::Dir,
        Ok(_) => PathKind::File,
        Err(_) => PathKind::Missing,
    }
}

#[tauri::command]
pub fn path_kind(path: String) -> PathKind {
    path_kind_of(Path::new(&path))
}

/// 보류된 「프로젝트 찾기」 요청을 꺼낸다(꺼내면 비운다). 없으면 `null`.
#[tauri::command]
pub fn take_pending_find_project(launch: State<'_, Launch>) -> Option<FindProjectRequest> {
    launch.pending().take()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use pw_core::EventBus;
    use pw_core::bus::DEFAULT_CAPACITY;
    use pw_core::config::AppPaths;
    use pw_core::registry::NewProject;
    use tauri::Listener;
    use tauri::test::{MockRuntime, mock_app};

    use super::*;

    #[test]
    fn path_kind_tells_files_folders_and_missing_paths_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(fs::canonicalize(tmp.path()).unwrap());
        let file = base.join("a.txt");
        fs::write(&file, "x").unwrap();
        let dir = base.join("dir");
        fs::create_dir(&dir).unwrap();
        assert_eq!(path_kind_of(&file), PathKind::File);
        assert_eq!(path_kind_of(&dir), PathKind::Dir);
        assert_eq!(path_kind_of(&base.join("nope")), PathKind::Missing);
        // 시험의 cwd(이 크레이트 폴더)에 있는 파일이어도 상대 경로는 받지 않는다.
        assert!(Path::new("Cargo.toml").exists());
        assert_eq!(path_kind_of(Path::new("Cargo.toml")), PathKind::Missing);
        #[cfg(unix)]
        {
            let link = base.join("link");
            std::os::unix::fs::symlink(&dir, &link).unwrap();
            assert_eq!(path_kind_of(&link), PathKind::Dir);
        }
        // 웹뷰가 받는 문자열(src/welcome/native.ts 의 `PathKind`).
        assert_eq!(serde_json::to_value(PathKind::Dir).unwrap(), "dir");
        assert_eq!(serde_json::to_value(PathKind::Missing).unwrap(), "missing");
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        /// 정규화된 임시 폴더. Windows 의 canonicalize 는 verbatim 접두를 붙이므로 레지스트리가 저장하는 꼴(simplify)로 맞춘다.
        base: PathBuf,
        registry: Registry,
        app: tauri::App<MockRuntime>,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = pw_core::registry::path::simplify(fs::canonicalize(tmp.path()).unwrap());
            let registry = Registry::new(
                AppPaths::under(&base.join("app")),
                None,
                EventBus::new(DEFAULT_CAPACITY),
            );
            let app = mock_app();
            app.manage(Launch::new(Some(registry.clone())));
            app.manage(windows::PendingOpens::default());
            app.manage(windows::FileWindows::default());
            Self {
                _tmp: tmp,
                base,
                registry,
                app,
            }
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let dir = self.base.join(rel);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn add(&self, root: &Path) -> String {
            self.registry
                .create(NewProject {
                    root: root.to_path_buf(),
                    ..NewProject::default()
                })
                .unwrap()
                .id
        }

        fn run(&self, args: &[&str], cwd: &Path) {
            let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
            handle(self.app.handle(), &args, &cwd.display().to_string());
        }

        fn labels(&self) -> Vec<String> {
            let mut labels: Vec<String> = self.app.webview_windows().into_keys().collect();
            labels.sort();
            labels
        }

        fn query(&self, label: &str) -> Vec<(String, String)> {
            let url = self.app.get_webview_window(label).unwrap().url().unwrap();
            url.query_pairs().into_owned().collect()
        }

        /// 그 프로젝트 창이 마운트할 때 꺼내는 것(창이 부르는 명령 그대로).
        fn opens(&self, project_id: &str) -> Vec<ProjectOpen> {
            let window = self
                .app
                .get_webview_window(&windows::project_label(project_id))
                .expect("프로젝트 창");
            windows::take_pending_project_opens(window, self.app.state())
        }

        /// 웰컴 창이 부르는 명령 그대로.
        fn take(&self) -> Option<FindProjectRequest> {
            take_pending_find_project(self.app.state::<Launch>())
        }
    }

    impl Fixture {
        /// S2 의 「파일만 열기」(웰컴 창이 부르는 명령 그대로).
        fn open_file_only(&self, path: &Path) -> Result<(), String> {
            tauri::async_runtime::block_on(open_file_window(
                self.app.handle().clone(),
                path.display().to_string(),
                None,
                None,
            ))
        }

        /// 그 파일의 단일 파일 창 label.
        fn file_window(&self, file: &Path) -> Option<String> {
            self.app.state::<windows::FileWindows>().label_of(file)
        }

        fn file_opens(&self, label: &str) -> Vec<ProjectOpen> {
            let window = self.app.get_webview_window(label).expect("단일 파일 창");
            windows::take_pending_project_opens(window, self.app.state())
        }
    }

    /// 등록 안 된 폴더의 파일: S2 → 「파일만 열기」 가 단일 파일 창을 연다. 같은 명령을 다시 하면(링크로 와도 실제 경로가 같으면)
    /// 새 창도 S2 도 없이 그 창이 앞으로 오고 줄 · 열이 그 창으로 간다.
    #[test]
    fn running_the_same_file_again_brings_its_file_window_forward() {
        let f = Fixture::new();
        let dir = f.dir("x");
        let file = dir.join("a.txt");
        fs::write(&file, "a").unwrap();
        f.run(&[file.to_str().unwrap()], &f.base);
        let request = f.take().expect("S2");
        f.open_file_only(Path::new(&request.path)).unwrap();
        let label = f.file_window(&file).expect("file window");
        assert_eq!(
            f.query(&label),
            pairs(&[("window", "file"), ("path", file.to_str().unwrap())])
        );
        assert_eq!(f.file_opens(&label), [open_file(&file, None, None)]);

        f.run(&["a.txt:5:2"], &dir);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&file, dir.join("link.txt")).unwrap();
            f.run(&["link.txt"], &dir);
        }
        assert_eq!(f.take(), None, "S2 를 다시 띄우지 않는다");
        assert_eq!(
            f.labels(),
            [label.clone(), windows::WELCOME_LABEL.to_owned()]
        );
        let mut expected = vec![open_file(&file, Some(5), Some(2))];
        #[cfg(unix)]
        expected.push(open_file(&file, None, None));
        assert_eq!(f.file_opens(&label), expected);
    }

    /// 「파일만 열기」 는 파일만 — 폴더 · 없는 경로 · 상대 경로는 창을 만들지 않는다.
    #[test]
    fn only_a_file_opens_a_file_window() {
        let f = Fixture::new();
        let dir = f.dir("x");
        for path in [dir.clone(), dir.join("missing.txt"), PathBuf::from("a.txt")] {
            assert!(f.open_file_only(&path).is_err(), "{}", path.display());
        }
        assert_eq!(f.labels(), Vec::<String>::new());
    }

    /// 「프로젝트로 열기」: 그 파일을 담는 프로젝트가 없으면 S2 를, 그사이 생겼으면 그 프로젝트 창을 연다. 단일 파일 창으로 돌아가지
    /// 않는다.
    #[test]
    fn open_as_project_asks_for_a_project_or_opens_the_one_that_now_holds_the_file() {
        let f = Fixture::new();
        let dir = f.dir("x");
        let file = dir.join("a.txt");
        fs::write(&file, "a").unwrap();
        f.open_file_only(&file).unwrap();
        open_as_project(f.app.handle(), &file);
        assert_eq!(
            f.take(),
            Some(FindProjectRequest {
                path: file.display().to_string(),
                line: None,
                col: None,
            })
        );
        let id = f.add(&dir);
        open_as_project(f.app.handle(), &file);
        assert_eq!(f.opens(&id), [open_file(&file, None, None)]);
    }

    fn open_file(path: &Path, line: Option<u32>, col: Option<u32>) -> ProjectOpen {
        ProjectOpen {
            file: Some(path.display().to_string()),
            line,
            col,
            ..ProjectOpen::default()
        }
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// `git worktree add` 와 같은 모양(gitdir.rs 시험과 같은 방법). 메인 `main/`, 워크트리 `wt/`.
    fn fake_worktree(f: &Fixture) -> (PathBuf, PathBuf) {
        let main = f.dir("main");
        let wt = f.dir("wt");
        let admin = f.dir("main/.git/worktrees/wt");
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        fs::write(
            admin.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        (main, wt)
    }

    #[test]
    fn a_registered_directory_opens_its_project_window_and_marks_it_opened() {
        let f = Fixture::new();
        let root = f.dir("repo");
        let id = f.add(&root);
        f.run(&[root.to_str().unwrap()], &f.base);
        let label = windows::project_label(&id);
        assert_eq!(f.labels(), std::slice::from_ref(&label));
        assert_eq!(
            f.query(&label),
            pairs(&[("window", "project"), ("project", &id)])
        );
        assert_eq!(f.opens(&id), [], "디렉터리는 파일을 넘기지 않는다");
        assert!(f.registry.get(&id).unwrap().opened_at.is_some());
        assert_eq!(f.take(), None);
    }

    #[test]
    fn a_relative_file_with_line_and_column_opens_in_its_project() {
        let f = Fixture::new();
        let root = f.dir("repo");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "").unwrap();
        let id = f.add(&root);
        f.run(&["src/a.rs:3:9"], &root);
        assert_eq!(
            f.opens(&id),
            // 여는 경로는 정규화된(OS 구분자) 문자열이다 — Windows 에서 `join("src/a.rs")` 는 구분자가 섞인다.
            [open_file(&root.join("src").join("a.rs"), Some(3), Some(9))]
        );
    }

    /// 창이 없는 프로젝트에 파일 둘. 첫 파일이 창을 만들고 둘째는 「이미 있는 창」으로 간다 — 막 만든 웹뷰는 아직 리스너가
    /// 없으므로 이벤트에 실었다면 둘째가 사라진다. 창이 마운트할 때 꺼내는 큐에는 둘 다 있어야 한다.
    #[test]
    fn two_files_for_a_project_without_a_window_both_arrive() {
        let f = Fixture::new();
        let root = f.dir("repo");
        fs::write(root.join("a.rs"), "").unwrap();
        fs::write(root.join("b.rs"), "").unwrap();
        let id = f.add(&root);
        let a = root.join("a.rs");
        let b = root.join("b.rs");
        f.run(&[a.to_str().unwrap(), b.to_str().unwrap()], &f.base);
        assert_eq!(f.labels(), [windows::project_label(&id)]);
        assert_eq!(
            f.opens(&id),
            [open_file(&a, None, None), open_file(&b, None, None)]
        );
    }

    #[test]
    fn an_unregistered_worktree_opens_the_main_project_with_the_worktree() {
        let f = Fixture::new();
        let (main, wt) = fake_worktree(&f);
        let id = f.add(&main);
        f.run(&[wt.to_str().unwrap()], &f.base);
        assert_eq!(
            f.opens(&id),
            [ProjectOpen {
                worktree: Some(wt.display().to_string()),
                ..ProjectOpen::default()
            }]
        );
    }

    /// 웰컴 창(`open_project_window`)이 워크트리 디렉터리를 `file` 로 넘겨도 파일로 보내지 않는다(받는 쪽 방어).
    #[test]
    fn a_directory_passed_as_the_file_is_dropped_with_its_line_and_column() {
        let f = Fixture::new();
        let (main, wt) = fake_worktree(&f);
        let id = f.add(&main);
        let directory = ProjectOpen {
            worktree: Some(wt.display().to_string()),
            file: Some(wt.display().to_string()),
            line: Some(1),
            col: Some(2),
        };
        open_project(f.app.handle(), &id, &directory).unwrap();
        assert_eq!(
            f.opens(&id),
            [ProjectOpen {
                worktree: Some(wt.display().to_string()),
                ..ProjectOpen::default()
            }]
        );
        // 남는 것이 없으면 창만 앞으로 가져온다. 파일 없는 줄 · 열도 버린다.
        let only_lines = ProjectOpen {
            line: Some(3),
            ..ProjectOpen::default()
        };
        open_project(f.app.handle(), &id, &only_lines).unwrap();
        assert_eq!(f.opens(&id), []);
    }

    #[test]
    fn an_unknown_path_asks_the_welcome_window_to_find_a_project() {
        let f = Fixture::new();
        let (_main, wt) = fake_worktree(&f);
        let notified = Arc::new(Mutex::new(0));
        let count = Arc::clone(&notified);
        f.app
            .listen_any(FIND_PROJECT_EVENT, move |_| *count.lock().unwrap() += 1);
        let file = wt.join("x.rs");
        fs::write(&file, "").unwrap();
        f.run(&[format!("{}:5", file.display()).as_str()], &f.base);
        assert_eq!(f.labels(), [windows::WELCOME_LABEL]);
        assert_eq!(*notified.lock().unwrap(), 1);
        assert_eq!(
            f.take(),
            Some(FindProjectRequest {
                path: file.display().to_string(),
                line: Some(5),
                col: None,
            })
        );
        assert_eq!(f.take(), None, "꺼내면 비운다");
    }

    #[test]
    fn a_newer_find_project_request_replaces_an_unclaimed_one() {
        let f = Fixture::new();
        let a = f.dir("a");
        let b = f.dir("b");
        f.run(&[a.to_str().unwrap()], &f.base);
        f.run(&[b.to_str().unwrap()], &f.base);
        assert_eq!(f.take().map(|r| r.path), Some(b.display().to_string()));
    }

    /// 콜드 스타트(setup)와 두 번째 실행(플러그인 콜백)이 부르는 입구. 첫 요소(실행 파일)를 빼고 [`handle`] 로 간다.
    #[test]
    fn dispatch_skips_the_executable_and_opens_on_a_blocking_thread() {
        let f = Fixture::new();
        let root = f.dir("repo");
        let id = f.add(&root);
        dispatch(
            f.app.handle(),
            vec!["/usr/local/bin/pitwall".into(), "repo".into()],
            f.base.display().to_string(),
        );
        let label = windows::project_label(&id);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while f.app.get_webview_window(&label).is_none() {
            assert!(std::time::Instant::now() < deadline, "창이 열리지 않았다");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(f.labels(), [label]);
    }

    #[test]
    fn no_path_brings_up_the_welcome_window() {
        let f = Fixture::new();
        f.run(&[], &f.base);
        assert_eq!(f.labels(), [windows::WELCOME_LABEL]);
        assert_eq!(f.take(), None);
    }

    #[test]
    fn opening_an_unknown_project_id_creates_no_window() {
        let f = Fixture::new();
        let err = open_project(f.app.handle(), "../nope", &ProjectOpen::default()).unwrap_err();
        assert!(err.contains("unknown project"), "{err}");
        assert_eq!(f.labels(), Vec::<String>::new());
    }

    #[test]
    fn an_already_open_project_window_is_reused_for_the_next_invocation() {
        let f = Fixture::new();
        let root = f.dir("repo");
        fs::write(root.join("b.rs"), "").unwrap();
        let id = f.add(&root);
        f.run(&[root.to_str().unwrap()], &f.base);
        assert_eq!(f.opens(&id), []);
        f.run(&["b.rs:2"], &root);
        assert_eq!(f.labels(), [windows::project_label(&id)]);
        assert_eq!(f.opens(&id), [open_file(&root.join("b.rs"), Some(2), None)]);
    }
}
