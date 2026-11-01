//! 완료 기준 ④ · 결정 M4-D19 ⑤: 합성 저장소(파일 2만 · 심볼 50만)의 첫 색인 시간 · 색인 메모리 · 질의 키 입력당 시간.
//!
//! release 에서만 판정한다 — 기본 시험에서 빠진다(`#[ignore]`). 돌리는 법(CONTRIBUTING.md 「명령」):
//!
//! ```sh
//! cargo test --release -p pw-index --test perf -- --ignored --nocapture --test-threads 1
//! ```
//!
//! - **저장소**: `CARGO_TARGET_TMPDIR/perf-index-v3/` 에 한 번 만들고 다시 쓴다(여섯 언어 — Rust · TypeScript · JavaScript ·
//!   Python · Java · Kotlin — 를 돌려 가며 파일마다 심볼 25개: 타입 하나 + 메서드 24개, Rust 는 구조체 · impl · 메서드 23개). 메서드마다
//!   문서 주석과 본문 열 줄쯤 — 심볼당 바이트를 이 저장소의 실제 코드 비율(약 450 B, M4-15 리뷰 L3)에 맞췄다. 만든 직후라 파일이
//!   OS 캐시에 있다 — 콜드 디스크보다 낙관적이다(콜드 디스크는 M4-19).
//! - **「8 코어」**: 파싱 · 매칭 스레드를 7(코어 8 − 1)로 묶는다(`SymbolIndex::with_threads`) — 이 기기의 코어가 더 많아도.
//! - **첫 색인**: 창이 `fs.watch` 를 연 것처럼 감시를 잡은 때부터 심볼 색인이 다 만들어지고 쉴 때까지(파일 목록 걷기 포함)의 벽시계.
//! - **메모리**: 같은 구간의 Rust 힙 증가(이 시험 바이너리의 전역 할당자가 센다 — tree-sitter 의 C 할당은 들지 않지만 트리 · 파서는
//!   파일마다 버린다)와 프로세스 RSS 증가(`ps`, 할당자 단편화 · C 할당 포함). 파일 목록 인덱스(같은 감시가 만든다)도 든다.
//! - **질의**: 색인이 준비된 뒤 `handleItem1234x5` 를 한 글자씩 늘려 가며(키 입력마다 요청 하나) `index.symbols.search` 를 부른 시간 —
//!   대소문자 섞인 질의 · 흔한 한 글자 질의를 함께. 키 입력마다 30 ms 안이어야 한다.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use pw_core::TaskSpawner;
use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry as Projects};
use pw_ext::Registry;
use pw_fs::Fs;
use pw_fs::index::FileIndex;
use pw_fs::watch::{FsWatchParams, Watch};
use pw_index::SymbolIndex;
use pw_index::methods::IndexSymbolSearchParams;
use semver::Version;
use tokio_util::sync::CancellationToken;

/// 이 바이너리의 Rust 힙(살아 있는 바이트).
struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: 시스템 할당자에 그대로 넘기고 크기만 센다.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const FILES: usize = 20_000;
/// 목표의 기준 기기(코어 8).
const EIGHT_CORES: usize = 8;
const SYMBOLS_PER_FILE: usize = 25;
const TARGET_FIRST_INDEX: Duration = Duration::from_secs(15);
const TARGET_MEMORY_BYTES: isize = 120 * 1024 * 1024;
const TARGET_QUERY: Duration = Duration::from_millis(30);

