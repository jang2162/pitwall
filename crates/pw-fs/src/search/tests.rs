//! `search.text` 시험. 레지스트리에 등록한 임시 프로젝트에서 옵션마다 결과를 본다.

use std::fs;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry as Projects};
use pw_rpc::protocol::{STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamOpened};
use pw_rpc::{Caller, EventHub, Outbound, Role, Session};
use serde_json::json;

use super::*;

struct Fixture {
    _tmp: tempfile::TempDir,
    /// 임시 폴더의 실제 경로.
    base: PathBuf,
    fs: Arc<Fs>,
    projects: Projects,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let projects = Projects::new(
            AppPaths::under(&base.join("app")),
            None,
            EventBus::new(DEFAULT_CAPACITY),
        );
        let fs = Arc::new(Fs::new());
        fs.attach(projects.clone());
        Self {
            _tmp: tmp,
            base,
            fs,
            projects,
        }
    }

    /// `base/repo` 에 `.git` 이 있는 저장소 프로젝트를 만든다.
    fn repo(&self) -> (String, PathBuf) {
        let root = self.base.join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        let id = self
            .projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        (id, root)
    }
}

fn write(path: &Path, text: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn params(project: &str, root: &Path, query: &str) -> SearchTextParams {
    SearchTextParams {
        project_id: project.to_owned(),
        paths: vec![root.display().to_string()],
        query: query.to_owned(),
        respect_ignore: true,
        ..SearchTextParams::default()
    }
}

/// 결과(경로순)와 수치.
fn search(fx: &Fixture, params: &SearchTextParams) -> (Vec<SearchFile>, Summary) {
    let plan = fx.fs.search_plan(params).unwrap();
    let mut files = Vec::new();
    let summary = plan.run(&AtomicBool::new(false), &mut |f| {
        files.push(f);
        true
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    (files, summary)
}

/// 결과가 있는 파일의 `root` 기준 경로와 줄 번호들.
fn hits(fx: &Fixture, root: &Path, params: &SearchTextParams) -> Vec<(String, Vec<u64>)> {
    search(fx, params)
        .0
        .into_iter()
        .map(|f| {
            let rel = Path::new(&f.path)
                .strip_prefix(root)
                .unwrap()
                .display()
                .to_string();
            (rel, f.lines.iter().map(|l| l.line).collect())
        })
        .collect()
}

fn owned(expected: &[(&str, &[u64])]) -> Vec<(String, Vec<u64>)> {
    expected
        .iter()
        .map(|(p, l)| ((*p).to_owned(), l.to_vec()))
        .collect()
}

#[test]
fn literal_regex_case_and_word_options() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join("a.txt"), "a.b\naxb\nA.B\nfoo a.bc\nfooBar foo\n");

    // 글자 그대로: `.` 은 아무 글자가 아니다. 기본은 대소문자를 가리지 않는다.
    let p = params(&id, &root, "a.b");
    assert_eq!(hits(&fx, &root, &p), owned(&[("a.txt", &[1, 3, 4])]));
    // 정규식.
    let p = SearchTextParams {
        regex: true,
        ..params(&id, &root, "a.b")
    };
    assert_eq!(hits(&fx, &root, &p), owned(&[("a.txt", &[1, 2, 3, 4])]));
    // 대소문자.
    let p = SearchTextParams {
        match_case: true,
        ..params(&id, &root, "a.b")
    };
    assert_eq!(hits(&fx, &root, &p), owned(&[("a.txt", &[1, 4])]));
    // 단어: `a.bc` 의 `a.b` 는 뒤가 단어 글자라 맞지 않는다. `fooBar` 의 `foo` 도.
    let p = SearchTextParams {
        whole_word: true,
        ..params(&id, &root, "a.b")
    };
    assert_eq!(hits(&fx, &root, &p), owned(&[("a.txt", &[1, 3])]));
    let p = SearchTextParams {
        whole_word: true,
        ..params(&id, &root, "foo")
    };
    let (files, _) = search(&fx, &p);
    assert_eq!(files[0].lines.len(), 2);
    assert_eq!(
        files[0].lines[1].ranges,
        [SearchRange { start: 7, end: 10 }],
        "`fooBar` 의 foo 는 범위가 아니다"
    );
}

#[test]
fn include_and_exclude_globs() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for f in ["src/a.rs", "src/b.ts", "src/gen/c.rs", "d.rs"] {
        write(&root.join(f), "needle\n");
    }
    let p = SearchTextParams {
        include: vec!["*.rs".into()],
        ..params(&id, &root, "needle")
    };
    assert_eq!(
        hits(&fx, &root, &p),
        owned(&[("d.rs", &[1]), ("src/a.rs", &[1]), ("src/gen/c.rs", &[1])])
    );
    // 제외가 포함보다 우선하고, 폴더 이름으로 그 아래를 통째로 뺀다. `/` 가 든 글롭은 찾는 폴더 기준이다.
    let p = SearchTextParams {
        include: vec!["*.rs".into()],
        exclude: vec!["gen".into(), "/d.rs".into()],
        ..params(&id, &root, "needle")
    };
    assert_eq!(hits(&fx, &root, &p), owned(&[("src/a.rs", &[1])]));
    // 잘못된 글롭은 invalid params.
    let p = SearchTextParams {
        include: vec!["a{".into()],
        ..params(&id, &root, "needle")
    };
    assert!(fx.fs.search_plan(&p).is_err());
}

