//! blame · 파일 히스토리 픽스처(`docs/design/13-m2-plan.md` M2-05).
//!
//! - [`TestRepo::blame_fixture`]: 이름 바뀜 2번 · 줄 이동 · 머지가 있는 작은 저장소(git CLI 로 만든다). 완료 기준의 픽스처다.
//! - [`random`]: 시드로 정해지는 무작위 이력(`git fast-import`). 대상 파일 하나를 고치고 · 줄 덩어리를 옮기고 · 이름을 바꾸고 ·
//!   지웠다가 다른 파일의 복사로 다시 만들고, 갈래를 만들어 합친다. 커밋 시각이 같거나 부모보다 이른 커밋이 섞인다(git log 순서의
//!   동률 · 시계 어긋남). 흔한 줄(`}` · 빈 줄)이 섞여 diff 정렬이 하나로 정해지지 않는 경우도 생긴다.
//! - [`load`]: 줄 수 · 커밋 수를 정한 파일 하나의 곧은 이력(측정 — 5,000줄 · 1천 커밋).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::TestRepo;
use super::synth::{Rng, finish, import};

/// [`TestRepo::blame_fixture`] 가 만든 커밋.
#[derive(Debug, Clone)]
pub struct BlameFixture {
    /// 마지막 경로.
    pub path: &'static str,
    /// 경로가 `a.txt` → `b.txt` → `dir/c.txt` 로 바뀌었다.
    pub paths: [&'static str; 3],
    /// 블록(원래 10~14행)을 파일 끝으로 옮긴 커밋.
    pub moved: String,
    /// `side` 를 합친 머지.
    pub merge: String,
}

impl TestRepo {
    /// 30줄 `a.txt` 를 만들고: 5행 고침 → `b.txt` 로 이름 바꿈 → 10~14행을 끝으로 옮김 → `side` 에서 20행, `main` 에서 2행 고침 →
    /// 머지 → `dir/c.txt` 로 이름 바꿈 → 한 줄 덧붙임.
    pub fn blame_fixture(&self) -> BlameFixture {
        let mut lines: Vec<String> = (1..=30)
            .map(|i| format!("line {i} of the original file with some words"))
            .collect();
        let text = |lines: &[String]| lines.iter().map(|l| format!("{l}\n")).collect::<String>();
        self.commit_file("a.txt", &text(&lines), "add a");
        lines[4] = "LINE 5 of the original file with some words".into();
        self.commit_file("a.txt", &text(&lines), "edit 5");
        self.git(&["mv", "a.txt", "b.txt"]);
        self.commit_all("rename a to b");
        let block: Vec<String> = lines.drain(9..14).collect();
        lines.extend(block);
        let moved = self.commit_file("b.txt", &text(&lines), "move block");
        self.branch("side");
        self.checkout("side");
        let mut side = lines.clone();
        side[14] = "side 20 of the original file with some words".into();
        self.commit_file("b.txt", &text(&side), "side edit 20");
        self.checkout("main");
        lines[1] = "main 2 of the original file with some words".into();
        self.commit_file("b.txt", &text(&lines), "main edit 2");
        self.git(&["merge", "--quiet", "--no-edit", "side"]);
        let merge = self.head();
        std::fs::create_dir_all(self.root().join("dir")).unwrap();
        self.git(&["mv", "b.txt", "dir/c.txt"]);
        self.commit_all("rename b to dir/c");
        let mut tail = std::fs::read_to_string(self.root().join("dir/c.txt")).unwrap();
        tail.push_str("tail added\n");
        self.commit_file("dir/c.txt", &tail, "append");
        BlameFixture {
            path: "dir/c.txt",
            paths: ["a.txt", "b.txt", "dir/c.txt"],
            moved,
            merge,
        }
    }
}

/// [`random`] 이 만든 저장소의 개요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Random {
    /// `main` 끝에서 대상 파일의 경로(지워졌으면 마지막 이름).
    pub target: String,
    /// 대상 파일이 가졌던 모든 이름(처음 이름이 앞).
    pub names: Vec<String>,
    pub commits: usize,
    pub merges: usize,
}

