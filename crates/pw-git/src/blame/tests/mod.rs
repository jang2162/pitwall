//! blame 시험. 저장소는 임시 폴더에 진짜 git 으로 만들고(`crate::testing::blame`), 결과는 같은 리비전의 `git blame
//! --line-porcelain`(엔진 `git` 은 `-M` 과, 엔진 `gix` 는 `-M` 없이)과 줄마다 맞춘다.

mod compare;
mod parse;
pub(crate) mod rpc;
#[cfg(unix)]
mod untrusted;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::testing::repo::git_command;

/// 줄 하나의 blame: 커밋 · 원래 줄 · 그 커밋에서의 경로 · 「이 커밋 이전으로」(부모, 경로).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub commit: String,
    pub source_line: u32,
    pub path: String,
    pub previous: Option<(String, String)>,
}

/// 커밋 정보(작성자 · 메일 · 시각 · 제목 · 경계).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Meta {
    pub author: String,
    pub email: String,
    pub time: i64,
    pub subject: String,
    pub boundary: bool,
}

/// `git blame --line-porcelain [-M] <rev> -- <path>`. 경로를 따옴표 없이 받는다(`core.quotePath=false`) — 엔진 `git` 의 따옴표
/// 풀기([`incremental::unquote`])를 이 쪽이 쓰지 않게.
pub(crate) fn git_blame(
    dir: &Path,
    ceiling: &Path,
    rev: &str,
    path: &str,
    moves: bool,
) -> (Vec<Line>, HashMap<String, Meta>) {
    let mut c = git_command(dir, ceiling);
    c.args(["-c", "core.quotePath=false", "blame", "--line-porcelain"]);
    if moves {
        c.arg("-M");
    }
    let out = c.args([rev, "--", path]).output().unwrap();
    assert!(
        out.status.success(),
        "git blame {rev} {path}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines = Vec::new();
    let mut metas = HashMap::new();
    let mut cur: Option<(String, u32)> = None;
    let mut meta = None::<Meta>;
    let mut previous = None;
    let mut path = String::new();
    for l in text.lines() {
        if let Some(_content) = l.strip_prefix('\t') {
            let (commit, source_line) = cur.take().expect("a header before the content");
            let m = meta.take().expect("line-porcelain repeats the metadata");
            metas.insert(commit.clone(), m);
            lines.push(Line {
                commit,
                source_line,
                path: std::mem::take(&mut path),
                previous: previous.take(),
            });
            continue;
        }
        let (key, value) = l.split_once(' ').unwrap_or((l, ""));
        if cur.is_none() {
            let mut parts = l.split(' ');
            let commit = parts.next().unwrap().to_owned();
            let source: u32 = parts.next().unwrap().parse().unwrap();
            cur = Some((commit, source));
            meta = Some(Meta {
                author: String::new(),
                email: String::new(),
                time: 0,
                subject: String::new(),
                boundary: false,
            });
            continue;
        }
        let m = meta.as_mut().unwrap();
        match key {
            "author" => m.author = value.to_owned(),
            "author-mail" => m.email = value.trim_matches(['<', '>']).to_owned(),
            "author-time" => m.time = value.parse().unwrap(),
            "summary" => m.subject = value.to_owned(),
            "boundary" => m.boundary = true,
            "previous" => {
                let (c, p) = value.split_once(' ').unwrap();
                previous = Some((c.to_owned(), p.to_owned()));
            }
            "filename" => path = value.to_owned(),
            _ => {}
        }
    }
    (lines, metas)
}

/// 청크를 줄로 펼친다. 범위가 1 ~ `lines` 를 겹치지 않고 한 번씩 덮는지, 커밋이 범위보다 먼저(같은 청크나 앞 청크) 한 번만
/// 실렸는지, 첫 청크에만 `file` 이 있는지 본다.
pub(crate) fn expand(chunks: &[BlameChunk]) -> (BlameFile, Vec<Line>, HashMap<String, Meta>) {
    let file = chunks[0]
        .file
        .clone()
        .expect("the first chunk has the file");
    assert!(chunks[1..].iter().all(|c| c.file.is_none()));
    let mut metas = HashMap::new();
    let mut slots: Vec<Option<Line>> = vec![None; file.lines as usize];
    for chunk in chunks {
        for c in &chunk.commits {
            let fresh = metas.insert(
                c.hash.clone(),
                Meta {
                    author: c.author_name.clone(),
                    email: c.author_email.clone(),
                    time: c.author_time,
                    subject: c.subject.clone(),
                    boundary: c.boundary,
                },
            );
            assert!(fresh.is_none(), "commit {} sent twice", c.hash);
        }
        for r in &chunk.ranges {
            assert!(metas.contains_key(&r.commit), "range before its commit");
            assert!(r.count >= 1);
            for k in 0..r.count {
                let slot = &mut slots[(r.line + k - 1) as usize];
                assert!(slot.is_none(), "line {} covered twice", r.line + k);
                *slot = Some(Line {
                    commit: r.commit.clone(),
                    source_line: r.source_line + k,
                    path: r.source_path.clone(),
                    previous: r
                        .previous
                        .as_ref()
                        .map(|p| (p.commit.clone(), p.path.clone())),
                });
            }
        }
    }
    let lines = slots
        .into_iter()
        .enumerate()
        .map(|(i, l)| l.unwrap_or_else(|| panic!("line {} not covered", i + 1)))
        .collect();
    (file, lines, metas)
}

/// 미신뢰로 연다(RPC 와 같은 열기 경로).
pub(crate) fn open(dir: &Path) -> gix::Repository {
    crate::repo::discover(dir, false)
        .expect("open the repository")
        .expect("a repository")
}

/// 엔진 `gix` 로 blame 한다.
pub(crate) fn blame_gix(dir: &Path, rev: &str, path: &str) -> Vec<BlameChunk> {
    let repo = open(dir);
    let subject = subject(&repo, Some(rev), path).unwrap().expect("the file");
    let mut chunks = Vec::new();
    let done = native::run(&repo, false, &subject, &AtomicBool::new(false), &mut |c| {
        chunks.push(c);
        true
    })
    .unwrap();
    assert!(done);
    chunks
}

/// 청크를 모은다.
pub(crate) struct Collect(pub Vec<BlameChunk>);

impl incremental::Emit for Collect {
    async fn emit(&mut self, chunk: BlameChunk) -> bool {
        self.0.push(chunk);
        true
    }
}

/// 엔진 `git` 으로 blame 한다(격리된 git 으로 RPC 와 같은 인자 — [`incremental::command`] 와 같다).
pub(crate) fn blame_cli(dir: &Path, ceiling: &Path, rev: &str, path: &str) -> Vec<BlameChunk> {
    let repo = open(dir);
    let subject = subject(&repo, Some(rev), path).unwrap().expect("the file");
    let mut command = git_command(dir, ceiling);
    command
        .args(incremental::ARGS)
        .args([&subject.commit.to_string(), "--", path]);
    let file = BlameFile {
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
            let mut out = Collect(Vec::new());
            let drained = incremental::pump(stdout, file, &mut out)
                .await
                .unwrap()
                .expect("not abandoned");
            assert!(child.wait().await.unwrap().success());
            out.0.extend(drained.finish().unwrap());
            out.0
        })
}