#[test]
fn respect_ignore_toggle() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join(".gitignore"), "target/\n*.log\n");
    write(&root.join("src/a.rs"), "needle\n");
    write(&root.join("target/debug/x.rs"), "needle\n");
    write(&root.join("run.log"), "needle\n");

    let p = params(&id, &root, "needle");
    assert_eq!(hits(&fx, &root, &p), owned(&[("src/a.rs", &[1])]));
    let p = SearchTextParams {
        respect_ignore: false,
        ..params(&id, &root, "needle")
    };
    assert_eq!(
        hits(&fx, &root, &p),
        owned(&[
            ("run.log", &[1]),
            ("src/a.rs", &[1]),
            ("target/debug/x.rs", &[1])
        ])
    );
    // 키가 없으면 켜진 것이다.
    let wire: SearchTextParams =
        serde_json::from_value(json!({"projectId": id, "paths": [], "query": "x"})).unwrap();
    assert!(wire.respect_ignore);
}

#[test]
fn hidden_files_are_opt_in_and_dot_git_is_never_searched() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join(".git/config"), "needle\n");
    write(&root.join(".env"), "needle\n");
    write(&root.join(".github/ci.yml"), "needle\n");
    write(&root.join("a.txt"), "needle\n");

    let p = params(&id, &root, "needle");
    assert_eq!(hits(&fx, &root, &p), owned(&[("a.txt", &[1])]));
    let p = SearchTextParams {
        hidden: true,
        ..params(&id, &root, "needle")
    };
    assert_eq!(
        hits(&fx, &root, &p),
        owned(&[(".env", &[1]), (".github/ci.yml", &[1]), ("a.txt", &[1])])
    );
}

#[test]
fn vendored_and_generated_files_can_be_excluded() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for f in [
        "src/main.rs",
        "vendor/lib.js",
        "vendor/own.js",
        "node_modules/pkg/index.js",
        "Cargo.lock",
        "third/x.js",
        "api/client.rs",
    ] {
        write(&root.join(f), "needle\n");
    }
    // `.gitattributes` 가 경로 규칙을 덮는다: `third/` 는 벤더, `vendor/own.js` 는 벤더가 아니다, `api/` 는 생성.
    write(
        &root.join(".gitattributes"),
        "third/** linguist-vendored\nvendor/own.js -linguist-vendored\napi/** linguist-generated\n",
    );
    let all = owned(&[
        ("Cargo.lock", &[1]),
        ("api/client.rs", &[1]),
        ("node_modules/pkg/index.js", &[1]),
        ("src/main.rs", &[1]),
        ("third/x.js", &[1]),
        ("vendor/lib.js", &[1]),
        ("vendor/own.js", &[1]),
    ]);
    assert_eq!(hits(&fx, &root, &params(&id, &root, "needle")), all);

    let p = SearchTextParams {
        exclude_vendored: true,
        ..params(&id, &root, "needle")
    };
    assert_eq!(
        hits(&fx, &root, &p),
        owned(&[
            ("Cargo.lock", &[1]),
            ("api/client.rs", &[1]),
            ("src/main.rs", &[1]),
            ("vendor/own.js", &[1]),
        ])
    );
    let p = SearchTextParams {
        exclude_generated: true,
        ..params(&id, &root, "needle")
    };
    assert_eq!(
        hits(&fx, &root, &p),
        owned(&[
            ("src/main.rs", &[1]),
            ("third/x.js", &[1]),
            ("vendor/lib.js", &[1]),
            ("vendor/own.js", &[1]),
        ]),
        "Cargo.lock(경로 규칙) · api/(속성) · node_modules/(generated 의 경로 규칙)가 빠진다"
    );
}

