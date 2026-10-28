//! 에디터 거터 [스테이징](13-m2-plan M2-17 — 리뷰 F1)의 차등 시험. 세 단계가 fixture 둘로 이어진다(`apps/desktop/src/git/gutter/
//! fixtures/`):
//!
//! 1. [`gutter_stage_cases_match_pw_git`]: 무작위 저장소 상태(HEAD · index · 작업 트리, 시드 고정)마다 진짜 `git.diff`(index → 작업
//!    트리)를 받아 `stage-cases.json` 과 견준다(다시 만들 때 `PW_UPDATE_GUTTER_STAGE_CASES=1`).
//! 2. 창 시험(`apps/desktop/src/git/gutter/stage-plan.test.ts`)이 그 상태의 거터 hunk 마다 `planStage` 로 계획(고를 줄 · 목표 index)을
//!    세워 `stage-plans.json` 과 견준다(다시 만들 때 `PW_UPDATE_GUTTER_STAGE_PLANS=1`).
//! 3. [`gutter_stage_plans_stage_exactly_the_hunk`]: 그 계획을 진짜 `git.stageLines` 로 돌려 index(`git show :<경로>`)가 목표와
//!    바이트로 같은지, index 가 HEAD 와 같은 상태면 「HEAD 에 그 거터 hunk 만 얹은 것」(여기서 따로 계산한다)과도 같은지 본다.
//!    「이미 스테이징됨」 은 index 가 그것과 이미 같은지 본다. 고르지 않은 것(`inexact`)의 수는 출력에 남긴다.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::bytes;
use crate::diff::methods::{DiffSide, GitDiffParams};
use crate::stage::methods::{GitLinesParams, HunkSelection};
use crate::testing::write::WriteFixture;

const CASES: &str = "../../apps/desktop/src/git/gutter/fixtures/stage-cases.json";
const PLANS: &str = "../../apps/desktop/src/git/gutter/fixtures/stage-plans.json";
const PATH: &str = "g.txt";
const COUNT: usize = 240;
const SEED: u64 = 0x6075_7465_7232_0017;

/// 줄 글자: 빈 줄 · 괄호 · 같은 줄이 많아 두 diff 의 정렬이 갈리기 쉽다.
const WORDS: &[&str] = &["", "", "}", "{", "a", "b", "fn x() {", "    y", "*"];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn words(rng: &mut Rng, n: usize) -> Vec<String> {
    (0..n)
        .map(|_| WORDS[rng.below(WORDS.len())].to_owned())
        .collect()
}

/// 고치기 1~`most` 번: 줄 몇 개 넣기 · 지우기 · 바꾸기.
fn edit(rng: &mut Rng, lines: &[String], most: usize) -> Vec<String> {
    let mut out = lines.to_vec();
    for _ in 0..1 + rng.below(most) {
        let at = rng.below(out.len() + 1);
        let n = 1 + rng.below(3);
        match rng.below(3) {
            0 => {
                let new = words(rng, n);
                out.splice(at..at, new);
            }
            1 => {
                let end = (at + n).min(out.len());
                out.drain(at..end);
            }
            _ => {
                let end = (at + n).min(out.len());
                let new = words(rng, n);
                out.splice(at..end, new);
            }
        }
    }
    out
}

fn text(lines: &[String], eol: bool) -> String {
    let mut s = lines.join("\n");
    if eol && !lines.is_empty() {
        s.push('\n');
    }
    s
}

/// 상태 하나: (HEAD, index, 작업 트리) 의 내용.
fn cases() -> Vec<(String, String, String)> {
    let mut rng = Rng(SEED);
    (0..COUNT)
        .map(|_| {
            let n = rng.below(14);
            let head = words(&mut rng, n);
            let head_eol = !rng.chance(15);
            let (index, index_eol) = if rng.chance(30) {
                (edit(&mut rng, &head, 2), !rng.chance(10))
            } else {
                (head.clone(), head_eol)
            };
            let work = edit(&mut rng, &index, 5);
            let work_eol = if rng.chance(15) {
                !index_eol
            } else {
                index_eol
            };
            (
                text(&head, head_eol),
                text(&index, index_eol),
                text(&work, work_eol),
            )
        })
        .collect()
}

fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// index 를 `index`, 작업 트리를 `work` 로 둔다(HEAD 는 diff · stageLines 가 보지 않는다).
fn set_state(f: &WriteFixture, index: &str, work: &str) {
    f.repo.write(PATH, index);
    f.repo.git(&["add", PATH]);
    f.repo.write(PATH, work);
}

fn work_tree_diff(f: &WriteFixture) -> Value {
    let files = f
        .write
        .git()
        .diff(&GitDiffParams {
            project_id: f.project.clone(),
            worktree: None,
            from: None,
            to: DiffSide::WorkTree,
            paths: Some(vec![PATH.to_owned()]),
            ignore_whitespace: false,
        })
        .unwrap()
        .files;
    files
        .into_iter()
        .next()
        .map_or(Value::Null, |file| serde_json::to_value(file).unwrap())
}

