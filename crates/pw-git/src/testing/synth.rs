//! 합성 이력(`docs/design/13-m2-plan.md` M2-03): 브랜치 · 머지 · 문어발 머지 · 고아 브랜치가 섞인 저장소를 `git fast-import` 로
//! 만든다. 같은 시드면 같은 저장소(같은 해시)다 — 커밋 시각 · 작성자 · 내용을 모두 시드에서 정한다.
//!
//! 10만 커밋 저장소는 `examples/synth_history.rs` 가 만들고, 시험은 작은 저장소로 필터 · 순서를 git 과 맞춘다.
//!
//! - 시각: 커밋마다 0~90초 늘린다(같은 시각이 섞인다 — 순서의 동률 처리를 본다). 자식은 언제나 부모보다 늦다.
//! - 커밋마다 파일 하나(`d<a>/s<b>/f<c>.txt`, 1,000개 중 하나)를 고친다. 머지는 30% 가 파일 하나를 고친다.
//! - 브랜치는 동시에 24개까지, 머지된 브랜치는 반쯤 지운다. 태그(가벼운 것 · 주석 단 것)와 원격 ref(`refs/remotes/origin/*`)도 있다.
//!
//! [`fanout`] 은 다른 모양이다: 곧은 `main` 곳곳에서 커밋 하나씩 갈라진 원격 브랜치가 수천 개 쌓인 저장소(레인 상한을 본다).

use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use super::repo::git_command;

/// 기본 시드. 보고의 수치는 이 시드로 잰다.
pub const SEED: u64 = 0x5eed_2026_1026;

/// 첫 커밋의 시각(초).
const EPOCH: i64 = 1_600_000_000;

const AUTHORS: [(&str, &str); 8] = [
    ("Ada Lovelace", "ada@pitwall.invalid"),
    ("Grace Hopper", "grace@pitwall.invalid"),
    ("Linus Torvalds", "linus@pitwall.invalid"),
    ("Junio Hamano", "junio@pitwall.invalid"),
    ("Margaret Hamilton", "margaret@pitwall.invalid"),
    ("Ken Thompson", "ken@pitwall.invalid"),
    ("Barbara Liskov", "barbara@pitwall.invalid"),
    ("Dennis Ritchie", "dennis@pitwall.invalid"),
];

const WORDS: [&str; 12] = [
    "parser", "lexer", "cache", "graph", "render", "index", "socket", "buffer", "config", "theme",
    "search", "watch",
];

const KINDS: [&str; 4] = ["feat", "fix", "refactor", "docs"];

/// 동시에 살아 있는 브랜치 상한(`main` 포함).
const MAX_BRANCHES: usize = 24;

/// SplitMix64 — 의존성 없이 시드로 정해지는 수열.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `0..n`.
    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// 만든 저장소의 개요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub commits: usize,
    pub merges: usize,
    pub octopus: usize,
    pub orphans: usize,
    /// 남은 브랜치(`refs/heads/*`) 수.
    pub branches: usize,
    pub tags: usize,
}

struct Branch {
    name: String,
    /// 끝 커밋의 mark. 아직 커밋이 없는 새 브랜치는 갈라진 커밋의 mark 다.
    tip: usize,
}

/// fast-import 입력과 지울 브랜치 · 개요.
struct Script {
    stream: Vec<u8>,
    deleted: Vec<String>,
    summary: Summary,
}