const AUTHORS: [(&str, &str); 4] = [
    ("Ada Lovelace", "ada@pitwall.invalid"),
    ("Grace Hopper", "grace@pitwall.invalid"),
    ("Ken Thompson", "ken@pitwall.invalid"),
    ("Barbara Liskov", "barbara@pitwall.invalid"),
];

const WORDS: [&str; 16] = [
    "parser", "lexer", "cache", "graph", "render", "index", "socket", "buffer", "config", "theme",
    "search", "watch", "token", "frame", "queue", "stream",
];

/// 대상 파일이 옮겨 갈 이름. 따옴표로 감싸 보이는 이름(공백 · 한글)이 하나 있다.
const NAMES: [&str; 6] = [
    "src/lib/core.txt",
    "src/main.txt",
    "docs/메모 notes.txt",
    "pkg/a/b/deep.txt",
    "top.txt",
    "src/renamed.txt",
];

/// 시드로 정한 줄 하나. 대개 다른 줄과 겹치지 않고, 가끔 흔한 줄이다.
fn line(rng: &mut Rng, n: &mut usize) -> String {
    match rng.below(10) {
        0 => "}".to_owned(),
        1 => String::new(),
        _ => {
            *n += 1;
            let mut s = String::new();
            for _ in 0..5 {
                s.push_str(WORDS[rng.below(WORDS.len())]);
                s.push(' ');
            }
            format!("{s}#{n}")
        }
    }
}

#[derive(Clone)]
struct Branch {
    name: String,
    mark: usize,
    files: BTreeMap<String, Vec<String>>,
    target: Option<String>,
    /// 지운 대상 파일의 이름(다시 만들 때 쓴다).
    last: Option<String>,
    time: i64,
}

fn data(out: &mut String, text: &str) {
    let _ = write!(out, "data {}\n{text}\n", text.len());
}

fn commit(
    out: &mut String,
    branch: &Branch,
    mark: usize,
    merge: Option<usize>,
    message: &str,
    author: usize,
) {
    let (name, email) = AUTHORS[author % AUTHORS.len()];
    let _ = writeln!(out, "commit refs/heads/{}", branch.name);
    let _ = writeln!(out, "mark :{mark}");
    let _ = writeln!(out, "author {name} <{email}> {} +0000", branch.time);
    let _ = writeln!(out, "committer {name} <{email}> {} +0000", branch.time);
    data(out, message);
    if branch.mark > 0 {
        let _ = writeln!(out, "from :{}", branch.mark);
    }
    if let Some(m) = merge {
        let _ = writeln!(out, "merge :{m}");
    }
    out.push_str("deleteall\n");
    for (path, lines) in &branch.files {
        let _ = writeln!(out, "M 100644 inline {path}");
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        data(out, &text);
    }
    out.push('\n');
}

