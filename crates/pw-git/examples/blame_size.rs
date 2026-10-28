//! 상한(`READ_ONLY_MAX`, 50 MiB) 가까운 파일의 blame 시간을 텍스트 · 바이너리(줄마다 NUL)로 잰다 — 엔진 `gix` 가 바이너리도 텍스트와
//! 같게 견주므로(`blame::native` 의 `text_cache`) 시간 · 메모리도 같은 상한 안이어야 한다(`docs/design/13-m2-plan.md` M2-05).
//!
//! ```text
//! cargo run --release -p pw-git --features test-support --example blame_size -- <폴더> <text|binary> <gix|git>
//! /usr/bin/time -l target/release/examples/blame_size <폴더> binary gix   # 최대 RSS(엔진 `git` 은 git 프로세스의 것이 빠진다)
//! ```
//!
//! 저장소가 없으면 `<폴더>/repo` 에 만든다: 그 파일(약 48 MiB, 64바이트 줄 75만 개)을 커밋 셋으로 — 둘째는 1,000줄마다, 셋째는
//! 1,000줄마다 500줄 밀린 자리를 바꾼다. 한 번 blame 하고 걸린 시간 · 범위 수를 찍는다.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use pw_git::blame::incremental::{ARGS, Emit, pump};
use pw_git::blame::{BlameChunk, BlameEngine, BlameFile, native, subject};
use pw_git::testing::repo::git_command;

const LINES: usize = 750_000;

fn content(binary: bool, round: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(LINES * 64);
    for i in 0..LINES {
        let changed = match round {
            1 => i % 1000 == 0,
            2 => i % 1000 == 500,
            _ => false,
        };
        let mark = if changed { round } else { 0 };
        let mut line = format!("{i:>12} {mark} {:>40}", "payload-of-the-line").into_bytes();
        if binary {
            line[12] = 0;
            line.push(0xff);
        }
        line.resize(63, b'.');
        line.push(b'\n');
        out.extend_from_slice(&line);
    }
    out
}

fn make(dir: &Path, ceiling: &Path, file: &str, binary: bool) {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        let out = git_command(dir, ceiling).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    for round in 0..3 {
        std::fs::write(dir.join(file), content(binary, round)).unwrap();
        git(&["add", file]);
        git(&["commit", "-q", "-m", &format!("round {round}")]);
    }
}

struct Count(usize);

impl Emit for Count {
    async fn emit(&mut self, chunk: BlameChunk) -> bool {
        self.0 += chunk.ranges.len();
        true
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, base, kind, engine] = &args[..] else {
        panic!("usage: blame_size <folder> <text|binary> <gix|git>");
    };
    let binary = kind == "binary";
    let file = if binary { "big.bin" } else { "big.txt" };
    let base = Path::new(base);
    let dir = base.join(format!("repo-{kind}"));
    if !dir.join(".git").exists() {
        make(&dir, base, file, binary);
    }
    let started = Instant::now();
    let repo = pw_git::repo::discover(&dir, false).unwrap().unwrap();
    let subject = subject(&repo, None, file).unwrap().unwrap();
    let ranges = if engine == "gix" {
        let mut ranges = 0;
        native::run(&repo, false, &subject, &AtomicBool::new(false), &mut |c| {
            ranges += c.ranges.len();
            true
        })
        .unwrap();
        ranges
    } else {
        let mut command = git_command(&dir, base);
        command
            .args(ARGS)
            .args([&subject.commit.to_string(), "--", file]);
        let blamed = BlameFile {
            commit: subject.commit.to_string(),
            lines: subject.lines,
            engine: BlameEngine::Git,
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let mut child = tokio::process::Command::from(command)
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                let stdout = child.stdout.take().unwrap();
                let mut count = Count(0);
                let drained = pump(stdout, blamed, &mut count).await.unwrap().unwrap();
                assert!(child.wait().await.unwrap().success());
                if let Some(chunk) = drained.finish().unwrap() {
                    count.emit(chunk).await;
                }
                count.0
            })
    };
    println!(
        "{kind} {engine}: {} lines, {:.1} MiB, {ranges} ranges, {} ms",
        subject.lines,
        std::fs::metadata(dir.join(file)).unwrap().len() as f64 / 1048576.0,
        started.elapsed().as_millis()
    );
}
