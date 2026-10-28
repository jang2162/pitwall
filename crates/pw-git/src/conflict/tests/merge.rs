//! 구간 계산이 `git merge-file -p --diff3` 와 같다: 정한 픽스처(겹침 · 닿음 · 같은 변경 · 지우기 대 고치기 · 같은 자리 더하기 ·
//! 빈 조상 · 끝 개행 없음 · CRLF · 여러 충돌)와 시드를 고정한 무작위 세 쪽 수백 개. git 의 출력을 표시(`<<<<<<<` …)로 나눈 구간과
//! 이쪽 결과(충돌 사이의 결과 줄 · 충돌의 세 쪽 줄)를 견준다.

use std::path::Path;

use super::super::merge::{Kind, merge};
use super::{merge_file, sections};
use crate::diff::hunks::lines;

/// 세 쪽을 이쪽과 git 으로 병합해 구간이 같은지 본다. 이쪽의 충돌 수를 돌려준다.
fn same_as_git(dir: &Path, base: &str, ours: &str, theirs: &str) -> usize {
    let (b, o, t) = (base.as_bytes(), ours.as_bytes(), theirs.as_bytes());
    let m = merge(b, o, t);
    let regions: Vec<_> = m
        .regions
        .iter()
        .map(|r| {
            let ranges = [
                r.base.clone(),
                r.ours.clone(),
                r.theirs.clone(),
                r.merged.clone(),
            ];
            (r.kind, ranges)
        })
        .collect();
    let ours_side = sections(&m.text(), &regions, b, o, t);
    let git = merge_file(dir, b, o, t);
    assert_eq!(
        ours_side, git,
        "regions differ from git merge-file --diff3\nbase={base:?}\nours={ours:?}\ntheirs={theirs:?}"
    );
    // 구간 목록이 결과를 빠짐없이 덮는다: 구간 밖의 결과 줄은 ours 의 줄과 같은 자리에서 같다.
    let ours_lines = lines(o);
    let mut i = 0;
    let mut at = 0;
    for r in &m.regions {
        assert_eq!(
            m.lines[at..r.merged.start as usize],
            ours_lines[i..r.ours.start as usize]
        );
        at = r.merged.end as usize;
        i = r.ours.end as usize;
    }
    assert_eq!(m.lines[at..], ours_lines[i..]);
    m.conflicts().count()
}

#[test]
fn fixed_fixtures_have_the_regions_of_git_merge_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let base = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let cases: &[(&str, &str, &str, &str, usize)] = &[
        // 이름, ours, theirs, base(빈 글이면 위의 base), 기대 충돌 수
        (
            "apart",
            "1\n2o\n3\n4\n5\n6\n7\n8\n9\n10\n",
            "1\n2\n3\n4\n5\n6\n7\n8\n9t\n10\n",
            "",
            0,
        ),
        (
            "overlap",
            "1\n2o\n3o\n4\n5\n6\n7\n8\n9\n10\n",
            "1\n2\n3t\n4t\n5\n6\n7\n8\n9\n10\n",
            "",
            1,
        ),
        (
            "touching",
            "1\n2o\n3\n4\n5\n6\n7\n8\n9\n10\n",
            "1\n2\n3t\n4\n5\n6\n7\n8\n9\n10\n",
            "",
            1,
        ),
        (
            "one line apart",
            "1\n2o\n3\n4\n5\n6\n7\n8\n9\n10\n",
            "1\n2\n3\n4t\n5\n6\n7\n8\n9\n10\n",
            "",
            0,
        ),
        (
            "same change",
            "1\n2x\n3\n4\n5\n6\n7\n8\n9\n10\n",
            "1\n2x\n3\n4\n5\n6\n7\n8\n9\n10\n",
            "",
            0,
        ),
        (
            "delete vs modify",
            "1\n4\n5\n6\n7\n8\n9\n10\n",
            "1\n2\n3t\n4\n5\n6\n7\n8\n9\n10\n",
            "",
            1,
        ),
        (
            "insert at the same place",
            "1\n2\n3\no\n4\n5\n6\n7\n8\n9\n10\n",
            "1\n2\n3\nt\n4\n5\n6\n7\n8\n9\n10\n",
            "",
            1,
        ),
        (
            "one side unchanged",
            base,
            "1\n2\nt\n3\n4\n5\n6\n7\n8\n10\n",
            "",
            0,
        ),
        (
            "both added",
            "same\nours\nend\n",
            "same\ntheirs\nend\n",
            "\0",
            1,
        ),
        (
            "no newline at the end",
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10o",
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10t",
            "",
            1,
        ),
        (
            "crlf",
            "a\r\nb o\r\nc\r\n",
            "a\r\nb t\r\nc\r\n",
            "a\r\nb\r\nc\r\n",
            1,
        ),
        (
            "several conflicts and clean changes",
            "1\n2o\n3\n4\n5o\n6\n7\n8\n9o\n10\n",
            "1\n2t\n3\n4\n5\n6t\n7\n8\n9t\n10\nnew\n",
            "",
            // 5 · 6 줄의 변경은 닿기만 하지만 충돌이다.
            3,
        ),
        ("everything", "o\n", "t\n", "", 1),
    ];
    for (name, ours, theirs, own_base, conflicts) in cases {
        let b = match *own_base {
            "" => base,
            "\0" => "",
            other => other,
        };
        assert_eq!(same_as_git(dir, b, ours, theirs), *conflicts, "{name}");
    }
}

