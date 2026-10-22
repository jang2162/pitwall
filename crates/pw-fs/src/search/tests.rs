//! `search.text` 시험. 레지스트리에 등록한 임시 프로젝트에서 옵션마다 결과를 본다.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry as Projects};
use pw_rpc::protocol::{STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamOpened};
use pw_rpc::{Caller, EventHub, Outbound, Role, Session};
use serde_json::json;

use super::*;

/// 벽시계로 「100 ms 안」 을 판정하는가(M1-07 완료 기준 「취소하면 100 ms 안에 스트림이 끝난다」). release 빌드에서만 한다 —
/// debug 빌드는 기계 부하만큼 스레드가 밀려 흔들린다(부하 40~180 에서 번갈아 실패했다). debug 빌드는 멈춘 뒤에 한 일(결과 수 ·
/// 찾은 파일 수 · 읽은 데까지)로 판정한다. 시간을 잴 때는 혼자 돌린다:
/// `cargo nextest run -p pw-fs --release -j 1 -E 'test(/^search::tests::stopping_/) | test(=search::tests::the_stream_sends_files_then_done_and_ends_soon_after_cancel)'`
const RELEASE_TIMING: bool = !cfg!(debug_assertions);

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

/// 결과가 있는 파일의 `root` 기준 경로(`/` 로 가른다 — 결과의 절대 경로는 OS 구분자다)와 줄 번호들.
fn hits(fx: &Fixture, root: &Path, params: &SearchTextParams) -> Vec<(String, Vec<u64>)> {
    search(fx, params)
        .0
        .into_iter()
        .map(|f| {
            let rel = Path::new(&f.path)
                .strip_prefix(root)
                .unwrap()
                .iter()
                .map(|c| c.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
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
    let at = |byte, unit| At { byte, unit };
    // 첫 일치(`b`) 앞 40 단위가 쌍의 가운데에 떨어지면 그 글자 앞으로, 끝(시작 + 200)도 쌍의 가운데면 한 단위 짧게.
    let line = format!("{}ab{}", "😀".repeat(100), "😀".repeat(100));
    let (from, to) = window(line.as_bytes(), 401);
    assert_eq!((from, to), (at(320, 160), at(718, 360)));
    let line = format!("{}a{}", "😀".repeat(100), "😀".repeat(100));
    assert_eq!(window(line.as_bytes(), 84), (at(4, 2), at(401, 201)));
}

/// 미리보기 구간이 줄 전체를 UTF-16 으로 바꿔 정하던 방식(아래 `reference`)과 같다 — 구간 앞뒤만 글자로 읽어도.
#[test]
fn window_matches_converting_the_whole_line() {
    fn reference(line: &[u8], first: usize) -> (usize, usize) {
        let low = |u: u16| (0xDC00..=0xDFFF).contains(&u);
        let units: Vec<u16> = String::from_utf8_lossy(line).encode_utf16().collect();
        let first = String::from_utf8_lossy(&line[..first])
            .encode_utf16()
            .count();
        if units.len() <= PREVIEW_MAX {
            return (0, units.len());
        }
        let mut from = first
            .saturating_sub(PREVIEW_BEFORE)
            .min(units.len() - PREVIEW_MAX);
        if from > 0 && low(units[from]) {
            from -= 1;
        }
        let mut to = (from + PREVIEW_MAX).min(units.len());
        if to < units.len() && low(units[to]) {
            to -= 1;
        }
        (from, to)
    }
    // 한 바이트 · 한글(3) · 이모지(4) · 잘린 UTF-8 · 혼자 떨어진 이어지는 바이트를 섞은 줄들.
    let pieces: [&[u8]; 6] = [
        b"a",
        "가".as_bytes(),
        "😀".as_bytes(),
        b"\xE2\x82",
        b"\x80",
        b"\xF0\x9F",
    ];
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for _ in 0..2000 {
        let mut line = Vec::new();
        for _ in 0..next(1200) {
            line.extend_from_slice(pieces[next(pieces.len())]);
        }
        // 첫 일치는 글자 경계에서 시작한다(아무 바이트나 고르고 경계로 내린다).
        let first = boundary(&line, next(line.len() + 1));
        let (from, to) = window(&line, first);
        assert_eq!(
            (from.unit, to.unit),
            reference(&line, first),
            "{line:?} @ {first}"
        );
        // 바이트 위치가 그 단위 위치와 맞는다.
        for at in [from, to] {
            assert_eq!(
                String::from_utf8_lossy(&line[..at.byte])
                    .encode_utf16()
                    .count(),
                at.unit
            );
        }
    }
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

/// 오래 걸리는 검색(읽기마다 쉰다)을 멈추면 남은 파일을 찾지 않는다 — 멈춘 뒤에 오는 결과는 그때 찾던 파일(스레드마다
/// 하나)뿐이다. 100 ms 안에 끝나는지는 release 빌드에서만 판정한다([`RELEASE_TIMING`]).
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
    let after_stop = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = {
        let (stop, after_stop) = (Arc::clone(&stop), Arc::clone(&after_stop));
        std::thread::spawn(move || {
            plan.run(&Arc::clone(&stop), &mut |_| {
                if stop.load(Ordering::Relaxed) {
                    after_stop.fetch_add(1, Ordering::Relaxed);
                }
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
    let after_stop = after_stop.load(Ordering::Relaxed);
    assert!(
        after_stop <= SEARCH_THREADS,
        "{after_stop} files after the stop"
    );
    assert!(summary.searched_files < 256, "{summary:?}");
    if RELEASE_TIMING {
        assert!(took < Duration::from_millis(100), "took {took:?}");
    }
}

/// 큰 파일 하나를 읽는 도중에 멈춰도 그 파일을 끝까지 읽지 않는다 — 멈춘 뒤의 읽기는 그때 하던 한 번뿐이다. 100 ms 는 release
/// 빌드에서만 판정한다([`RELEASE_TIMING`]).
#[test]
fn stopping_ends_a_long_file_read_too() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    // 일치가 없는 8 MiB. 검색기는 64 KiB 쯤씩 읽으므로 읽기가 100 번이 넘는다.
    write(&root.join("big.txt"), "filler line\n".repeat(700_000));
    let mut plan = fx.fs.search_plan(&params(&id, &root, "needle")).unwrap();
    plan.read_delay = Some(Duration::from_millis(5));
    let reads = Arc::clone(&plan.reads);
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || plan.run(&stop, &mut |_| true))
    };
    // 읽기가 시작된 뒤에 멈춘다.
    let started = Instant::now();
    while reads.load(Ordering::Relaxed) < 3 {
        assert!(started.elapsed() < Duration::from_secs(20), "no reads");
        std::thread::sleep(Duration::from_millis(1));
    }
    let stopped = Instant::now();
    stop.store(true, Ordering::Relaxed);
    let at_stop = reads.load(Ordering::Relaxed);
    let summary = worker.join().unwrap();
    let took = stopped.elapsed();
    let after_stop = reads.load(Ordering::Relaxed) - at_stop;
    assert!(after_stop <= 1, "{after_stop} reads after the stop");
    assert_eq!(summary.searched_files, 1);
    if RELEASE_TIMING {
        assert!(took < Duration::from_millis(100), "took {took:?}");
    }
}

/// RPC 로: 청크 형식, 마지막 `done`, 취소하면 `stream.end` 가 오고 검색 스레드도 남은 파일을 찾지 않고 멈춘다. 100 ms 는 release
/// 빌드에서만 판정한다([`RELEASE_TIMING`]).
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
            "lines": [{"line": 1, "preview": "x needle", "previewStart": 0, "ranges": [{"start": 2, "end": 8}], "more": false}],
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
    // 끝까지 찾으면 256 × 20 ms / 스레드 8개 = 0.64초 넘게 걸린다. 쉬는 읽기는 도중에 멈추지 않으므로 스레드는 그때 쉬던 읽기를
    // 마친 뒤에 멈춘다 — 쉬는 시간이 100 ms 기준에 가까우면 그것만으로 기준을 넘는다(100 ms 로 쉬게 했을 때 macOS CI 에서 134 ms).
    let slow = root.join("slow");
    for i in 0..256 {
        write(&slow.join(format!("f{i}.txt")), "nothing\n");
    }
    let finished = Arc::new(Mutex::new(None::<Summary>));
    let mut router = Router::new();
    let (fs, flag) = (Arc::clone(&fx.fs), Arc::clone(&finished));
    router.stream(SearchTextMethod, move |_ctx, params, sink| {
        let (fs, flag) = (Arc::clone(&fs), Arc::clone(&flag));
        async move {
            stream(fs, params, &sink, |plan| {
                plan.read_delay = Some(Duration::from_millis(20));
                plan.on_finish = Some(Box::new(move |summary: &Summary| {
                    *flag.lock().unwrap() = Some(*summary);
                }));
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
        finished.lock().unwrap().is_none(),
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
    let summary = loop {
        if let Some(summary) = *finished.lock().unwrap() {
            break summary;
        }
        assert!(
            cancelled.elapsed() < Duration::from_secs(20),
            "the search thread did not end"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    let stopped = cancelled.elapsed();
    assert!(
        summary.searched_files < 256,
        "the search kept running after the cancel: {summary:?}"
    );
    if RELEASE_TIMING {
        assert!(
            ended < Duration::from_millis(100),
            "stream.end took {ended:?}"
        );
        assert!(
            stopped < Duration::from_millis(100),
            "the search thread took {stopped:?}"
        );
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
    let both = Exclude {
        vendored: true,
        generated: true,
    };
    let labels = Labels::default();
    for pass in ["cold", "warm"] {
        let started = Instant::now();
        let excluded = files
            .iter()
            .filter(|f| labels.excluded(f, &root, both))
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

// ── 긴 줄 · 메모리 · 동시 검색 ──────────────────────────────────

pub(crate) const MIB: usize = 1 << 20;

/// 힙 할당 계수. 한 시험 안에서 쓴 힙의 최고치를 잰다. nextest 는 시험마다 프로세스가 따로라 다른 시험의 할당이 섞이지 않는다
/// (`cargo test` 처럼 한 프로세스에서 나란히 돌리면 섞여 크게 나올 수 있다).
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(n: usize) {
    let live = LIVE.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: 할당은 모두 `System` 이 하고 여기서는 크기만 센다.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size > layout.size() {
                grew(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

#[global_allocator]
static HEAP: Counting = Counting;

/// `f` 가 도는 동안 힙이 시작보다 더 쓴 최고치(바이트).
fn heap_peak<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let r = f();
    (r, PEAK.load(Ordering::Relaxed).saturating_sub(base))
}

/// `f` 를 다른 스레드에서 돌려, `within` 안에 끝나고 그동안 이 프로세스의 힙이 시작보다 `limit` 넘게 늘지 않는지 본다. 넘거나 늦으면
/// 그 자리에서 실패한다 — 고장 난 코드가 `/dev/zero` 를 끝없이 읽어도 기계의 메모리를 먹기 전에 멈추고, FIFO 에서 영영 막혀도 시험이
/// 끝난다(nextest 는 시험이 끝나면 프로세스를 끝내 남은 스레드도 함께 끝난다). 힙은 다른 스레드(감시 스레드 등)의 것도 센다.
#[cfg(unix)]
pub(crate) fn bounded<R: Send + 'static>(
    limit: usize,
    within: Duration,
    f: impl FnOnce() -> R + Send + 'static,
) -> R {
    let base = LIVE.load(Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    let deadline = Instant::now() + within;
    loop {
        let grown = LIVE.load(Ordering::Relaxed).saturating_sub(base);
        assert!(grown < limit, "heap grew by {} MiB", grown / MIB);
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok(r) => return r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                assert!(Instant::now() < deadline, "not done within {within:?}");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("the work panicked"),
        }
    }
}

fn literal(query: &str) -> RegexMatcher {
    RegexMatcherBuilder::new()
        .fixed_strings(true)
        .line_terminator(Some(b'\n'))
        .build(query)
        .unwrap()
}

/// 50 MiB 한 줄의 결과를 만드는 데 드는 힙이 미리보기 크기에 비례한다 — 일치가 5천만 개여도, 일치가 줄 끝에 하나여도.
#[test]
fn a_50_mib_line_costs_memory_in_proportion_to_the_preview() {
    let stop = AtomicBool::new(false);
    let all_a = vec![b'a'; READ_ONLY_MAX as usize - 1];
    let (result, peak) = heap_peak(|| line_result(&literal("a"), 1, &all_a, &stop));
    assert!(peak < MIB, "peak {peak}");
    assert_eq!(result.preview_start, 0);
    assert_eq!(result.preview.len(), PREVIEW_MAX);
    assert_eq!(result.ranges.len(), PREVIEW_MAX);
    assert_eq!(result.ranges[0], SearchRange { start: 0, end: 1 });
    assert!(result.more, "더 있음");

    let mut tail = vec![b'b'; READ_ONLY_MAX as usize - 7];
    tail.extend_from_slice(b"needle\n");
    let n = tail.len() - 1;
    let (result, peak) = heap_peak(|| line_result(&literal("needle"), 1, &tail, &stop));
    assert!(peak < MIB, "peak {peak}");
    assert_eq!(result.preview_start as usize, n - PREVIEW_MAX);
    assert!(result.preview.ends_with("needle"));
    let end = n as u32;
    assert_eq!(
        result.ranges,
        [SearchRange {
            start: end - 6,
            end
        }]
    );
    assert!(!result.more);

    // 멈추라면 범위를 더 모으지 않는다(줄 안 반복도 멈춤을 본다).
    stop.store(true, Ordering::Relaxed);
    let result = line_result(&literal("a"), 1, &all_a, &stop);
    assert!(result.ranges.is_empty(), "{}", result.ranges.len());
}

/// 50 MiB 한 줄 파일 하나를 찾는 동안의 힙 최고치. 검색기는 줄 하나를 통째로 버퍼에 담는다(`rg` 와 같다) — 그 버퍼 말고는 거의
/// 쓰지 않는다.
#[test]
fn searching_a_50_mib_one_line_file_stays_within_memory() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(
        &root.join("long.txt"),
        vec![b'a'; READ_ONLY_MAX as usize - 1],
    );
    let ((files, summary), peak) = heap_peak(|| search(&fx, &params(&id, &root, "a")));
    assert!(peak < 200 * MIB, "peak {} MiB", peak / MIB);
    assert_eq!(summary.lines, 1);
    assert!(files[0].lines[0].more);
}

/// 긴 한 줄 파일들을 찾는 도중에 멈춰도 100 ms 안에 끝난다 — 줄 하나의 결과를 만드는 데 오래 걸리지 않는다. 일치가 2백만 개인
/// 2 MiB 줄이면 일치를 모두 모으던 구현은 (시험 빌드에서) 1초 가까이 걸린다. 멈춘 뒤 남는 일(검색기가 버퍼에서 줄 끝을 찾는 것)은
/// 줄 길이에 비례해 짧다. 시간은 release 빌드에서만 판정한다([`RELEASE_TIMING`]) — debug 빌드에서는 끝나는지만 본다(줄 하나의
/// 결과가 미리보기 크기에 비례하는 것은 `a_50_mib_line_costs_memory_in_proportion_to_the_preview`).
#[test]
fn stopping_ends_long_line_files_within_100_ms() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for i in 0..4 {
        write(&root.join(format!("long/{i}.txt")), vec![b'a'; 2 * MIB]);
    }
    let plan = fx
        .fs
        .search_plan(&params(&id, &root.join("long"), "a"))
        .unwrap();
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
    rx.recv_timeout(Duration::from_secs(60))
        .expect("a first result");
    let stopped = Instant::now();
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    let took = stopped.elapsed();
    if RELEASE_TIMING {
        assert!(took < Duration::from_millis(100), "took {took:?}");
    }
}

/// 상태가 폭발하는 정규식(lazy DFA 캐시가 한도까지 자란다)으로 찾아도 힙이 한도 안이다.
#[test]
fn an_exploding_regex_stays_within_memory() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    for i in 0..8 {
        let mut text = String::new();
        for _ in 0..4000 {
            for _ in 0..120 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push(if seed & 1 == 0 { '0' } else { '1' });
            }
            text.push('\n');
        }
        write(&root.join(format!("bits/{i}.txt")), text);
    }
    let p = SearchTextParams {
        regex: true,
        ..params(&id, &root, "[01]*1[01]{30}2")
    };
    let ((files, _), peak) = heap_peak(|| search(&fx, &p));
    assert!(files.is_empty());
    assert!(peak < 64 * MIB, "peak {} MiB", peak / MIB);
}

/// 컴파일한 크기가 한도를 넘는 정규식은 invalid params 다(grep-regex 의 기본 한도로는 컴파일되는 것).
#[test]
fn a_regex_over_the_size_limit_is_invalid_params() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    let p = SearchTextParams {
        regex: true,
        ..params(&id, &root, TOO_LARGE)
    };
    let e = fx.fs.search_plan(&p).err().expect("rejected");
    assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "{e:?}");
    assert!(
        RegexMatcherBuilder::new().build(TOO_LARGE).is_ok(),
        "grep-regex 의 기본 한도로는 컴파일된다"
    );
}

const TOO_LARGE: &str = r"\w{500}";

/// 검색 하나는 많아야 [`SEARCH_THREADS`] 스레드로 찾는다.
#[test]
fn a_search_uses_at_most_8_threads() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for i in 0..256 {
        write(&root.join(format!("f{i}.txt")), "needle\n");
    }
    let mut plan = fx.fs.search_plan(&params(&id, &root, "needle")).unwrap();
    // 읽기마다 쉬어 모든 스레드가 일을 받게 한다.
    plan.read_delay = Some(Duration::from_millis(5));
    let mut threads = std::collections::HashSet::new();
    plan.run(&AtomicBool::new(false), &mut |_| {
        threads.insert(std::thread::current().id());
        true
    });
    let cores = std::thread::available_parallelism().unwrap().get();
    assert_eq!(threads.len(), cores.min(SEARCH_THREADS), "{threads:?}");
    assert_eq!(SEARCH_THREADS, 8);
}

/// 동시 검색은 [`MAX_SEARCHES`] 개까지 돈다. 넘는 것은 앞 검색이 끝나야(취소 포함) 시작한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn searches_beyond_the_limit_wait_for_a_turn() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for i in 0..256 {
        write(&root.join(format!("slow/f{i}.txt")), "nothing\n");
    }
    let started = Arc::new(AtomicUsize::new(0));
    let mut router = Router::new();
    let (fs, counter) = (Arc::clone(&fx.fs), Arc::clone(&started));
    router.stream(SearchTextMethod, move |_ctx, params, sink| {
        let (fs, counter) = (Arc::clone(&fs), Arc::clone(&counter));
        async move {
            stream(fs, params, &sink, move |plan| {
                counter.fetch_add(1, Ordering::Relaxed);
                plan.read_delay = Some(Duration::from_millis(20));
            })
            .await
        }
    });
    let (session, mut rx) =
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    let p = params(&id, &root.join("slow"), "needle");
    for n in 1..=MAX_SEARCHES + 1 {
        session
            .handle_value(json!({"jsonrpc": "2.0", "id": n, "method": "search.text", "params": p}));
    }
    let mut streams = Vec::new();
    while streams.len() < MAX_SEARCHES + 1 {
        let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let Some(response) = message.into_response() {
            let opened: StreamOpened = response.result_as().unwrap();
            streams.push(opened.stream_id);
        }
    }
    for s in &streams {
        session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": s, "credit": 100}}));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while started.load(Ordering::Relaxed) < MAX_SEARCHES {
        assert!(
            Instant::now() < deadline,
            "the first searches did not start"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        started.load(Ordering::Relaxed),
        MAX_SEARCHES,
        "one too many ran"
    );
    // 하나를 취소하면 기다리던 것이 곧 시작한다.
    session.handle_value(
        json!({"jsonrpc": "2.0", "method": "stream.cancel", "params": {"streamId": streams[0]}}),
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while started.load(Ordering::Relaxed) < MAX_SEARCHES + 1 {
        assert!(
            Instant::now() < deadline,
            "the waiting search did not start"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ── `.git` · 줄 끝 · 위의 ignore 파일 ─────────────────────────────

/// `.git` 안은 링크로도, 찾는 경로로 직접 줘도 찾지 않는다(실제 경로로 판정한다).
#[cfg(unix)]
#[test]
fn dot_git_is_not_searched_through_links_or_direct_paths() {
    use std::os::unix::fs::symlink;

    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join(".git/config"), "needle\n");
    write(&root.join("a.txt"), "needle\n");
    symlink(root.join(".git"), root.join("gl")).unwrap();
    symlink(root.join(".git/config"), root.join("config.txt")).unwrap();

    let only_a = owned(&[("a.txt", &[1])]);
    assert_eq!(hits(&fx, &root, &params(&id, &root, "needle")), only_a);
    let p = SearchTextParams {
        hidden: true,
        ..params(&id, &root, "needle")
    };
    assert_eq!(hits(&fx, &root, &p), only_a);
    for path in [".git", ".git/config", "gl", "config.txt"] {
        let e = fx
            .fs
            .search_plan(&params(&id, &root.join(path), "needle"))
            .err()
            .expect(path);
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "{path}: {e:?}");
    }
    // 걷기도 `.git` 을 가리키는 링크로 들어가지 않는다(아래 열기 판정에 기대지 않는다).
    let plan = fx.fs.search_plan(&params(&id, &root, "needle")).unwrap();
    let walked = walked(&plan);
    assert!(walked.contains(&root.join("a.txt")), "{walked:?}");
    assert!(
        !walked
            .iter()
            .any(|p| p.starts_with(root.join("gl")) || p.ends_with("config.txt")),
        "{walked:?}"
    );
    // 걷기 판정 뒤 링크로 바뀐 경우: 연 핸들의 실제 경로가 `.git` 안이면 읽지 않는다.
    let roots = Roots::new([root.clone()]);
    assert_eq!(
        open(&roots, &root.join("config.txt")).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
}

/// 외톨이 CR 도 줄 끝이다(판정 · 에디터와 같다). CRLF 는 한 번만 센다. 미리보기에 `\r` 은 없다.
#[test]
fn a_lone_cr_ends_a_line_like_the_editor() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(
        &root.join("a.txt"),
        "one\rneedle\r\nthree\rneedle\nneedle\r",
    );
    let (files, _) = search(&fx, &params(&id, &root, "needle"));
    let lines: Vec<(u64, &str)> = files[0]
        .lines
        .iter()
        .map(|l| (l.line, l.preview.as_str()))
        .collect();
    assert_eq!(lines, [(2, "needle"), (4, "needle"), (5, "needle")]);
}

/// `\r` 에 맞는 정규식도 첫 일치를 잃지 않는다 — 범위는 보이는 줄 끝에서 자른다.
#[test]
fn a_match_on_the_cr_of_crlf_keeps_its_range() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&root.join("a.txt"), "ab\r\n");
    let ranges = |query: &str| {
        let p = SearchTextParams {
            regex: true,
            ..params(&id, &root, query)
        };
        let (files, _) = search(&fx, &p);
        assert_eq!(files[0].lines[0].preview, "ab");
        files[0].lines[0].ranges.clone()
    };
    assert_eq!(ranges(r"b\r"), [SearchRange { start: 1, end: 2 }]);
    assert_eq!(ranges(r"\r"), [SearchRange { start: 2, end: 2 }]);
}

/// 외톨이 CR 바꾸기는 읽기 크기와 상관없다(버퍼 끝의 `\r` 은 다음 바이트를 보고 정한다).
#[test]
fn the_lone_cr_reader_works_across_reads() {
    /// 한 번에 한 바이트씩 주는 읽기.
    struct Trickle<'a>(&'a [u8]);
    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.0.len().min(buf.len()).min(1);
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }
    let input = b"a\rb\r\nc\r\r\nd\r";
    let expected = b"a\nb\r\nc\n\r\nd\n";
    let mut out = Vec::new();
    LoneCr::new(&input[..]).read_to_end(&mut out).unwrap();
    assert_eq!(out, expected);
    let mut out = Vec::new();
    LoneCr::new(Trickle(input)).read_to_end(&mut out).unwrap();
    assert_eq!(out, expected);
    // 한 바이트씩 꺼내도.
    let mut reader = LoneCr::new(Trickle(input));
    let mut out = Vec::new();
    let mut one = [0u8; 1];
    while reader.read(&mut one).unwrap() == 1 {
        out.push(one[0]);
    }
    assert_eq!(out, expected);
}

fn project(fx: &Fixture, root: &Path) -> String {
    fs::create_dir_all(root).unwrap();
    fx.projects
        .create(NewProject {
            root: root.to_path_buf(),
            ..NewProject::default()
        })
        .unwrap()
        .id
}

/// 루트가 더 큰 저장소의 하위 폴더면 그 저장소 꼭대기까지의 `.gitignore` · `info/exclude` · `.gitattributes` 를 따른다. 꼭대기
/// 위(홈 폴더 등)의 ignore 파일은 읽지 않는다. 루트 안의 `.ignore` 는 찾는 폴더 위에 있어도 따른다.
#[test]
fn ignore_files_above_the_path_count_up_to_its_repo_top_only() {
    let fx = Fixture::new();
    let outer = fx.base.join("outer");
    write(&fx.base.join(".gitignore"), "kept.txt\n");
    write(&fx.base.join(".ignore"), "kept.txt\n");
    write(&outer.join(".git/info/exclude"), "excluded.txt\n");
    write(&outer.join(".gitignore"), "ignored.txt\n");
    write(
        &outer.join(".gitattributes"),
        "proj/third/** linguist-vendored\n",
    );
    let root = outer.join("proj");
    let id = project(&fx, &root);
    write(&root.join(".ignore"), "skip.txt\n");
    // 루트 안의 `.gitignore` 도 따른다 — 걷기는 위의 `.git` 을 보지 못하지만 저장소 안이다. 그 안의 다른 저장소(`nested`)에는
    // 위 저장소의 `.gitignore` 가 닿지 않는다(git 과 같다).
    write(&root.join(".gitignore"), "local.txt\n");
    fs::create_dir_all(root.join("nested/.git")).unwrap();
    write(&root.join("nested/local.txt"), "needle\n");
    for f in [
        "a.txt",
        "kept.txt",
        "ignored.txt",
        "excluded.txt",
        "third/x.txt",
        "local.txt",
        "sub/local.txt",
        "sub/skip.txt",
        "sub/b.txt",
    ] {
        write(&root.join(f), "needle\n");
    }
    let p = SearchTextParams {
        exclude_vendored: true,
        ..params(&id, &root, "needle")
    };
    assert_eq!(
        hits(&fx, &root, &p),
        owned(&[
            ("a.txt", &[1]),
            ("kept.txt", &[1]),
            ("nested/local.txt", &[1]),
            ("sub/b.txt", &[1])
        ])
    );
    // 하위 폴더만 찾아도 위(루트 · 저장소)의 규칙이 적용된다.
    write(&root.join("sub/ignored.txt"), "needle\n");
    let p = params(&id, &root.join("sub"), "needle");
    assert_eq!(hits(&fx, &root, &p), owned(&[("sub/b.txt", &[1])]));
    // 포함 글롭에 맞으면 ignore 를 넘는다(걷기 안의 규칙과 같다).
    let p = SearchTextParams {
        include: vec!["ignored.txt".into()],
        ..params(&id, &root, "needle")
    };
    assert_eq!(
        hits(&fx, &root, &p),
        owned(&[("ignored.txt", &[1]), ("sub/ignored.txt", &[1])])
    );
    // ignore 를 끄면 모두.
    let p = SearchTextParams {
        respect_ignore: false,
        ..params(&id, &root, "needle")
    };
    assert_eq!(hits(&fx, &root, &p).len(), 11);
}

/// 루트의 `.git` 이 다른 저장소의 git 디렉터리를 가리키는 링크면 그 저장소의 `info/exclude` · `info/attributes` 를 읽지 않는다
/// (루트 밖 링크는 따라가지 않는다, 결정 M1-D8).
#[cfg(unix)]
#[test]
fn a_dot_git_link_does_not_bring_in_another_repos_rules() {
    let fx = Fixture::new();
    let other = fx.base.join("other");
    write(&other.join(".git/info/exclude"), "a.txt\n");
    write(&other.join(".git/info/attributes"), "* linguist-vendored\n");
    let root = fx.base.join("proj");
    let id = project(&fx, &root);
    std::os::unix::fs::symlink(other.join(".git"), root.join(".git")).unwrap();
    write(&root.join("a.txt"), "needle\n");
    let p = SearchTextParams {
        exclude_vendored: true,
        ..params(&id, &root, "needle")
    };
    assert_eq!(hits(&fx, &root, &p), owned(&[("a.txt", &[1])]));
}

/// 걷기가 내놓는 항목(찾는 경로 첫째).
#[cfg(unix)]
fn walked(plan: &Plan) -> Vec<PathBuf> {
    let found = Mutex::new(Vec::new());
    plan.walker(&plan.targets[0]).run(|| {
        let found = &found;
        Box::new(move |entry| {
            if let Ok(entry) = entry {
                found.lock().unwrap().push(entry.into_path());
            }
            WalkState::Continue
        })
    });
    let mut found = found.into_inner().unwrap();
    found.sort();
    found
}

/// 걷기 안의 ignore 파일이 `/dev/zero` · FIFO · 루트 밖 파일을 가리키는 링크면 읽지 않고(없는 것으로 보고) 검색이 제때 끝난다.
#[cfg(unix)]
#[test]
fn ignore_files_that_link_to_devices_fifos_or_outside_are_not_read() {
    use std::os::unix::fs::symlink;

    let fx = Fixture::new();
    let (id, root) = fx.repo();
    write(&fx.base.join("outside.gitignore"), "a.txt\n");
    let fifo = fx.base.join("fifo");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 경로 문자열이다.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    symlink("/dev/zero", root.join(".gitignore")).unwrap();
    for (dir, target) in [("fifo", &fifo), ("out", &fx.base.join("outside.gitignore"))] {
        write(&root.join(dir).join("a.txt"), "needle\n");
        symlink(target, root.join(dir).join(".gitignore")).unwrap();
        symlink(target, root.join(dir).join(".ignore")).unwrap();
    }
    write(&root.join("a.txt"), "needle\n");

    let (tx, rx) = std::sync::mpsc::channel();
    let fs = Arc::clone(&fx.fs);
    let p = params(&id, &root, "needle");
    std::thread::spawn(move || {
        let plan = fs.search_plan(&p).unwrap();
        let ((files, _), peak) = heap_peak(|| {
            let mut files = Vec::new();
            let summary = plan.run(&AtomicBool::new(false), &mut |f| {
                files.push(f.path);
                true
            });
            (files, summary)
        });
        let _ = tx.send((files.len(), peak));
    });
    let (found, peak) = rx.recv_timeout(Duration::from_secs(10)).expect("in time");
    assert_eq!(found, 3);
    assert!(peak < 64 * MIB, "peak {} MiB", peak / MIB);
}

/// 차례를 너무 오래 기다리면 「바쁨」 오류로 끝난다 — 막힌 검색이 있어도 다른 검색이 영영 기다리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_that_waits_too_long_for_a_turn_fails_as_busy() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    let held = SEARCHES.acquire_many(MAX_SEARCHES as u32).await.unwrap();
    let mut router = Router::new();
    register(&mut router, Arc::clone(&fx.fs));
    let (session, mut rx) =
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    let started = Instant::now();
    session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": "search.text", "params": params(&id, &root, "x")}));
    let opened: StreamOpened = rx
        .recv()
        .await
        .unwrap()
        .into_response()
        .unwrap()
        .result_as()
        .unwrap();
    session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened.stream_id, "credit": 10}}));
    let note = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap()
        .into_notification()
        .unwrap();
    assert_eq!(note.method, STREAM_END);
    match note.params_as::<StreamEnd>().unwrap().outcome {
        pw_rpc::protocol::StreamOutcome::Failed { error } => {
            assert!(
                error
                    .data
                    .as_ref()
                    .is_some_and(|d| d.to_string().contains("busy")),
                "{error:?}"
            );
        }
        other => panic!("expected failed, got {other:?}"),
    }
    assert!(started.elapsed() >= SEARCH_WAIT, "{:?}", started.elapsed());
    drop(held);
}