/// 하위 폴더의 `.gitattributes` 는 그 아래에만 적용되고, 그 아래 폴더들은 그 파일을 물려받는다.
#[test]
fn a_nested_gitattributes_applies_below_its_folder() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for f in ["web/a.js", "web/deep/er/b.js", "other/c.js"] {
        write(&root.join(f), "needle\n");
    }
    write(&root.join("web/.gitattributes"), "*.js linguist-vendored\n");
    let p = SearchTextParams {
        exclude_vendored: true,
        ..params(&id, &root, "needle")
    };
    assert_eq!(hits(&fx, &root, &p), owned(&[("other/c.js", &[1])]));
}

#[test]
fn binary_files_and_files_over_50_mib_are_skipped() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    // 앞 8 KiB 안의 NUL → 바이너리.
    let mut bin = b"needle\n".to_vec();
    bin.resize(SNIFF_BYTES, b'a');
    bin[SNIFF_BYTES - 1] = 0;
    write(&root.join("bin.dat"), &bin);
    // NUL 이 8 KiB 뒤에만 있으면 텍스트다. 검색은 NUL 을 만난 버퍼에서 멈추지만 그 앞(앞 버퍼)의 결과는 남는다.
    let mut late = b"needle\n".to_vec();
    late.resize(1024 * 1024, b'a');
    late.push(0);
    late.extend_from_slice(b"\nneedle\n");
    write(&root.join("late.txt"), &late);
    // UTF-16 BOM 이 있으면 NUL 이 있어도 텍스트(판정과 같다)이고, 검색기가 UTF-8 로 바꿔 찾는다.
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("x needle\n".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    write(&root.join("wide.txt"), &utf16);

    let (files, summary) = search(&fx, &params(&id, &root, "needle"));
    let found: Vec<(&str, Vec<u64>)> = files
        .iter()
        .map(|f| {
            (
                Path::new(&f.path).file_name().unwrap().to_str().unwrap(),
                f.lines.iter().map(|l| l.line).collect(),
            )
        })
        .collect();
    assert_eq!(found, [("late.txt", vec![1]), ("wide.txt", vec![1])]);
    assert_eq!(summary.binary_files, 1);
    assert_eq!(files[1].lines[0].ranges, [SearchRange { start: 2, end: 8 }]);
}

#[test]
fn the_size_limit_is_50_mib_inclusive() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    // 일치는 첫 줄 하나 — 상한에 걸려 걷기가 먼저 멈추지 않게.
    let line = b"filler filler filler filler filler filler filler filler filler\n";
    let mut text = Vec::with_capacity(READ_ONLY_MAX as usize + 1);
    text.extend_from_slice(b"needle\n");
    while text.len() + line.len() <= READ_ONLY_MAX as usize {
        text.extend_from_slice(line);
    }
    text.resize(READ_ONLY_MAX as usize, b'x');
    write(&root.join("at.txt"), &text);
    text.push(b'x');
    write(&root.join("over.txt"), &text);

    let (files, summary) = search(&fx, &params(&id, &root, "needle"));
    assert_eq!(files.len(), 1);
    assert!(files[0].path.ends_with("at.txt"));
    assert_eq!(summary.large_files, 1);
}

#[cfg(unix)]
#[test]
fn symlinks_are_followed_only_into_the_roots() {
    use std::os::unix::fs::symlink;

    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join("real/a.txt"), "needle\n");
    write(&fx.base.join("outside/secret.txt"), "needle\n");
    // 루트 안을 가리키는 파일 · 폴더 링크는 따라간다(`rg -L`).
    symlink(root.join("real"), root.join("to-real")).unwrap();
    symlink(root.join("real/a.txt"), root.join("a-link.txt")).unwrap();
    // 루트 밖(파일 · 폴더) · 부모 · 깨진 링크는 따르지 않는다.
    symlink(fx.base.join("outside"), root.join("to-outside")).unwrap();
    symlink(fx.base.join("outside/secret.txt"), root.join("secret.txt")).unwrap();
    symlink("..", root.join("up")).unwrap();
    symlink(root.join("missing"), root.join("broken")).unwrap();
    // 고리.
    symlink(&root, root.join("real/loop")).unwrap();

    assert_eq!(
        hits(&fx, &root, &params(&id, &root, "needle")),
        owned(&[
            ("a-link.txt", &[1]),
            ("real/a.txt", &[1]),
            ("to-real/a.txt", &[1]),
        ])
    );
}