#[test]
fn the_merged_text_takes_the_one_sided_changes_and_leaves_conflicts_empty() {
    let base = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let m = merge(
        base,
        b"1\n2 ours\n3\n4\n5\n6\n7\n8\n9 ours\n10\n",
        b"1\n2 theirs\n3\n4\n5\n6 theirs\n7\n8\n9\n10\n",
    );
    assert_eq!(m.text(), b"1\n3\n4\n5\n6 theirs\n7\n8\n9 ours\n10\n");
    let kinds: Vec<_> = m
        .regions
        .iter()
        .map(|r| (r.kind, r.merged.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            (Kind::Conflict, 1..1),
            (Kind::Theirs, 4..5),
            (Kind::Ours, 7..8)
        ]
    );
    let c = &m.regions[0];
    assert_eq!(
        (c.base.clone(), c.ours.clone(), c.theirs.clone()),
        (1..2, 1..2, 1..2)
    );
}

/// xorshift64 — 시드가 같으면 같은 수열.
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
}

/// 몇 군데를 지우고 · 더하고 · 바꾼다. 줄 글이 겹치게(작은 글자 모음) 해서 같은 줄 · 같은 변경이 자주 생기게 한다.
fn mutate(rng: &mut Rng, base: &[String], words: &[&str]) -> Vec<String> {
    let mut out = base.to_vec();
    for _ in 0..rng.below(4) + 1 {
        let at = rng.below(out.len() + 1);
        match rng.below(3) {
            0 if at < out.len() => {
                let n = (rng.below(3) + 1).min(out.len() - at);
                out.drain(at..at + n);
            }
            1 => {
                for _ in 0..rng.below(3) + 1 {
                    out.insert(at, words[rng.below(words.len())].to_owned());
                }
            }
            _ if at < out.len() => out[at] = words[rng.below(words.len())].to_owned(),
            _ => out.push(words[rng.below(words.len())].to_owned()),
        }
    }
    out
}

fn text(rng: &mut Rng, lines: &[String]) -> String {
    let mut text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    if !text.is_empty() && rng.below(10) == 0 {
        text.pop();
    }
    text
}

#[test]
fn random_three_way_merges_have_the_regions_of_git_merge_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let common = ["a", "b", "c", "d", "e", "{", "}", ""];
    let (mut conflicts, mut clean) = (0, 0);
    for _ in 0..250 {
        let n = rng.below(24);
        let base: Vec<String> = (0..n)
            .map(|_| common[rng.below(common.len())].to_owned())
            .collect();
        let ours = mutate(&mut rng, &base, &["a", "b", "o", "x", ""]);
        let theirs = mutate(&mut rng, &base, &["a", "c", "t", "x", ""]);
        let (b, o, t) = (
            text(&mut rng, &base),
            text(&mut rng, &ours),
            text(&mut rng, &theirs),
        );
        if same_as_git(tmp.path(), &b, &o, &t) > 0 {
            conflicts += 1;
        } else {
            clean += 1;
        }
    }
    // 수열이 두 갈래를 다 만든다(시드를 바꿔도 이 비율이면 한쪽만 나오지 않는다).
    assert!(
        conflicts > 30 && clean > 30,
        "conflicts {conflicts}, clean {clean}"
    );
}

