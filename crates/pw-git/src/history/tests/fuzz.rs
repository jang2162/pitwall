//! 차등 퍼즈: 무작위 저장소 상태(파일 · 폴더 · 링크 · 실행 비트 · ignore · sparse · assume-unchanged · skip-worktree · 서브모듈 ·
//! 대소문자 · 이름 바뀜 · 중첩 저장소 · 스테이징)를 만들고 [`super::differ::check`] 로 미리보기가 git 이 실제로 잃는 것을 하나도
//! 놓치지 않는지 본다. 시드가 고정이라 같은 시드는 같은 상태를 만든다 — 실패하면 시드와 만든 순서를 보인다.
//!
//! 기본 시험은 조각 넷 × 10 개다. 긴 판(조각마다 `PW_RESET_FUZZ_CASES` 개)과 시드 하나 다시 돌리기:
//!
//! ```text
//! PW_RESET_FUZZ_CASES=200 cargo nextest run -p pw-git -E 'test(/history::tests::fuzz::/)'
//! PW_RESET_FUZZ_SEED=<시드> PW_RESET_FUZZ_CASES=1 cargo nextest run -p pw-git -E 'test(/history::tests::fuzz::/)' --no-capture
//! ```

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::differ::check;
use crate::testing::repo::TestRepo;

/// 조각마다 기본 경우 수.
const CASES: u64 = 10;
/// 조각 시드의 간격.
const SHARD: u64 = 1_000_000;