fn script(commits: usize, seed: u64) -> Script {
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(commits * 220);
    let mut time = EPOCH;
    let mut mark = 0usize;
    let mut branches = vec![Branch {
        name: "main".to_owned(),
        tip: 0,
    }];
    let mut deleted = Vec::new();
    let mut summary = Summary {
        commits: 0,
        merges: 0,
        octopus: 0,
        orphans: 0,
        branches: 0,
        tags: 0,
    };
    let mut topics = 0usize;
    while mark < commits {
        time += rng.below(91) as i64;
        let roll = rng.below(1000);
        // 고를 일: (커밋할 브랜치, 머지할 브랜치들, 고아인가).
        let (target, sources, orphan) = if mark == 0 {
            (0, Vec::new(), true)
        } else if roll < 40 && branches.len() < MAX_BRANCHES {
            // 새 브랜치: 살아 있는 브랜치 끝이나 최근 커밋 하나에서 갈라진다.
            let from = if rng.below(2) == 0 {
                branches[rng.below(branches.len())].tip
            } else {
                mark - rng.below(mark.min(2000))
            };
            topics += 1;
            branches.push(Branch {
                name: format!("topic/{topics}"),
                tip: from,
            });
            (branches.len() - 1, Vec::new(), false)
        } else if roll < 100 && branches.len() > 1 {
            let target = if rng.below(10) < 6 {
                0
            } else {
                rng.below(branches.len())
            };
            let mut source = rng.below(branches.len());
            if source == target {
                source = (source + 1) % branches.len();
            }
            (target, vec![source], false)
        } else if roll < 103 && branches.len() > 3 {
            let target = 0;
            let mut sources = Vec::new();
            for _ in 0..(2 + rng.below(2)) {
                let s = 1 + rng.below(branches.len() - 1);
                if !sources.contains(&s) {
                    sources.push(s);
                }
            }
            (target, sources, false)
        } else if roll == 103 && summary.orphans < 3 {
            topics += 1;
            branches.push(Branch {
                name: format!("orphan/{topics}"),
                tip: 0,
            });
            (branches.len() - 1, Vec::new(), true)
        } else {
            let target = if rng.below(2) == 0 {
                0
            } else {
                rng.below(branches.len())
            };
            (target, Vec::new(), false)
        };
        // 같은 커밋을 두 번 부모로 두지 않는다(갈라진 뒤 아직 커밋이 없는 브랜치끼리는 끝이 같다).
        let mut tips = vec![branches[target].tip];
        let sources: Vec<usize> = sources
            .into_iter()
            .filter(|&s| {
                let fresh = !tips.contains(&branches[s].tip);
                tips.push(branches[s].tip);
                fresh
            })
            .collect();
        mark += 1;
        let (name, email) = AUTHORS[rng.below(AUTHORS.len())];
        let kind = KINDS[rng.below(KINDS.len())];
        let word = WORDS[rng.below(WORDS.len())];
        let message = if sources.is_empty() {
            let mut m = format!("{kind}: {word} change {mark}\n");
            if rng.below(4) == 0 {
                m.push_str(&format!("\nTouches the {word} again.\n"));
            }
            m
        } else {
            let names: Vec<&str> = sources.iter().map(|&s| branches[s].name.as_str()).collect();
            format!(
                "Merge {} into {}\n",
                names.join(", "),
                branches[target].name
            )
        };
        let touch = sources.is_empty() || rng.below(10) < 3;
        let file = rng.below(1000);
        let branch = &branches[target];
        if orphan {
            let _ = writeln!(out, "reset refs/heads/{}", branch.name);
        }
        let _ = writeln!(out, "commit refs/heads/{}\nmark :{mark}", branch.name);
        let _ = writeln!(out, "author {name} <{email}> {time} +0000");
        let _ = writeln!(out, "committer {name} <{email}> {time} +0000");
        let _ = write!(out, "data {}\n{message}", message.len());
        if !orphan {
            let _ = writeln!(out, "from :{}", branch.tip);
        }
        for &s in &sources {
            let _ = writeln!(out, "merge :{}", branches[s].tip);
        }
        if touch {
            let content = format!("{mark}\n");
            let _ = write!(
                out,
                "M 100644 inline d{}/s{}/f{}.txt\ndata {}\n{content}",
                file / 100,
                file / 10 % 10,
                file % 10,
                content.len()
            );
        }
        out.push(b'\n');
        summary.commits += 1;
        if orphan {
            summary.orphans += 1;
        }
        match sources.len() {
            0 => {}
            1 => summary.merges += 1,
            _ => summary.octopus += 1,
        }
        branches[target].tip = mark;
        // 머지된 브랜치는 반쯤 지운다(`main` 은 남긴다). 뒤에서부터 지워야 번호가 밀리지 않는다.
        let mut gone: Vec<usize> = sources
            .into_iter()
            .filter(|&s| s != 0 && rng.below(2) == 0)
            .collect();
        gone.sort_unstable();
        for s in gone.into_iter().rev() {
            deleted.push(branches.remove(s).name);
        }
        // 2,000 커밋마다 `main` 에 태그. 짝수 번째는 주석 단 태그.
        if mark.is_multiple_of(2000) {
            let n = mark / 2000;
            let tip = branches[0].tip;
            if n.is_multiple_of(2) {
                let msg = format!("release {n}\n");
                let _ = write!(
                    out,
                    "tag v{n}\nfrom :{tip}\ntagger {name} <{email}> {time} +0000\ndata {}\n{msg}\n",
                    msg.len()
                );
            } else {
                let _ = write!(out, "reset refs/tags/v{n}\nfrom :{tip}\n\n");
            }
            summary.tags += 1;
        }
    }
    // 원격 ref: `main` 과 살아 있는 브랜치 몇 개를 조금 뒤처진 자리로.
    let main_tip = branches[0].tip;
    let _ = write!(
        out,
        "reset refs/remotes/origin/main\nfrom :{}\n\n",
        main_tip.saturating_sub(5).max(1)
    );
    for b in branches.iter().skip(1).take(3) {
        if !b.name.starts_with("orphan/") {
            let _ = write!(
                out,
                "reset refs/remotes/origin/{}\nfrom :{}\n\n",
                b.name, b.tip
            );
        }
    }
    let _ = writeln!(out, "done");
    // 아직 커밋이 없는 새 브랜치는 fast-import 가 ref 를 만들지 않았다(이름만 있었다).
    summary.branches = branches.len();
    Script {
        stream: out,
        deleted,
        summary,
    }
}