/// 메서드 하나의 본문(문장 여러 줄 — 심볼당 바이트를 이 저장소의 실제 코드 비율 400 ~ 500 B 에 맞춘다, 리뷰 L3). `{in}` 은 들여쓰기.
fn body(lang: usize) -> &'static str {
    match lang {
        0 => {
            "        // Doubles the input, then folds a few steps of noise into it.\n        let doubled = input * 2;\n        let mut total = doubled;\n        for step in 0..input.min(16) {\n            total = total.wrapping_add(step * 3 + 1);\n            if total % 7 == 0 {\n                total /= 2;\n            }\n        }\n        let label = format!(\"{total}:{doubled}\");\n        total + label.len() as u64\n"
        }
        1 => {
            "    // Doubles the input, then folds a few steps of noise into it.\n    const doubled = input * 2;\n    let total = doubled;\n    for (let step = 0; step < Math.min(input, 16); step++) {\n      total += step * 3 + 1;\n      if (total % 7 === 0) {\n        total = Math.floor(total / 2);\n      }\n    }\n    const label = `${total}:${doubled}`;\n    return total + label.length;\n"
        }
        2 => {
            "        # Doubles the value, then folds a few steps of noise into it.\n        doubled = value * 2\n        total = doubled\n        for step in range(min(value, 16)):\n            total += step * 3 + 1\n            if total % 7 == 0:\n                total //= 2\n        label = f\"{total}:{doubled}\"\n        return total + len(label)\n"
        }
        3 => {
            "        // Doubles the input, then folds a few steps of noise into it.\n        int doubled = input * 2;\n        int total = doubled;\n        for (int step = 0; step < Math.min(input, 16); step++) {\n            total += step * 3 + 1;\n            if (total % 7 == 0) {\n                total /= 2;\n            }\n        }\n        String label = total + \":\" + doubled;\n        return total + label.length();\n"
        }
        4 => {
            "        // Doubles the input, then folds a few steps of noise into it.\n        val doubled = input * 2\n        var total = doubled\n        for (step in 0 until minOf(input, 16)) {\n            total += step * 3 + 1\n            if (total % 7 == 0) {\n                total /= 2\n            }\n        }\n        val label = \"$total:$doubled\"\n        return total + label.length\n"
        }
        _ => {
            "    // Doubles the input, then folds a few steps of noise into it.\n    const doubled = input * 2;\n    let total = doubled;\n    for (let step = 0; step < Math.min(input, 16); step++) {\n      total += step * 3 + 1;\n      if (total % 7 === 0) {\n        total = Math.floor(total / 2);\n      }\n    }\n    const label = `${total}:${doubled}`;\n    return total + label.length;\n"
        }
    }
}

