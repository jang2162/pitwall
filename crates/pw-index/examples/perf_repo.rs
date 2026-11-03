//! `cargo run --release -p pw-index --example perf_repo -- <폴더>`: 심볼 인덱스 성능의 합성 저장소(파일 2만 · 심볼 50만)를 `<폴더>` 에
//! 만든다 — release 시험 `tests/perf.rs` 와 같은 저장소다(`tests/support/perf_repo.rs`). M4-19 가 앱으로 첫 색인 · 질의를 잴 때
//! `scripts/perf-m4.mjs` 가 부른다. 표식이 있으면 다시 만들지 않는다.

#[path = "../tests/support/perf_repo.rs"]
mod perf_repo;

fn main() {
    let Some(dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: perf_repo <dir>");
        std::process::exit(2);
    };
    let dir = perf_repo::ensure(std::path::Path::new(&dir));
    let bytes = std::fs::read_to_string(dir.join(perf_repo::MARKER)).unwrap_or_default();
    println!(
        "{} ({} files, {} symbols, {} source bytes)",
        dir.display(),
        perf_repo::FILES,
        perf_repo::FILES * perf_repo::SYMBOLS_PER_FILE,
        bytes.trim()
    );
}