/// `dir` 에 합성 저장소를 만든다(`git init` → fast-import → 머지된 브랜치 지우기 → `repack -ad`). commit-graph 는 쓰지 않는다 —
/// 필요하면 부른 쪽이 [`write_commit_graph`] 를 부른다. 위로 찾기는 `ceiling` 에서 멈춘다(사람의 저장소 · 설정을 건드리지 않는다).
pub fn create(dir: &Path, ceiling: &Path, commits: usize, seed: u64) -> std::io::Result<Summary> {
    let Script {
        stream,
        deleted,
        mut summary,
    } = script(commits.max(1), seed);
    import(dir, ceiling, &stream)?;
    if !deleted.is_empty() {
        let mut child = git_command(dir, ceiling)
            .args(["update-ref", "--stdin"])
            .stdin(Stdio::piped())
            .spawn()?;
        let mut input = String::new();
        for name in &deleted {
            input.push_str(&format!("delete refs/heads/{name}\n"));
        }
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input.as_bytes())?;
        check(child.wait_with_output()?, "update-ref")?;
    }
    finish(dir, ceiling)?;
    let heads = run(git_command(dir, ceiling).args(["for-each-ref", "--format=x", "refs/heads"]))?;
    summary.branches = heads.lines().count();
    Ok(summary)
}

/// `dir` 에 곧은 `main` 커밋 `commits` 개와, 그 곳곳(시드로 고른 커밋)에서 커밋 하나씩 갈라진 원격 브랜치
/// `refs/remotes/origin/b<n>` `branches` 개를 만든다 — 원격 브랜치가 쌓인 저장소 모양. 시각은 `main` 이 커밋마다 60초, 갈래
/// 커밋은 모두 `main` 끝보다 늦다(오래된 커밋 위에 최근 만든 갈래 — 로그 맨 위에서 줄이 모두 시작해 갈라진 커밋까지 내려가므로
/// 레인이 갈래 수만큼 열린다).
pub fn fanout(
    dir: &Path,
    ceiling: &Path,
    commits: usize,
    branches: usize,
    seed: u64,
) -> std::io::Result<()> {
    let mut rng = Rng(seed);
    let commits = commits.max(1);
    let mut out = Vec::with_capacity((commits + branches) * 200);
    let time = |mark: usize| EPOCH + 60 * mark as i64;
    for mark in 1..=commits {
        let (name, email) = AUTHORS[mark % AUTHORS.len()];
        let message = format!("main {mark}\n");
        let _ = writeln!(out, "commit refs/heads/main\nmark :{mark}");
        let _ = writeln!(out, "author {name} <{email}> {} +0000", time(mark));
        let _ = writeln!(out, "committer {name} <{email}> {} +0000", time(mark));
        let _ = write!(out, "data {}\n{message}", message.len());
        if mark > 1 {
            let _ = writeln!(out, "from :{}", mark - 1);
        }
        let content = format!("{mark}\n");
        let _ = write!(
            out,
            "M 100644 inline main.txt\ndata {}\n{content}\n",
            content.len()
        );
    }
    for n in 0..branches {
        let mark = commits + 1 + n;
        let from = 1 + rng.below(commits);
        let (name, email) = AUTHORS[rng.below(AUTHORS.len())];
        let message = format!("branch {n}\n");
        let _ = writeln!(out, "commit refs/remotes/origin/b{n}\nmark :{mark}");
        let at = time(commits) + 30 * (n as i64 + 1);
        let _ = writeln!(out, "author {name} <{email}> {at} +0000");
        let _ = writeln!(out, "committer {name} <{email}> {at} +0000");
        let _ = write!(out, "data {}\n{message}from :{from}\n", message.len());
        let content = format!("{n}\n");
        let _ = write!(
            out,
            "M 100644 inline b/{n}.txt\ndata {}\n{content}\n",
            content.len()
        );
    }
    let _ = writeln!(out, "done");
    import(dir, ceiling, &out)?;
    finish(dir, ceiling)
}

