//! Linguist `samples/` 전체의 일치율(M1-02 완료 기준). samples 는 36MB · 3천여 파일이라 저장소에 넣지 않는다 —
//! `mise run lang-samples` 가 고정 커밋(`linguist/VERSION`)의 samples 를 받아 `PITWALL_LINGUIST_SAMPLES` 로 넘기고 이
//! 시험만 돌린다. 기본 `mise run test` 에서는 `#[ignore]` 로 빠진다(네트워크가 필요하다).
//!
//! 판정은 Linguist 의 `Samples.each` 와 같다: `samples/<언어>/<파일>` 은 파일 이름 · 내용으로, `samples/<언어>/filenames/<이름>`
//! 은 그 이름으로 판별해 폴더 이름(언어 이름)과 견준다. `.gitattributes` · 설정 매핑은 쓰지 않는다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 착수 때 정한 하한(`docs/design/12-m1-plan.md` M1-02 행의 권장값). 분류기 단계를 옮기지 않으므로 100% 가 아니다.
const MIN_RATE: f64 = 0.95;

fn samples() -> Vec<(String, PathBuf, String)> {
    let root = std::env::var_os("PITWALL_LINGUIST_SAMPLES")
        .map(PathBuf::from)
        .expect("PITWALL_LINGUIST_SAMPLES 가 없다 — `mise run lang-samples` 로 돌린다");
    let mut out = Vec::new();
    let mut languages: Vec<_> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .map(|e| e.unwrap().path())
        .collect();
    languages.sort();
    for dir in languages {
        let language = dir.file_name().unwrap().to_string_lossy().into_owned();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            if file == "filenames" {
                for sub in std::fs::read_dir(&path).unwrap() {
                    let sub = sub.unwrap().path();
                    let name = sub.file_name().unwrap().to_string_lossy().into_owned();
                    out.push((
                        language.clone(),
                        sub,
                        format!("{language}/filenames/{name}"),
                    ));
                }
            } else {
                out.push((language.clone(), path, format!("{language}/{file}")));
            }
        }
    }
    out
}

fn rel(path: &Path) -> String {
    path.display().to_string()
}

#[test]
#[ignore = "needs the Linguist samples: run `mise run lang-samples`"]
fn linguist_samples_match_rate() {
    let all = samples();
    assert!(all.len() > 1000, "samples 가 너무 적다: {}", all.len());
    let mut misses: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut matched = 0usize;
    for (expected, path, sample) in &all {
        let data = std::fs::read(path).unwrap();
        // Linguist 의 시험처럼 samples 안의 경로를 이름으로 준다(heuristics 는 경로 끝의 확장자를 본다).
        let detected = pw_lang::linguist::detect(&format!("samples/{sample}"), Some(&data))
            .map(|(id, _)| pw_lang::linguist::language(id));
        if detected.is_some_and(|l| l.name == expected || l.fs_name == Some(expected.as_str())) {
            matched += 1;
        } else {
            misses
                .entry(format!(
                    "{expected} → {}",
                    detected.map_or("(none)", |l| l.name)
                ))
                .or_default()
                .push(rel(Path::new(sample)));
        }
    }
    let rate = matched as f64 / all.len() as f64;
    let mut ranked: Vec<_> = misses.iter().collect();
    ranked.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (pair, files) in &ranked {
        println!("miss {:>3}  {pair}  e.g. {}", files.len(), files[0]);
    }
    println!(
        "linguist samples: {matched}/{} matched ({:.2}%), floor {:.0}%",
        all.len(),
        rate * 100.0,
        MIN_RATE * 100.0
    );
    assert!(
        rate >= MIN_RATE,
        "일치율 {:.2}% 가 하한 {:.0}% 아래다",
        rate * 100.0,
        MIN_RATE * 100.0
    );
}