/// 사람의 전역 gitignore(`core.excludesFile`)는 저장소 안에서만 적용되고, `/` 로 시작하는 패턴은 저장소 꼭대기 기준이다.
#[test]
fn the_global_gitignore_applies_inside_repos_only() {
    let fx = Fixture::new();
    let config = fx.base.join("gitconfig");
    let excludes = fx.base.join("global-ignore");
    write(&excludes, "*.glob\n/rooted.txt\n");
    write(
        &config,
        format!("[core]\n\texcludesFile = {}\n", excludes.display()),
    );
    // SAFETY: nextest 는 시험마다 프로세스가 따로이고, 이 시험은 아직 다른 스레드를 띄우지 않았다.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &config) };
    let (id, root) = fx.repo();
    for f in ["a.glob", "rooted.txt", "sub/rooted.txt", "b.txt"] {
        write(&root.join(f), "needle\n");
    }
    assert_eq!(
        hits(&fx, &root, &params(&id, &root, "needle")),
        owned(&[("b.txt", &[1]), ("sub/rooted.txt", &[1])])
    );
    // 저장소 밖 프로젝트에는 적용하지 않는다(`.gitignore` 와 같다).
    let plain = fx.base.join("plain");
    let plain_id = project(&fx, &plain);
    write(&plain.join("a.glob"), "needle\n");
    assert_eq!(
        hits(&fx, &plain, &params(&plain_id, &plain, "needle")),
        owned(&[("a.glob", &[1])])
    );
}