/// `git init` 하고 fast-import 입력 `stream` 을 들인다.
pub(super) fn import(dir: &Path, ceiling: &Path, stream: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    run(git_command(dir, ceiling).args(["init", "--quiet"]))?;
    let mut child = git_command(dir, ceiling)
        .args(["fast-import", "--quiet", "--done"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    child.stdin.take().expect("piped stdin").write_all(stream)?;
    check(child.wait_with_output()?, "fast-import").map(drop)
}

/// 팩을 하나로 묶고 작업 트리를 `main` 에 맞춘다.
pub(super) fn finish(dir: &Path, ceiling: &Path) -> std::io::Result<()> {
    run(git_command(dir, ceiling).args(["-c", "repack.writeBitmaps=false", "repack", "-adq"]))?;
    // fast-import 는 HEAD 가 가리키는 `main` 을 체크아웃하지 않는다 — 작업 트리를 맞춘다(status 가 깨끗하게).
    run(git_command(dir, ceiling).args(["reset", "--quiet", "--hard", "main"])).map(drop)
}

/// 실기 측정 픽스처([`perf_fixture`])의 큰 파일 줄 수 — 그중 짝수 줄 절반을 바꾼다.
pub const PERF_LINES: usize = 20_000;

/// 실기 측정 픽스처의 큰 파일(커밋 상세 탭이 첫 파일로 고르도록 파일 하나만 바꾼다).
pub const PERF_PATH: &str = "perf/big.rs";

/// 실기 측정 픽스처의 체크아웃 대상 브랜치. `main` 과 같은 커밋이라 체크아웃은 HEAD 만 바꾼다.
pub const PERF_BRANCH: &str = "perf-checkout";

/// [`perf_fixture`] 가 더한 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfFixture {
    /// 1만 줄을 바꾼 커밋(`main` 의 끝 — 로그 첫 행).
    pub commit: String,
}

/// 실기 측정(`crates/pw-perf` 의 git 시나리오, CONTRIBUTING.md 「성능 기준선」)이 쓰는 커밋 둘과 브랜치 하나를 `main` 위에 더한다:
/// [`PERF_PATH`] 를 [`PERF_LINES`] 줄로 만드는 커밋, 그 짝수 줄 1만 개를 바꾸는 커밋(M2-15 의 「1만 줄 바뀐 파일」 — 앱 쪽
/// `apps/desktop/perf/diff/diff-bench.tsx` 와 같은 꼴), 그 커밋을 가리키는 [`PERF_BRANCH`](M2-13 의 체크아웃). 시각은 합성 이력의
/// 어느 커밋보다 늦은 고정값이라 같은 저장소면 같은 해시다. commit-graph 를 쓸 저장소면 이것을 먼저 부른다(새 커밋이 그래프에 든다).
/// 저장소 설정에 `maintenance.auto=false` · `gc.auto=0` 을 남긴다 — 「commit-graph 없음」 이 그대로 남게.
pub fn perf_fixture(dir: &Path, ceiling: &Path) -> std::io::Result<PerfFixture> {
    let before: Vec<String> = (0..PERF_LINES)
        .map(|i| format!("    let value_{i} = {i};"))
        .collect();
    let after: Vec<String> = before
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i % 2 == 0 {
                format!("    let value_{i} = {i} + 1;")
            } else {
                line.clone()
            }
        })
        .collect();
    // git 2.54 는 commit 뒤에 `git maintenance run --auto --detach` 를 띄워 commit-graph 를 쓴다 — 「commit-graph 없음」 저장소가 백그라운드
    // 에서 조용히 「있음」 이 된다. 이 저장소에서 그 뒤에 도는 git(측정하는 쪽의 체크아웃 포함)도 쓰지 않게 저장소 설정에 끈다.
    for (key, value) in [("maintenance.auto", "false"), ("gc.auto", "0")] {
        run(git_command(dir, ceiling).args(["config", key, value]))?;
    }
    let path = dir.join(PERF_PATH);
    std::fs::create_dir_all(path.parent().expect("a parent folder"))?;
    // 합성 이력은 커밋마다 90초까지 늘어 100만 커밋이어도 이 시각에 닿지 않는다.
    for (step, (lines, message)) in [
        (&before, "perf: add big.rs"),
        (&after, "perf: change 10,000 lines"),
    ]
    .into_iter()
    .enumerate()
    {
        std::fs::write(&path, format!("{}\n", lines.join("\n")))?;
        let at = format!("{} +0000", EPOCH + 100_000_000 + 60 * step as i64);
        run(git_command(dir, ceiling).args(["add", PERF_PATH]))?;
        run(git_command(dir, ceiling)
            .env("GIT_AUTHOR_DATE", &at)
            .env("GIT_COMMITTER_DATE", &at)
            .args(["commit", "--quiet", "-m", message]))?;
    }
    run(git_command(dir, ceiling).args(["branch", PERF_BRANCH]))?;
    let commit = run(git_command(dir, ceiling).args(["rev-parse", "HEAD"]))?;
    Ok(PerfFixture {
        commit: commit.trim().to_owned(),
    })
}