/// 머지 문서(M2-19)의 `≫` · `≪` 가 git 과 같은 바이트를 만드는지 창 시험이 견줄 무작위 충돌:
/// `apps/desktop/src/git/merge/fixtures/merge-cases.json`. 줄바꿈 세 갈래(LF · 순수 CRLF · 섞임 — CRLF 와 LF 가 섞이고 줄 안에
/// 홀로 선 `\r` 이 있다)마다 충돌이 있는 경우를 모아 세 쪽 · 자동 병합 결과 · 구간(`git.conflict.get` 과 같은 꼴)과
/// `git merge-file -p --diff3 --ours` · `--theirs` · `--union` 의 출력을 싣는다(구간이 `--diff3` 의 것이라 그 모드로 — 없으면 git 이 충돌을
/// 더 잘게 쪼갠다). 창 시험(`merge-model.test.ts`)이 모든 충돌에 `≫`(또는
/// `≪`, 또는 `≫` 뒤 `≪`)를 누른 결과를 이 출력과 바이트로 견준다. 이 시험은 그 JSON 이 지금 pw-git · git 의 출력과 같은지
/// 본다 — 다시 만들 때는 `PW_UPDATE_MERGE_CASES=1`.
#[test]
fn window_merge_cases_match_pw_git_and_git_merge_file() {
    const FIXTURE: &str = "../../apps/desktop/src/git/merge/fixtures/merge-cases.json";
    const PER_STYLE: usize = 80;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut rng = Rng(0x6d65_7267_6500_0019);
    let common = ["a", "b", "c", "d", "e", "{", "}", "", "한글"];
    let side = |dir: &Path, flag: &str, b: &str, o: &str, t: &str| -> String {
        for (name, data) in [("base", b), ("ours", o), ("theirs", t)] {
            std::fs::write(dir.join(name), data).unwrap();
        }
        let out = crate::testing::repo::git_command(dir, dir)
            .args([
                "merge-file",
                "-p",
                "--diff3",
                flag,
                "ours",
                "base",
                "theirs",
            ])
            .output()
            .unwrap();
        assert!(out.status.code().is_some_and(|c| (0..=127).contains(&c)));
        String::from_utf8(out.stdout).unwrap()
    };
    let mut cases = Vec::new();
    for style in 0..3 {
        let mut found = 0;
        while found < PER_STYLE {
            let n = rng.below(14);
            let base: Vec<String> = (0..n)
                .map(|_| common[rng.below(common.len())].to_owned())
                .collect();
            let ours = mutate(&mut rng, &base, &["a", "b", "o", "x\ry", "가", ""]);
            let theirs = mutate(&mut rng, &base, &["a", "c", "t", "x\ry", "가", ""]);
            let mut join = |lines: &[String]| -> String {
                let mut text = String::new();
                for l in lines {
                    let l = if style == 2 {
                        l.clone()
                    } else {
                        l.replace('\r', "")
                    };
                    let eol = match style {
                        0 => "\n",
                        1 => "\r\n",
                        _ => ["\n", "\r\n"][rng.below(2)],
                    };
                    text.push_str(&l);
                    text.push_str(eol);
                }
                if !text.is_empty() && rng.below(8) == 0 {
                    text.pop();
                    if text.ends_with('\r') && style == 1 {
                        text.pop();
                    }
                }
                text
            };
            let (b, o, t) = (join(&base), join(&ours), join(&theirs));
            let m = merge(b.as_bytes(), o.as_bytes(), t.as_bytes());
            if m.conflicts().count() == 0 {
                continue;
            }
            found += 1;
            let name = ["lf", "crlf", "mixed"][style];
            cases.push(serde_json::json!({
                "style": name,
                "base": b,
                "ours": o,
                "theirs": t,
                "merged": String::from_utf8(m.text()).unwrap(),
                "regions": super::super::regions_of(&m),
                "ours_result": side(dir, "--ours", &b, &o, &t),
                "theirs_result": side(dir, "--theirs", &b, &o, &t),
                "union_result": side(dir, "--union", &b, &o, &t),
            }));
        }
    }
    let got = serde_json::Value::Array(cases);
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    if std::env::var_os("PW_UPDATE_MERGE_CASES").is_some() {
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&got).unwrap())).unwrap();
        return;
    }
    let want: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    assert!(
        got == want,
        "{} 이 지금 pw-git · git 의 출력과 다르다 — PW_UPDATE_MERGE_CASES=1 로 이 시험을 돌려 다시 만든다",
        path.display()
    );
}