/// 5 MiB 넘는 파일은 Host 전체에서 둘씩만 함께 찾고, 그 줄 버퍼는 그 파일이 끝나면 버린다 — 긴 한 줄 파일 여럿이 스레드마다
/// 줄 버퍼(6 MiB 줄이면 8 MiB)를 쥐고 있지 않게.
#[test]
fn big_files_are_searched_two_at_a_time() {
    let fx = Fixture::new();
    let (id, root) = fx.repo();
    for i in 0..8 {
        write(
            &root.join(format!("big/{i}.txt")),
            vec![b'a'; EDITABLE_MAX as usize + MIB],
        );
    }
    let mut plan = fx
        .fs
        .search_plan(&params(&id, &root.join("big"), "a"))
        .unwrap();
    // 읽기마다 쉬어 여러 파일이 함께 읽히게 한다.
    plan.read_delay = Some(Duration::from_millis(2));
    let base = LIVE.load(Ordering::Relaxed);
    // 파일 하나를 마치고 보낼 때 살아 있는 힙: 그 파일의 버퍼는 이미 버려졌고, 많아야 다른 큰 파일 하나가 읽히는 중이다.
    let mut most = 0;
    let summary = plan.run(&AtomicBool::new(false), &mut |_| {
        most = most.max(LIVE.load(Ordering::Relaxed).saturating_sub(base));
        true
    });
    assert_eq!(summary.matched_files, 8);
    assert_eq!(BIG_FILES.peak.load(Ordering::Relaxed), BIG_FILES_AT_ONCE);
    assert!(most < 20 * MIB, "live {} MiB", most / MIB);
}