/// `dir` 에 무작위 이력(머리 주석)을 `commits` 개 커밋으로 만든다. 같은 시드면 같은 저장소다.
pub fn random(dir: &Path, ceiling: &Path, commits: usize, seed: u64) -> std::io::Result<Random> {
    let mut rng = Rng(seed);
    let mut n = 0usize;
    let mut out = String::new();
    let start: &str = "src/a.txt";
    let mut main = Branch {
        name: "main".into(),
        mark: 0,
        files: BTreeMap::new(),
        target: Some(start.to_owned()),
        last: None,
        time: 1_700_000_000,
    };
    main.files.insert(
        start.into(),
        (0..40).map(|_| line(&mut rng, &mut n)).collect(),
    );
    main.files.insert(
        "src/other.txt".into(),
        (0..30).map(|_| line(&mut rng, &mut n)).collect(),
    );
    main.files.insert(
        "docs/notes.txt".into(),
        (0..20).map(|_| line(&mut rng, &mut n)).collect(),
    );
    let mut names = vec![start.to_owned()];
    let mut mark = 1;
    commit(&mut out, &main, mark, None, "initial", 0);
    main.mark = mark;
    let mut sides: Vec<Branch> = Vec::new();
    let mut summary = Random {
        target: start.into(),
        names: Vec::new(),
        commits: 1,
        merges: 0,
    };
    let mut made = 0usize;
    while summary.commits < commits {
        let roll = rng.below(100);
        if sides.len() < 3 && roll < 8 {
            made += 1;
            let mut side = main.clone();
            side.name = format!("b{made}");
            sides.push(side);
            continue;
        }
        if !sides.is_empty() && roll < 18 {
            let side = sides.swap_remove(rng.below(sides.len()));
            let mut merged = main.files.clone();
            for (path, lines) in &side.files {
                if !merged.contains_key(path) || rng.below(2) == 0 {
                    merged.insert(path.clone(), lines.clone());
                }
            }
            if main.target.as_ref().is_none_or(|t| !merged.contains_key(t)) {
                main.target = side.target.clone().filter(|t| merged.contains_key(t));
            }
            main.files = merged;
            main.time = main.time.max(side.time) + rng.below(90) as i64;
            mark += 1;
            commit(
                &mut out,
                &main,
                mark,
                Some(side.mark),
                &format!("merge {}", side.name),
                rng.below(4),
            );
            main.mark = mark;
            summary.commits += 1;
            summary.merges += 1;
            continue;
        }
        let pick = rng.below(sides.len() + 2);
        let branch = if pick < sides.len() {
            &mut sides[pick]
        } else {
            &mut main
        };
        // 시계: 대개 늘고, 가끔 그대로(동률) · 거꾸로(어긋남).
        branch.time += rng.below(160) as i64 - 40;
        let op = rng.below(100);
        let message = match (&branch.target, op) {
            (Some(t), 0..=44) => {
                let lines = branch.files.get_mut(t).expect("target exists");
                for _ in 0..=rng.below(3) {
                    let at = rng.below(lines.len() + 1);
                    match rng.below(3) {
                        0 if at < lines.len() => lines[at] = line(&mut rng, &mut n),
                        1 if at < lines.len() && lines.len() > 8 => {
                            lines.remove(at);
                        }
                        _ => lines.insert(at, line(&mut rng, &mut n)),
                    }
                }
                "edit target"
            }
            (Some(t), 45..=56) => {
                let lines = branch.files.get_mut(t).expect("target exists");
                let len = 2 + rng.below(5);
                if lines.len() > len + 2 {
                    let from = rng.below(lines.len() - len);
                    let block: Vec<String> = lines.drain(from..from + len).collect();
                    let to = rng.below(lines.len() + 1);
                    for (k, l) in block.into_iter().enumerate() {
                        lines.insert(to + k, l);
                    }
                }
                "move block"
            }
            (Some(t), 57..=64) => {
                let to = NAMES[rng.below(NAMES.len())];
                if to != t && !branch.files.contains_key(to) {
                    let mut lines = branch.files.remove(&t.clone()).expect("target exists");
                    if rng.below(3) == 0 && !lines.is_empty() {
                        let at = rng.below(lines.len());
                        lines[at] = line(&mut rng, &mut n);
                    }
                    branch.files.insert(to.to_owned(), lines);
                    branch.target = Some(to.to_owned());
                    if !names.iter().any(|x| x == to) {
                        names.push(to.to_owned());
                    }
                }
                "rename target"
            }
            (Some(t), 65..=69) => {
                let t = t.clone();
                branch.files.remove(&t);
                // 다시 만들 때 쓸 이름으로 남긴다.
                branch.target = None;
                branch.last = Some(t);
                "delete target"
            }
            (Some(t), 70..=74) => {
                let copy = format!("copies/c{}.txt", rng.below(1000));
                let lines = branch.files[t].clone();
                branch.files.insert(copy, lines);
                "copy target"
            }
            (None, 0..=49) => {
                let name = branch.last.take().unwrap_or_else(|| start.to_owned());
                let lines = if rng.below(2) == 0 {
                    let mut l = branch.files["src/other.txt"].clone();
                    l.push(line(&mut rng, &mut n));
                    l
                } else {
                    (0..25).map(|_| line(&mut rng, &mut n)).collect()
                };
                branch.files.insert(name.clone(), lines);
                branch.target = Some(name);
                "recreate target"
            }
            _ => {
                let other = if rng.below(2) == 0 {
                    "src/other.txt"
                } else {
                    "docs/notes.txt"
                };
                let lines = branch.files.entry(other.to_owned()).or_default();
                let at = rng.below(lines.len() + 1);
                lines.insert(at, line(&mut rng, &mut n));
                "edit other"
            }
        };
        mark += 1;
        let author = rng.below(4);
        let b = if pick < sides.len() {
            &mut sides[pick]
        } else {
            &mut main
        };
        commit(&mut out, b, mark, None, message, author);
        b.mark = mark;
        summary.commits += 1;
    }
    out.push_str("done\n");
    import(dir, ceiling, out.as_bytes())?;
    finish(dir, ceiling)?;
    summary.target = main
        .target
        .clone()
        .or_else(|| main.last.clone())
        .unwrap_or_else(|| start.to_owned());
    summary.names = names;
    Ok(summary)
}