fn write_or_compare(rel: &str, env: &str, got: &Value) {
    let path = fixture_path(rel);
    if std::env::var_os(env).is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{}\n", serde_json::to_string(got).unwrap())).unwrap();
        return;
    }
    let want: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    assert!(
        *got == want,
        "{} 이 지금 pw-git 의 출력과 다르다 — {env}=1 로 이 시험을 돌려 다시 만든다",
        path.display()
    );
}

#[test]
fn gutter_stage_cases_match_pw_git() {
    let f = WriteFixture::new(true);
    let got: Vec<Value> = cases()
        .into_iter()
        .map(|(head, index, work)| {
            set_state(&f, &index, &work);
            json!({ "head": head, "index": index, "work": work, "diff": work_tree_diff(&f) })
        })
        .collect();
    write_or_compare(CASES, "PW_UPDATE_GUTTER_STAGE_CASES", &Value::Array(got));
}

/// git 처럼 센 줄과 끝 개행.
fn split(text: &str) -> (Vec<&str>, bool) {
    if text.is_empty() {
        return (Vec::new(), true);
    }
    let eol = text.ends_with('\n');
    let body = if eol { &text[..text.len() - 1] } else { text };
    (body.split('\n').collect(), eol)
}

/// HEAD 에 거터 hunk 하나(HEAD 줄 `[old_from, old_to)` → 작업 트리 줄 `[new_from, new_to)`)만 얹은 내용.
fn head_with_hunk(head: &str, work: &str, hunk: &Value) -> String {
    let n = |k: &str| hunk[k].as_u64().unwrap() as usize;
    let (h, h_eol) = split(head);
    let (w, w_eol) = split(work);
    let (old_from, old_to, new_from, new_to) = (n("oldFrom"), n("oldTo"), n("newFrom"), n("newTo"));
    let mut lines: Vec<&str> = Vec::new();
    lines.extend_from_slice(&h[..old_from - 1]);
    lines.extend_from_slice(&w[new_from - 1..new_to - 1]);
    lines.extend_from_slice(&h[old_to - 1..]);
    // 끝 개행은 마지막 줄이 온 쪽의 것: 뒤 HEAD 줄이 남으면 HEAD 의 끝, 아니면 hunk 의 작업 트리 줄이 마지막이면 그 줄이 작업 트리 끝일
    // 때 작업 트리의 끝(아니면 뒤에 줄이 있었으므로 개행이 있다), 앞 HEAD 줄이 마지막이면 그 뒤에 줄(지운 줄)이 있었으므로 개행이 있다.
    let eol = if old_to - 1 < h.len() {
        h_eol
    } else if new_to > new_from {
        new_to - 1 < w.len() || w_eol
    } else {
        true
    };
    text(
        &lines.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        eol,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gutter_stage_plans_stage_exactly_the_hunk() {
    let cases: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture_path(CASES)).unwrap()).unwrap();
    let plans: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture_path(PLANS)).unwrap()).unwrap();
    let f = WriteFixture::new(true);
    let (mut ready, mut already, mut inexact) = (0, 0, 0);
    for plan in plans.as_array().unwrap() {
        let case = &cases[plan["case"].as_u64().unwrap() as usize];
        let head = case["head"].as_str().unwrap();
        let index = case["index"].as_str().unwrap();
        let work = case["work"].as_str().unwrap();
        let independent = (index == head).then(|| head_with_hunk(head, work, &plan["hunk"]));
        match plan["kind"].as_str().unwrap() {
            "ready" => {
                ready += 1;
                let target = plan["target"].as_str().unwrap();
                set_state(&f, index, work);
                // 고른 hunk 는 그 상태의 diff 안 번호다.
                let hunks: Vec<HunkSelection> = plan["hunks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|sel| HunkSelection {
                        hunk: serde_json::from_value(
                            case["diff"]["hunks"][sel["hunk"].as_u64().unwrap() as usize].clone(),
                        )
                        .unwrap(),
                        lines: serde_json::from_value(sel["lines"].clone()).unwrap(),
                    })
                    .collect();
                f.write
                    .stage_lines(
                        f.caller(),
                        GitLinesParams {
                            project_id: f.project.clone(),
                            worktree: None,
                            path: PATH.to_owned(),
                            old_path: None,
                            hunks,
                            ignore_whitespace: false,
                        },
                    )
                    .await
                    .unwrap_or_else(|e| panic!("{plan}: {e:?}"));
                let staged = String::from_utf8(bytes(&f, &["show", &format!(":{PATH}")])).unwrap();
                assert_eq!(staged, target, "{plan}\n{case}");
                if let Some(expected) = &independent {
                    assert_eq!(&staged, expected, "{plan}\n{case}");
                }
            }
            "already" => {
                already += 1;
                if let Some(expected) = &independent {
                    assert_eq!(index, expected, "{plan}\n{case}");
                }
            }
            "inexact" => inexact += 1,
            other => panic!("unknown plan kind {other}"),
        }
    }
    let total = ready + already + inexact;
    eprintln!("[gutter stage] {total} hunks: ready {ready}, already {already}, inexact {inexact}");
    assert!(total >= COUNT, "{total} hunks");
    assert!(
        ready * 10 >= total * 7,
        "too many refused: {inexact} of {total}"
    );
}