/// `git commit-graph write --reachable`.
pub fn write_commit_graph(dir: &Path, ceiling: &Path) -> std::io::Result<()> {
    run(git_command(dir, ceiling).args(["commit-graph", "write", "--reachable"])).map(drop)
}

fn run(command: &mut std::process::Command) -> std::io::Result<String> {
    let out = command.output()?;
    let what = format!("{:?}", command.get_args().collect::<Vec<_>>());
    check(out, &what)
}

fn check(out: std::process::Output, what: &str) -> std::io::Result<String> {
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(std::io::Error::other(format!(
            "git {what} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, ceiling: &Path, args: &[&str]) -> String {
        run(git_command(dir, ceiling).args(args)).unwrap()
    }

    /// 픽스처는 `main` 끝에 1만 줄을 바꾼 파일 하나짜리 커밋을 두고, 같은 커밋에 체크아웃용 브랜치를 단다. 같은 저장소면 같은 해시다.
    #[test]
    fn the_perf_fixture_changes_ten_thousand_lines_of_one_file_on_main() {
        let tmp = tempfile::tempdir().unwrap();
        let ceiling = std::fs::canonicalize(tmp.path()).unwrap();
        let mut commits = Vec::new();
        let mut dirs = Vec::new();
        for name in ["a", "b"] {
            let dir = ceiling.join(name);
            // 200 커밋: git 의 자동 유지보수는 그래프 밖 커밋이 100 개를 넘어야 commit-graph 를 쓴다.
            create(&dir, &ceiling, 200, SEED).unwrap();
            let fixture = perf_fixture(&dir, &ceiling).unwrap();
            assert_eq!(
                git(&dir, &ceiling, &["rev-parse", "main"]).trim(),
                fixture.commit
            );
            assert_eq!(
                git(
                    &dir,
                    &ceiling,
                    &["log", "-1", "--format=%H", "--date-order", "--all"]
                )
                .trim(),
                fixture.commit,
                "로그 첫 행이 아니다"
            );
            assert_eq!(
                git(&dir, &ceiling, &["diff", "--numstat", "HEAD~1", "HEAD"]),
                format!("10000\t10000\t{PERF_PATH}\n")
            );
            assert_eq!(
                git(&dir, &ceiling, &["rev-parse", PERF_BRANCH]).trim(),
                fixture.commit
            );
            assert_eq!(
                git(&dir, &ceiling, &["symbolic-ref", "--short", "HEAD"]).trim(),
                "main"
            );
            assert_eq!(git(&dir, &ceiling, &["status", "--porcelain"]), "");
            commits.push(fixture.commit);
            dirs.push(dir);
        }
        assert_eq!(commits[0], commits[1]);
        // 커밋 뒤 백그라운드 유지보수가 commit-graph 를 쓰지 않았다(돌았다면 끝날 틈을 준다).
        std::thread::sleep(std::time::Duration::from_millis(1500));
        for dir in &dirs {
            let info = dir.join(".git/objects/info");
            assert!(
                !info.join("commit-graph").exists() && !info.join("commit-graphs").exists(),
                "{} got a commit-graph",
                dir.display()
            );
        }
    }
}