/// 걷기가 잘라 내지 못한 경우(판정 뒤에 링크로 바뀜)에도 연 핸들의 실제 경로가 루트 밖이면 읽지 않는다.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_file_that_opens_outside_the_roots_is_not_read() {
    let fx = Fixture::new();
    let (_id, root) = fx.repo();
    write(&fx.base.join("secret.txt"), "needle\n");
    std::os::unix::fs::symlink(fx.base.join("secret.txt"), root.join("s.txt")).unwrap();
    let roots = Roots::new([root.clone()]);
    assert_eq!(
        open(&roots, &root.join("s.txt")).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    write(&root.join("in.txt"), "needle\n");
    assert!(open(&roots, &root.join("in.txt")).is_ok());
}

/// FIFO 는 걷기가 건너뛰고, 그사이 FIFO 로 바뀐 경로를 열어도 막히지 않는다.
#[cfg(unix)]
#[test]
fn a_fifo_does_not_block_the_search() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join("a.txt"), "needle\n");
    let fifo = root.join("pipe");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 경로 문자열이다.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);

    let (tx, rx) = std::sync::mpsc::channel();
    let fs = Arc::clone(&fx.fs);
    let p = params(&id, &root, "needle");
    std::thread::spawn(move || {
        let plan = fs.search_plan(&p).unwrap();
        let mut n = 0;
        plan.run(&AtomicBool::new(false), &mut |_| {
            n += 1;
            true
        });
        // 걷기가 건너뛴 FIFO 를 직접 연다(판정 뒤 바뀐 경우).
        let shared = Shared {
            plan: &plan,
            stop: &AtomicBool::new(false),
            lines: AtomicU64::new(0),
            truncated: AtomicBool::new(false),
            matched_files: AtomicU64::new(0),
            searched_files: AtomicU64::new(0),
            binary_files: AtomicU64::new(0),
            large_files: AtomicU64::new(0),
            labels: Labels::default(),
        };
        let mut searcher = Searcher::new();
        let direct = shared.search(&mut searcher, &fifo);
        tx.send((n, direct.is_none())).unwrap();
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok((1, true)));
}

#[test]
fn results_stop_at_the_limit_and_report_more() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join("a.txt"), "needle\n".repeat(30));

    let p = SearchTextParams {
        max_results: Some(10),
        ..params(&id, &root, "needle")
    };
    let (files, summary) = search(&fx, &p);
    assert_eq!(files[0].lines.len(), 10);
    assert_eq!(files[0].lines.last().unwrap().line, 10);
    assert!(summary.truncated);
    assert_eq!(summary.lines, 10);

    // 딱 상한만큼이면 「더 있음」 이 아니다.
    let p = SearchTextParams {
        max_results: Some(30),
        ..params(&id, &root, "needle")
    };
    let (_, summary) = search(&fx, &p);
    assert!(!summary.truncated);
    assert_eq!(summary.lines, 30);

    // 기본 상한은 1만 줄이다.
    write(&root.join("b.txt"), "needle\n".repeat(10_001));
    let (_, summary) = search(&fx, &params(&id, &root, "needle"));
    assert!(summary.truncated);
    assert_eq!(summary.lines, u64::from(DEFAULT_MAX_RESULTS));
    assert_eq!(DEFAULT_MAX_RESULTS, 10_000);
}