/// `dir` 에 `lines` 줄 파일(`src/big.txt` → `src/bigger.txt` → `lib/big.txt`) 하나의 곧은 이력을 `commits` 개 커밋으로 만든다.
/// 커밋마다 줄 1~8개를 바꾸거나 넣거나 빼고(줄 수는 거의 그대로), 50 커밋마다 20줄을 옮기고, 3분의 1 · 3분의 2 지점에서 이름을
/// 바꾼다.
pub fn load(
    dir: &Path,
    ceiling: &Path,
    lines: usize,
    commits: usize,
    seed: u64,
) -> std::io::Result<()> {
    let mut rng = Rng(seed);
    let mut n = 0usize;
    let mut out = String::new();
    let mut main = Branch {
        name: "main".into(),
        mark: 0,
        files: BTreeMap::new(),
        target: None,
        last: None,
        time: 1_700_000_000,
    };
    let mut path = "src/big.txt".to_owned();
    main.files.insert(
        path.clone(),
        (0..lines).map(|_| line(&mut rng, &mut n)).collect(),
    );
    for k in 1..=commits {
        if k > 1 {
            let file = main.files.get_mut(&path).expect("the file");
            for _ in 0..=rng.below(8) {
                let at = rng.below(file.len());
                match rng.below(3) {
                    0 => file[at] = line(&mut rng, &mut n),
                    1 if file.len() > lines / 2 => {
                        file.remove(at);
                    }
                    _ => file.insert(at, line(&mut rng, &mut n)),
                }
            }
            if k % 50 == 0 && file.len() > 40 {
                let from = rng.below(file.len() - 20);
                let block: Vec<String> = file.drain(from..from + 20).collect();
                let to = rng.below(file.len());
                for (i, l) in block.into_iter().enumerate() {
                    file.insert(to + i, l);
                }
            }
            let rename = if k == commits / 3 {
                Some("src/bigger.txt")
            } else if k == commits * 2 / 3 {
                Some("lib/big.txt")
            } else {
                None
            };
            if let Some(to) = rename {
                let moved = main.files.remove(&path).expect("the file");
                path = to.to_owned();
                main.files.insert(path.clone(), moved);
            }
        }
        main.time += 60;
        commit(&mut out, &main, k, None, &format!("change {k}"), k);
        main.mark = k;
    }
    out.push_str("done\n");
    import(dir, ceiling, out.as_bytes())?;
    finish(dir, ceiling)
}