fn cases() -> u64 {
    std::env::var("PW_RESET_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES)
}

/// 조각 `shard` 를 돈다. `PW_RESET_FUZZ_SEED` 가 있으면 조각 0 만 그 시드부터 돈다.
fn run_shard(shard: u64) {
    let (start, count) = match std::env::var("PW_RESET_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(_) if shard != 0 => return,
        Some(seed) => (seed, cases()),
        None => (1 + shard * SHARD, cases()),
    };
    let mut extra = 0;
    let mut lost = 0;
    let mut refused = 0;
    for seed in start..start + count {
        // 잡힌 출력이라 실패한 시험에서만 보인다(만들다 패닉해도 시드를 안다).
        eprintln!("seed {seed}");
        let case = Case::build(seed);
        let verdict = check(&case.repo, &case.target);
        if let Err(e) = verdict.complete() {
            panic!(
                "seed {seed} — rerun with PW_RESET_FUZZ_SEED={seed} PW_RESET_FUZZ_CASES=1\nsteps:\n  {}\nindex (ls-files -v -s):\n{}{e}",
                case.log.join("\n  "),
                case.repo.git(&["ls-files", "-v", "-s"]),
            );
        }
        for side in [&verdict.hard, &verdict.keep] {
            extra += side.extra();
            lost += side.lost.len();
            if let Some(why) = &side.refused {
                refused += 1;
                eprintln!(
                    "seed {seed}: the {:?} preview refused: {why}\n  {}",
                    side.mode,
                    case.log.join("\n  ")
                );
            }
        }
    }
    eprintln!(
        "shard {shard}: {count} cases from seed {start}, {lost} lost paths all listed, {extra} listed entries not lost, {refused} previews refused"
    );
}

#[test]
fn the_preview_misses_nothing_shard_0() {
    run_shard(0);
}

#[test]
fn the_preview_misses_nothing_shard_1() {
    run_shard(1);
}

#[test]
fn the_preview_misses_nothing_shard_2() {
    run_shard(2);
}

#[test]
fn the_preview_misses_nothing_shard_3() {
    run_shard(3);
}

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// 경로 성분. 같은 이름이 파일도 폴더도 되어 파일 ↔ 폴더 충돌이 잦다. `z.log` 는 ignore 규칙에 걸린다.
const PARTS: [&str; 4] = ["a", "b", "c", "z.log"];
/// `.gitignore` 에 넣을 규칙.
const RULES: [&str; 5] = ["z.log", "c", "b/", "*.log", "/a/c"];

fn random_path(rng: &mut Rng) -> String {
    let depth = 1 + rng.below(3);
    (0..depth)
        .map(|_| *rng.pick(&PARTS))
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    File { exec: bool },
    Link,
}

type Tree = BTreeMap<String, (Kind, String)>;

/// `path` 를 넣어도 파일 ↔ 폴더가 겹치지 않는가.
fn fits(tree: &Tree, path: &str) -> bool {
    tree.keys().all(|p| {
        p != path && !p.starts_with(&format!("{path}/")) && !path.starts_with(&format!("{p}/"))
    })
}

fn random_tree(rng: &mut Rng) -> Tree {
    let mut tree = Tree::new();
    for _ in 0..2 + rng.below(5) {
        let path = random_path(rng);
        if fits(&tree, &path) {
            let kind = if rng.one_in(6) {
                Kind::Link
            } else {
                Kind::File {
                    exec: rng.one_in(5),
                }
            };
            tree.insert(path, (kind, format!("v{}\n", rng.below(3))));
        }
    }
    if rng.one_in(2) {
        tree.insert(
            ".gitignore".into(),
            (Kind::File { exec: false }, rules(rng)),
        );
    }
    tree
}

fn rules(rng: &mut Rng) -> String {
    let mut out = String::new();
    for rule in RULES {
        if rng.one_in(3) {
            out.push_str(rule);
            out.push('\n');
        }
    }
    out
}

/// 한 커밋 뒤의 트리: 내용 · 실행 비트 · 종류를 바꾸고, 지우고, 더하고, 파일을 폴더로 · 폴더를 파일로.
fn mutate(rng: &mut Rng, tree: &Tree) -> Tree {
    let mut next = tree.clone();
    for _ in 0..1 + rng.below(4) {
        let keys: Vec<String> = next.keys().cloned().collect();
        match rng.below(7) {
            0 if !keys.is_empty() => {
                let k = rng.pick(&keys).clone();
                next.get_mut(&k).unwrap().1 = format!("w{}\n", rng.below(3));
            }
            1 if !keys.is_empty() => {
                next.remove(rng.pick(&keys));
            }
            2 if !keys.is_empty() => {
                let k = rng.pick(&keys).clone();
                let entry = next.get_mut(&k).unwrap();
                entry.0 = match entry.0 {
                    Kind::File { exec } if rng.one_in(2) => Kind::File { exec: !exec },
                    Kind::File { .. } => Kind::Link,
                    Kind::Link => Kind::File { exec: false },
                };
            }
            3 if !keys.is_empty() => {
                // 파일 → 폴더(`.gitignore` 는 파일로 둔다).
                let k = rng.pick(&keys).clone();
                if k == ".gitignore" {
                    continue;
                }
                let (kind, text) = next.remove(&k).unwrap();
                next.insert(format!("{k}/{}", rng.pick(&PARTS)), (kind, text));
            }
            4 if !keys.is_empty() => {
                // 폴더 → 파일: 경로의 조상 하나를 파일로.
                let k = rng.pick(&keys).clone();
                if let Some((dir, _)) = k.split_once('/') {
                    let dir = dir.to_owned();
                    next.retain(|p, _| !p.starts_with(&format!("{dir}/")));
                    next.insert(dir, (Kind::File { exec: false }, "was a folder\n".into()));
                }
            }
            5 => {
                next.insert(
                    ".gitignore".into(),
                    (Kind::File { exec: false }, rules(rng)),
                );
            }
            _ => {
                let path = random_path(rng);
                if fits(&next, &path) {
                    next.insert(path, (Kind::File { exec: false }, "added\n".into()));
                }
            }
        }
    }
    next
}

/// 경우 하나: 시험 저장소, 대상 커밋, 만든 순서.
struct Case {
    repo: TestRepo,
    /// 서브모듈의 원본(살려 둔다).
    _library: Option<TestRepo>,
    target: String,
    log: Vec<String>,
}

impl Case {
    fn build(seed: u64) -> Self {
        let mut rng = Rng(seed);
        let repo = TestRepo::new();
        let mut log = Vec::new();
        let x = random_tree(&mut rng);
        let y = mutate(&mut rng, &x);
        let mut commits = vec![materialize(&repo, &x, "x"), materialize(&repo, &y, "y")];
        log.push(format!("x = {x:?}"));
        log.push(format!("y = {y:?}"));
        let library = rng.one_in(5).then(|| {
            let library = TestRepo::with_commit();
            repo.git(&[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                library.root().to_str().unwrap(),
                "s",
            ]);
            commits.push(repo.commit_all("submodule"));
            log.push("w = y + submodule s".into());
            library
        });
        let head = rng.pick(&commits).clone();
        repo.git(&["reset", "-q", "--hard", &head]);
        let target = rng.pick(&commits).clone();
        log.push(format!(
            "HEAD = {}, target = {}",
            name(&commits, &head),
            name(&commits, &target)
        ));
        for n in 0..1 + rng.below(6) {
            let step = step(&mut rng, &repo, n, library.is_some());
            log.push(step);
        }
        Self {
            repo,
            _library: library,
            target,
            log,
        }
    }
}

fn name(commits: &[String], id: &str) -> &'static str {
    match commits.iter().position(|c| c == id) {
        Some(0) => "x",
        Some(1) => "y",
        _ => "w",
    }
}