#[test]
fn ranges_are_utf16_and_long_lines_are_cut_around_the_first_match() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    // 한글은 UTF-16 한 단위, 😀 는 두 단위. CRLF 는 미리보기에서 빠진다.
    let long = format!("{}needle{}", "가".repeat(300), "b".repeat(300));
    write(
        &root.join("a.txt"),
        format!("😀 needle 가 needle\r\n{long}\n"),
    );
    let (files, _) = search(&fx, &params(&id, &root, "needle"));
    let lines = &files[0].lines;
    assert_eq!(lines[0].preview, "😀 needle 가 needle");
    assert_eq!(lines[0].preview_start, 0);
    assert_eq!(
        lines[0].ranges,
        [
            SearchRange { start: 3, end: 9 },
            SearchRange { start: 12, end: 18 }
        ]
    );

    assert_eq!(lines[1].preview_start, 300 - PREVIEW_BEFORE as u32);
    assert_eq!(lines[1].preview.encode_utf16().count(), PREVIEW_MAX);
    assert!(lines[1].preview.starts_with(&"가".repeat(PREVIEW_BEFORE)));
    assert_eq!(
        lines[1].ranges,
        [SearchRange {
            start: 300,
            end: 306
        }]
    );

    // 미리보기 밖의 범위는 싣지 않는다.
    write(
        &root.join("b.txt"),
        format!("needle{}needle\n", "x".repeat(400)),
    );
    let p = SearchTextParams {
        include: vec!["b.txt".into()],
        ..params(&id, &root, "needle")
    };
    let (files, _) = search(&fx, &p);
    assert_eq!(files[0].lines[0].ranges, [SearchRange { start: 0, end: 6 }]);
}

#[test]
fn window_keeps_surrogate_pairs_whole() {
    let units: Vec<u16> = format!("{}{}", "😀".repeat(200), "x")
        .encode_utf16()
        .collect();
    // 첫 일치 앞 40 단위가 쌍의 가운데에 떨어지면 한 단위 앞으로 간다.
    let (from, to) = window(&units, 101);
    assert_eq!(from, 60);
    assert!(!is_low_surrogate(units[from]));
    assert!(to - from <= PREVIEW_MAX);
    let (from, _) = window(&units, 102);
    assert_eq!(from, 62);
    let (from, to) = window(&units, 141);
    assert_eq!((from, to), (100, 300));
    let (from, to) = window(&units, 143);
    assert_eq!(from, 102);
    assert!(!is_low_surrogate(units[to]));
    // 끝이 쌍의 가운데에 떨어지면 한 단위 짧게 끊는다(`a` 뒤로 쌍의 짝이 한 칸 밀린다).
    let units: Vec<u16> = format!("{}a{}", "😀".repeat(100), "😀".repeat(100))
        .encode_utf16()
        .collect();
    assert_eq!(window(&units, 42), (2, 201));
}

#[test]
fn invalid_requests_are_invalid_params() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    fs::create_dir_all(fx.base.join("outside")).unwrap();
    let invalid = |p: SearchTextParams| {
        let e = fx.fs.search_plan(&p).err().expect("rejected");
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "{e:?}");
    };
    invalid(params(&id, &root, ""));
    invalid(SearchTextParams {
        regex: true,
        ..params(&id, &root, "(")
    });
    invalid(SearchTextParams {
        paths: vec![],
        ..params(&id, &root, "x")
    });
    invalid(params(&id, &fx.base.join("outside"), "x"));
    invalid(params(&id, Path::new("relative"), "x"));
    invalid(SearchTextParams {
        max_results: Some(0),
        ..params(&id, &root, "x")
    });
    invalid(SearchTextParams {
        max_results: Some(MAX_RESULTS_LIMIT + 1),
        ..params(&id, &root, "x")
    });
    // 없는 경로는 NOT_FOUND.
    let e = fx
        .fs
        .search_plan(&params(&id, &root.join("missing"), "x"))
        .err()
        .unwrap();
    assert_eq!(e.code, pw_rpc::code::NOT_FOUND);
}

/// 오래 걸리는 검색(읽기마다 쉰다)을 멈추면 100 ms 안에 검색 스레드가 끝난다.
#[test]
fn stopping_ends_the_search_within_100_ms() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for i in 0..256 {
        write(&root.join(format!("f{i}.txt")), "needle\n".repeat(100));
    }
    let mut plan = fx.fs.search_plan(&params(&id, &root, "needle")).unwrap();
    // 앞부분 뒤의 읽기가 파일마다 한 번(끝)이라 멈추지 않으면 256 × 20 ms / 스레드 수(12 이하) ≈ 430 ms 넘게 걸린다.
    plan.read_delay = Some(Duration::from_millis(20));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            plan.run(&stop, &mut |_| {
                let _ = tx.send(());
                true
            })
        })
    };
    rx.recv_timeout(Duration::from_secs(5))
        .expect("a first result");
    let stopped = Instant::now();
    stop.store(true, Ordering::Relaxed);
    let summary = worker.join().unwrap();
    let took = stopped.elapsed();
    assert!(took < Duration::from_millis(100), "took {took:?}");
    assert!(summary.searched_files < 256, "{summary:?}");
}

