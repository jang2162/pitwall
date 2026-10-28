//! 결정 M2-D1 의 되돌림 판정(`docs/design/13-m2-plan.md` M2-05): 5,000줄 · 커밋 1천 개 파일의 blame 첫 청크 · 전체 시간을 gix ·
//! git CLI 둘로 잰다. 파일 히스토리(`git.fileHistory`)의 시간도 함께 잰다.
//!
//! ```text
//! cargo run --release -p pw-git --features test-support --example blame_timing -- <폴더> [반복=5] [줄=5000] [커밋=1000]
//! ```
//!
//! 저장소가 없으면 `<폴더>/repo` 에 만든다(`pw_git::testing::blame::load`, 시드 고정). 잰 것(반복마다 저장소를 새로 연다 — OS 파일
//! 캐시는 데워진 채다):
//!
//! - gix: RPC 와 같은 경로(미신뢰로 열기 → 대상 풀기 → `blame::native::run`). 첫 청크 = 전체 뒤다(gix-blame 은 다 만든 뒤 돌려준다).
//! - git: `git blame --incremental -M --no-textconv <커밋> -- <경로>`(RPC 와 같은 인자) 를 띄워 RPC 와 같은 해석기 · 청크로(`blame::incremental::pump`). 대상 풀기
//!   (gix)를 포함한다. 참고로 `-M` 없는 것도 잰다.
//! - 파일 히스토리: `file_history::run`(그 경로의 처음까지).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use pw_git::blame::incremental::{ARGS, Emit, pump};
use pw_git::blame::{BlameChunk, BlameEngine, BlameFile, native, subject};
use pw_git::testing::blame::load;
use pw_git::testing::repo::git_command;

const PATH: &str = "lib/big.txt";
const SEED: u64 = 0x5eed_2026_1027;

/// 한 번 잰 것: (첫 청크, 전체, 항목 수).
type Measure<'a> = dyn Fn() -> (Duration, Duration, usize) + 'a;

struct Timed {
    started: Instant,
    first: Option<Duration>,
    ranges: usize,
}

impl Emit for Timed {
    async fn emit(&mut self, chunk: BlameChunk) -> bool {
        self.first.get_or_insert(self.started.elapsed());
        self.ranges += chunk.ranges.len();
        true
    }
}

fn open(dir: &Path) -> gix::Repository {
    pw_git::repo::discover(dir, false)
        .expect("open the repository")
        .expect("a repository")
}

/// 엔진 `git`(`moves` 면 `-M`): (첫 청크, 전체, 범위 수).
fn cli(dir: &Path, ceiling: &Path, moves: bool) -> (Duration, Duration, usize) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();
    let repo = open(dir);
    let subject = subject(&repo, None, PATH).unwrap().unwrap();
    let mut command = git_command(dir, ceiling);
    // RPC 와 같은 인자. `-M` 없는 참고 측정은 그것만 뺀다.
    command.args(ARGS);
    if moves {
        command.arg("-M");
    }
    command.args([&subject.commit.to_string(), "--", PATH]);
    let file = BlameFile {
        commit: subject.commit.to_string(),
        lines: subject.lines,
        engine: BlameEngine::Git,
        moves,
    };
    rt.block_on(async move {
        let mut child = tokio::process::Command::from(command)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut timed = Timed {
            started,
            first: None,
            ranges: 0,
        };
        let drained = pump(stdout, file, &mut timed).await.unwrap().unwrap();
        assert!(child.wait().await.unwrap().success());
        if let Some(chunk) = drained.finish().unwrap() {
            timed.emit(chunk).await;
        }
        (timed.first.unwrap(), started.elapsed(), timed.ranges)
    })
}

/// 엔진 `gix`: (첫 청크, 전체, 범위 수).
fn gix(dir: &Path) -> (Duration, Duration, usize) {
    let started = Instant::now();
    let repo = open(dir);
    let subject = subject(&repo, None, PATH).unwrap().unwrap();
    let mut first = None;
    let mut ranges = 0;
    native::run(
        &repo,
        false,
        &subject,
        &AtomicBool::new(false),
        &mut |chunk| {
            first.get_or_insert(started.elapsed());
            ranges += chunk.ranges.len();
            true
        },
    )
    .unwrap();
    (first.unwrap(), started.elapsed(), ranges)
}

/// 파일 히스토리: (첫 청크, 전체, 행 수).
fn history(dir: &Path) -> (Duration, Duration, usize) {
    let started = Instant::now();
    let repo = open(dir);
    let start = repo.head_id().unwrap().detach();
    let mut first = None;
    let mut rows = 0;
    pw_git::file_history::run(
        &repo,
        false,
        start,
        PATH.into(),
        &AtomicBool::new(false),
        &mut |chunk| {
            first.get_or_insert(started.elapsed());
            rows += chunk.rows.len();
            true
        },
    )
    .unwrap();
    (first.unwrap(), started.elapsed(), rows)
}

fn ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(base) = args.first().map(PathBuf::from) else {
        eprintln!("usage: blame_timing <dir> [runs=5] [lines=5000] [commits=1000]");
        std::process::exit(2);
    };
    let runs: usize = args.get(1).map_or(5, |n| n.parse().expect("runs"));
    let lines: usize = args.get(2).map_or(5000, |n| n.parse().expect("lines"));
    let commits: usize = args.get(3).map_or(1000, |n| n.parse().expect("commits"));
    let dir = base.join("repo");
    if !dir.exists() {
        let t = Instant::now();
        load(&dir, &base, lines, commits, SEED).expect("create the repository");
        println!(
            "created {lines} lines x {commits} commits in {} ms",
            ms(t.elapsed())
        );
    }
    let measures: [(&str, &Measure<'_>); 4] = [
        ("gix", &|| gix(&dir)),
        ("git -M", &|| cli(&dir, &base, true)),
        ("git (no -M)", &|| cli(&dir, &base, false)),
        ("fileHistory", &|| history(&dir)),
    ];
    println!("engine\trun\tfirst ms\ttotal ms\titems");
    for (name, f) in measures {
        let (mut firsts, mut totals) = (Vec::new(), Vec::new());
        for run in 1..=runs {
            let (first, total, items) = f();
            println!("{name}\t{run}\t{}\t{}\t{items}", ms(first), ms(total));
            firsts.push(first);
            totals.push(total);
        }
        println!(
            "{name}\tmedian\t{}\t{}",
            ms(median(firsts)),
            ms(median(totals))
        );
    }
}