/// 파일 `i` 의 (상대 경로, 내용). 심볼은 정확히 [`SYMBOLS_PER_FILE`] 개.
fn file(i: usize) -> (String, String) {
    let dir = format!("pkg{:03}/mod{:02}", i / 200, (i / 10) % 20);
    let methods = SYMBOLS_PER_FILE - 1;
    let mut s = String::new();
    let verbs = ["handle", "render", "parse", "load", "update", "compute"];
    let name = |k: usize| format!("{}Item{i}x{k}", verbs[k % verbs.len()]);
    let lang = i % 6;
    let b = body(lang);
    match lang {
        0 => {
            let _ = writeln!(s, "pub struct Widget{i};\n\nimpl Widget{i} {{");
            for k in 0..methods - 1 {
                let _ = writeln!(
                    s,
                    "    /// Computes one item of the widget.\n    pub fn {}(&self, input: u64) -> u64 {{\n{b}    }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/widget{i}.rs"), s)
        }
        1 => {
            let _ = writeln!(s, "export class Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "  /** Computes one item of the widget. */\n  {}(input: number): number {{\n{b}  }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/widget{i}.ts"), s)
        }
        2 => {
            let _ = writeln!(s, "class Widget{i}:");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "    def {}(self, value):\n        \"\"\"Computes one item of the widget.\"\"\"\n{b}",
                    name(k)
                );
            }
            (format!("{dir}/widget{i}.py"), s)
        }
        3 => {
            let _ = writeln!(s, "package demo;\n\npublic class Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "    /** Computes one item of the widget. */\n    public int {}(int input) {{\n{b}    }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/Widget{i}.java"), s)
        }
        4 => {
            let _ = writeln!(s, "package demo\n\nclass Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "    /** Computes one item of the widget. */\n    fun {}(input: Int): Int {{\n{b}    }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/Widget{i}.kt"), s)
        }
        _ => {
            let _ = writeln!(s, "export class Widget{i} {{");
            for k in 0..methods {
                let _ = writeln!(
                    s,
                    "  /** Computes one item of the widget. */\n  {}(input) {{\n{b}  }}\n",
                    name(k)
                );
            }
            s.push_str("}\n");
            (format!("{dir}/widget{i}.js"), s)
        }
    }
}

/// 합성 저장소. 다 만든 뒤 표식을 쓰고, 표식이 있으면 다시 만들지 않는다.
fn repository() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("perf-index-v3");
    let marker = dir.join(".pitwall-perf-ready");
    if marker.exists() {
        return dir;
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let mut bytes = 0usize;
    for i in 0..FILES {
        let (rel, text) = file(i);
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        bytes += text.len();
        std::fs::write(path, text).unwrap();
    }
    std::fs::write(&marker, bytes.to_string()).unwrap();
    dir
}

fn rss_bytes() -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kib: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kib * 1024)
}

fn machine() -> String {
    let cpu = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let model = std::process::Command::new("sysctl")
        .args(["-n", "hw.model"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    format!("{model} · {cpu} · {cores} logical cores")
}

fn mb(bytes: isize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "release 측정 — `cargo test --release -p pw-index --test perf -- --ignored --nocapture --test-threads 1`"]
async fn twenty_thousand_files_index_within_the_targets() {
    if cfg!(debug_assertions) {
        panic!("release 에서 돌린다(--release)");
    }
    let made = Instant::now();
    let root = simplify(std::fs::canonicalize(repository()).unwrap());
    println!("machine: {}", machine());
    let bytes: u64 = std::fs::read_to_string(root.join(".pitwall-perf-ready"))
        .ok()
        .and_then(|b| b.trim().parse().ok())
        .unwrap_or(0);
    println!(
        "repository: {} ({:.1} MB, {} B per symbol, ready in {:?})",
        root.display(),
        bytes as f64 / 1e6,
        bytes / (FILES * SYMBOLS_PER_FILE) as u64,
        made.elapsed()
    );
    println!("threads: {} (8-core condition)", EIGHT_CORES - 1);

    let app = tempfile::tempdir().unwrap();
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = Projects::new(AppPaths::under(app.path()), None, bus.clone());
    let fs = Arc::new(Fs::new());
    fs.attach(projects.clone());
    let id = projects
        .create(NewProject {
            root: root.clone(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
    // 「8 코어」 조건: 파싱 · 매칭 스레드를 코어 수 − 1 = 7 로 묶는다(이 기기의 코어가 더 많아도).
    let symbols =
        SymbolIndex::new(Arc::clone(&fs), Arc::new(registry)).with_threads(EIGHT_CORES - 1);
    let tasks = TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new());
    let files = FileIndex::new(Arc::clone(&fs));
    files.observe(symbols.observer());
    tokio::spawn(files.attach(&bus, tasks.clone()));
    symbols.attach(tasks);
    let watch = Watch::new(Arc::clone(&fs));
    watch.attach(bus.clone());
    watch.attach_index(files.clone());
    // 언어 · 쿼리를 올리는 것(Host 가 사는 동안 한 번)은 색인 메모리에 넣지 않는다.
    for ext in ["a.rs", "a.ts", "a.py", "a.java", "a.kt", "a.js"] {
        symbols.parse_text(ext, "");
    }

    let heap_before = LIVE.load(Ordering::Relaxed);
    let rss_before = rss_bytes();
    let started = Instant::now();
    let _lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    while !symbols.ready(&id, &root).await {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(symbols.idle(&id, &root).await);
    let first_index = started.elapsed();
    let heap = LIVE.load(Ordering::Relaxed) - heap_before;
    let rss = rss_bytes()
        .zip(rss_before)
        .map(|(a, b)| a as isize - b as isize);
    let (count, estimate) = symbols.footprint();

    println!("first index: {first_index:?} (target < {TARGET_FIRST_INDEX:?})");
    println!(
        "memory: heap +{:.1} MiB (target ≤ {:.0} MiB), rss {}, symbol index estimate {:.1} MiB",
        mb(heap),
        mb(TARGET_MEMORY_BYTES),
        rss.map_or("unknown".to_owned(), |r| format!("+{:.1} MiB", mb(r))),
        mb(estimate as isize)
    );
    println!("symbols: {count}");
    assert_eq!(
        count,
        (FILES * SYMBOLS_PER_FILE) as u64,
        "합성 저장소의 심볼 수"
    );

    // 질의: 키 입력마다 요청 하나.
    let word = "handleItem1234x5";
    let mut queries: Vec<String> = (1..=word.len()).map(|n| word[..n].to_owned()).collect();
    queries.extend(["r", "Widget", "wdg", "compute item 199", "zzzz"].map(str::to_owned));
    let search = |query: &str| IndexSymbolSearchParams {
        project_id: id.clone(),
        query: query.to_owned(),
        ..IndexSymbolSearchParams::default()
    };
    // 한 번 데운다(첫 질의의 스레드 · 페이지 폴트).
    symbols.search(search("h")).await.unwrap();
    let mut times = Vec::new();
    for q in &queries {
        let t = Instant::now();
        let found = symbols.search(search(q)).await.unwrap();
        let took = t.elapsed();
        println!("query {q:?}: {took:?} ({} matched)", found.matched);
        times.push(took);
    }
    times.sort();
    let max = *times.last().unwrap();
    let median = times[times.len() / 2];
    println!("query per keystroke: median {median:?}, max {max:?} (target < {TARGET_QUERY:?})");

    assert!(
        first_index < TARGET_FIRST_INDEX,
        "first index {first_index:?}"
    );
    assert!(heap <= TARGET_MEMORY_BYTES, "heap +{:.1} MiB", mb(heap));
    assert!(max < TARGET_QUERY, "slowest query {max:?}");
    drop(app);
    let _: &Path = &root;
}