/// 큰 파일 하나를 읽는 도중에 멈춰도 그 파일을 끝까지 읽지 않는다(읽기 한 번 안에 끝난다).
#[test]
fn stopping_ends_a_long_file_read_too() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    // 일치가 없는 8 MiB. 검색기는 64 KiB 쯤씩 읽으므로 읽기가 100 번이 넘는다.
    write(&root.join("big.txt"), "filler line\n".repeat(700_000));
    let mut plan = fx.fs.search_plan(&params(&id, &root, "needle")).unwrap();
    plan.read_delay = Some(Duration::from_millis(5));
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || plan.run(&stop, &mut |_| true))
    };
    std::thread::sleep(Duration::from_millis(50));
    assert!(!worker.is_finished(), "the read must still be running");
    let stopped = Instant::now();
    stop.store(true, Ordering::Relaxed);
    let summary = worker.join().unwrap();
    let took = stopped.elapsed();
    assert!(took < Duration::from_millis(100), "took {took:?}");
    assert_eq!(summary.searched_files, 1);
}

/// RPC 로: 청크 형식, 마지막 `done`, 취소하면 100 ms 안에 `stream.end` 가 오고 검색 스레드도 멈춘다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_sends_files_then_done_and_ends_soon_after_cancel() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join("a.txt"), "x needle\n");
    write(&root.join("b.txt"), "nothing\n");

    let mut router = Router::new();
    register(&mut router, Arc::clone(&fx.fs));
    let (session, mut rx) =
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    let next = async |rx: &mut tokio::sync::mpsc::UnboundedReceiver<Outbound>| {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("an outbound message")
            .expect("open channel")
    };
    let open = |n: i64, p: &SearchTextParams| {
        session
            .handle_value(json!({"jsonrpc": "2.0", "id": n, "method": "search.text", "params": p}));
    };

    open(1, &params(&id, &root, "needle"));
    let opened: StreamOpened = next(&mut rx)
        .await
        .into_response()
        .unwrap()
        .result_as()
        .unwrap();
    session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened.stream_id, "credit": 10}}));
    let chunk = |m: Outbound| -> serde_json::Value {
        let note = m.into_notification().unwrap();
        assert_eq!(note.method, STREAM_CHUNK);
        note.params_as::<StreamChunk>().unwrap().data
    };
    let files = chunk(next(&mut rx).await);
    assert_eq!(
        files,
        json!({"kind": "files", "files": [{
            "path": root.join("a.txt").display().to_string(),
            "lines": [{"line": 1, "preview": "x needle", "previewStart": 0, "ranges": [{"start": 2, "end": 8}]}],
        }]})
    );
    let done: SearchTextChunk = serde_json::from_value(chunk(next(&mut rx).await)).unwrap();
    assert_eq!(
        done,
        SearchTextChunk::Done {
            matched_files: 1,
            lines: 1,
            truncated: false,
            searched_files: 2,
            binary_files: 0,
            large_files: 0,
        }
    );
    let end = next(&mut rx).await.into_notification().unwrap();
    assert_eq!(end.method, STREAM_END);

    // 오래 걸리는 검색을 취소한다. 일치가 없어 검색 스레드가 보낼 것이 없다 — 받는 쪽이 사라진 것을 보내다 알 수 없으니,
    // 스트림이 끝날 때 멈추라고 알려야 멈춘다. 일치하는 `a.txt` 가 없는 폴더만 찾는다 — 취소 뒤 그것을 보내다 실패해도 멈추므로.
    let slow = root.join("slow");
    for i in 0..256 {
        write(&slow.join(format!("f{i}.txt")), "nothing\n");
    }
    let finished = Arc::new(AtomicBool::new(false));
    let mut router = Router::new();
    let (fs, flag) = (Arc::clone(&fx.fs), Arc::clone(&finished));
    router.stream(SearchTextMethod, move |_ctx, params, sink| {
        let (fs, flag) = (Arc::clone(&fs), Arc::clone(&flag));
        async move {
            stream(fs, params, &sink, |plan| {
                plan.read_delay = Some(Duration::from_millis(20));
                plan.on_finish = Some(Box::new(move || flag.store(true, Ordering::Relaxed)));
            })
            .await
        }
    });
    let (session, mut rx) =
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    session.handle_value(json!({"jsonrpc": "2.0", "id": 2, "method": "search.text", "params": params(&id, &slow, "needle")}));
    let opened: StreamOpened = next(&mut rx)
        .await
        .into_response()
        .unwrap()
        .result_as()
        .unwrap();
    session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened.stream_id, "credit": 100}}));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !finished.load(Ordering::Relaxed),
        "the search must still be running"
    );
    let cancelled = Instant::now();
    session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.cancel", "params": {"streamId": opened.stream_id}}));
    loop {
        let note = next(&mut rx).await.into_notification().unwrap();
        if note.method == STREAM_END {
            let end: StreamEnd = note.params_as().unwrap();
            assert_eq!(end.outcome, pw_rpc::protocol::StreamOutcome::Cancelled);
            break;
        }
    }
    let ended = cancelled.elapsed();
    assert!(
        ended < Duration::from_millis(100),
        "stream.end took {ended:?}"
    );
    while !finished.load(Ordering::Relaxed) {
        assert!(
            cancelled.elapsed() < Duration::from_millis(100),
            "the search kept running after the cancel"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// 벤더 · 생성 판정의 비용(측정용, 기본으로 돌지 않는다). 이 저장소의 모든 파일(ignore 무시)을 한 스레드에서 판정한다 —
/// 처음은 폴더마다 속성 파일을 읽고, 두 번째는 폴더 캐시를 쓴다.
///
/// ```bash
/// cargo test --release -p pw-fs label_cost -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement"]
fn label_cost() {
    let root = simplify(fs::canonicalize(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).unwrap());
    let files: Vec<PathBuf> = WalkBuilder::new(&root)
        .standard_filters(false)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .collect();
    let target = Target {
        path: root.clone(),
        root: root.clone(),
        overrides: Override::empty(),
    };
    let both = Exclude {
        vendored: true,
        generated: true,
    };
    let labels = Labels::default();
    for pass in ["cold", "warm"] {
        let started = Instant::now();
        let excluded = files
            .iter()
            .filter(|f| labels.excluded(f, &target, both))
            .count();
        let took = started.elapsed();
        println!(
            "{pass}: {} files, {} dirs, {excluded} excluded, {:.1} ms, {:.2} µs/file",
            files.len(),
            labels.lock().len(),
            took.as_secs_f64() * 1e3,
            took.as_secs_f64() * 1e6 / files.len() as f64
        );
    }
    let rels: Vec<String> = files.iter().map(|f| relative(f, &root).unwrap()).collect();
    let time = |name: &str, f: &dyn Fn(&str) -> bool| {
        let started = Instant::now();
        let n = rels.iter().filter(|r| f(r)).count();
        println!(
            "{name}: {n} hit, {:.2} µs/file",
            started.elapsed().as_secs_f64() * 1e6 / rels.len() as f64
        );
    };
    time("vendored_path", &|r| pw_lang::linguist::vendored_path(r));
    time("generated", &|r| {
        pw_lang::generated::generated(r, None).is_some()
    });
    let repo = find_repo(&root).unwrap();
    let attrs = RepoAttributes::load(&repo.top, repo.gitdir.as_deref(), [""]);
    time("attrs.get", &|r| attrs.get(r).vendored.is_some());
    let started = Instant::now();
    for _ in 0..100 {
        let _ = RepoAttributes::load(&repo.top, repo.gitdir.as_deref(), ["crates/pw-fs/src"]);
    }
    println!("load: {:.1} µs", started.elapsed().as_secs_f64() * 1e4);
}

/// 판정에서 거절된 요청은 청크 없이 `failed` 로 끝난다.
#[tokio::test]
async fn a_rejected_request_ends_the_stream_as_failed() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    let mut router = Router::new();
    register(&mut router, Arc::clone(&fx.fs));
    let (session, mut rx) =
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": "search.text", "params": params(&id, &root, "")}));
    let opened: StreamOpened = rx
        .recv()
        .await
        .unwrap()
        .into_response()
        .unwrap()
        .result_as()
        .unwrap();
    session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened.stream_id, "credit": 10}}));
    let note = rx.recv().await.unwrap().into_notification().unwrap();
    assert_eq!(note.method, STREAM_END);
    match note.params_as::<StreamEnd>().unwrap().outcome {
        pw_rpc::protocol::StreamOutcome::Failed { error } => {
            assert_eq!(error.code, pw_rpc::code::INVALID_PARAMS);
        }
        other => panic!("expected failed, got {other:?}"),
    }
}