/// 작업 트리를 `tree` 로 바꿔 모두(ignore 된 것도) 커밋한다.
fn materialize(repo: &TestRepo, tree: &Tree, message: &str) -> String {
    let root = repo.root();
    for entry in std::fs::read_dir(root).unwrap().flatten() {
        if entry.file_name() != ".git" {
            remove(&entry.path());
        }
    }
    for (path, (kind, text)) in tree {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        match kind {
            Kind::Link => std::os::unix::fs::symlink(text.trim(), &full).unwrap(),
            Kind::File { exec } => {
                std::fs::write(&full, text).unwrap();
                if *exec {
                    std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
        }
    }
    repo.git(&["add", "-A", "-f"]);
    repo.git(&["commit", "-q", "--allow-empty", "-m", message]);
    repo.head()
}

fn remove(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let _ = std::fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// `rel` 에 파일을 둘 수 있게 조상의 파일 · 링크와 그 자리의 폴더를 지운다.
fn clear_for(root: &Path, rel: &str) {
    for (at, _) in rel.match_indices('/') {
        let dir = root.join(&rel[..at]);
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| !m.is_dir()) {
            remove(&dir);
        }
    }
    remove(&root.join(rel));
}

fn write(root: &Path, rel: &str, text: &str) {
    clear_for(root, rel);
    let full = root.join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, text).unwrap();
}

/// 추적 중인 파일(`.gitmodules` 는 빼다 — 망가뜨리면 미리보기가 설정을 읽지 못해 오류다).
fn tracked(repo: &TestRepo) -> Vec<String> {
    repo.git(&["ls-files"])
        .lines()
        .filter(|p| *p != ".gitmodules")
        .map(str::to_owned)
        .collect()
}

/// 작업 트리 · index 를 한 번 바꾼다. 무엇을 했는지 돌려준다(git 이 거절한 것도 그대로 적는다).
fn step(rng: &mut Rng, repo: &TestRepo, n: usize, submodule: bool) -> String {
    let root = repo.root();
    let text = format!("human {n}\n");
    let files = tracked(repo);
    let some_tracked = |rng: &mut Rng| {
        if files.is_empty() {
            random_path(rng)
        } else {
            rng.pick(&files).clone()
        }
    };
    let try_git = |args: &[&str]| {
        let out = repo.try_git_in(root, args);
        if out.status.success() {
            ""
        } else {
            " (refused)"
        }
    };
    match rng.below(17) {
        0 => {
            let p = random_path(rng);
            write(root, &p, &text);
            format!("write {p}")
        }
        1 => {
            let p = some_tracked(rng);
            write(root, &p, &text);
            format!("edit {p}")
        }
        2 => {
            let p = random_path(rng);
            remove(&root.join(&p));
            format!("rm -rf {p}")
        }
        3 => {
            let p = some_tracked(rng);
            let full = root.join(&p);
            if let Ok(meta) = std::fs::symlink_metadata(&full)
                && meta.is_file()
            {
                let mode = meta.permissions().mode() ^ 0o111;
                std::fs::set_permissions(&full, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            format!("chmod +-x {p}")
        }
        4 => {
            let p = random_path(rng);
            clear_for(root, &p);
            let full = root.join(&p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            let _ = std::os::unix::fs::symlink(format!("link {n}"), &full);
            format!("ln -s {p}")
        }
        5 => {
            let p = random_path(rng);
            let r = try_git(&["add", "-f", "--", &p]);
            format!("git add {p}{r}")
        }
        6 => {
            let p = random_path(rng);
            let r = try_git(&["add", "-N", "--", &p]);
            format!("git add -N {p}{r}")
        }
        7 => {
            let p = some_tracked(rng);
            let r = try_git(&["rm", "-q", "-r", "--cached", "--", &p]);
            format!("git rm --cached {p}{r}")
        }
        8 | 9 => {
            let flag = if rng.one_in(2) {
                "--assume-unchanged"
            } else {
                "--skip-worktree"
            };
            let p = some_tracked(rng);
            let r = try_git(&["update-index", flag, "--", &p]);
            if rng.one_in(3) {
                format!("update-index {flag} {p}{r}")
            } else {
                write(root, &p, &text);
                format!("update-index {flag} {p}{r}, then edit it")
            }
        }
        10 => {
            let p = random_path(rng);
            clear_for(root, &p);
            let dir = root.join(&p);
            std::fs::create_dir_all(&dir).unwrap();
            repo.git_in(&dir, &["init", "-q"]);
            std::fs::write(dir.join("f"), "nested\n").unwrap();
            format!("nested repository {p}")
        }
        11 => {
            let dir = *rng.pick(&PARTS);
            let r = if rng.one_in(2) {
                try_git(&["sparse-checkout", "set", "--cone", dir])
            } else {
                try_git(&[
                    "sparse-checkout",
                    "set",
                    "--no-cone",
                    "/*",
                    &format!("!/{dir}"),
                    &format!("!/{dir}/"),
                ])
            };
            format!("sparse-checkout around {dir}{r}")
        }
        12 => {
            let from = some_tracked(rng);
            let to = random_path(rng);
            let r = try_git(&["mv", "-k", "--", &from, &to]);
            format!("git mv {from} {to}{r}")
        }
        13 => {
            let p = random_path(rng)
                .split('/')
                .map(|part| {
                    if rng.one_in(2) {
                        part.to_ascii_uppercase()
                    } else {
                        part.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            write(root, &p, &text);
            format!("write (case) {p}")
        }
        14 => {
            let p = format!("{}/z.log", *rng.pick(&PARTS));
            write(root, &p, &text);
            format!("write (ignored) {p}")
        }
        15 if submodule && root.join("s").is_dir() => {
            let p = if rng.one_in(2) {
                "s/README.md"
            } else {
                "s/new"
            };
            std::fs::write(root.join(p), "SUB EDIT\n").unwrap();
            format!("edit the submodule {p}")
        }
        _ => {
            let from = some_tracked(rng);
            let to = random_path(rng);
            if root.join(&from).exists() && !to.starts_with(&format!("{from}/")) {
                clear_for(root, &to);
                let full = root.join(&to);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                let _ = std::fs::rename(root.join(&from), full);
            }
            format!("mv {from} {to}")
        }
    }
}
