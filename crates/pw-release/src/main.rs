//! `pw-release` — 릴리스 워크플로의 판정(src/lib.rs). 사용법:
//!
//! ```text
//! pw-release plan --ref <GITHUB_REF> --on-main <true|false>
//!     이 실행의 버전 · 태그 · 채널 · 공개 여부 · MSI 버전 · Linux 번들 · updater 키 · 바이너리 업로드를 `key=value` 줄로
//!     stdout 에 낸다($GITHUB_OUTPUT 에 붙인다). 태그(`refs/tags/v…`)가 소스 버전과 다르거나 main 밖이면 실패한다.
//!     `--on-main` 은 이 커밋이 origin/main 의 조상인지(`git merge-base --is-ancestor`).
//! pw-release publish-action --existing <none|draft|published>
//!     같은 태그의 Release 상태에 따라 `create` · `resume` 을 낸다. 이미 공개됐으면 실패한다.
//! pw-release manifest --ref <GITHUB_REF> --on-main <true|false> --repo <owner/name> --dir <산출물 폴더> --date <RFC 3339> --out <파일> [--notes <글>]
//!     updater 매니페스트(latest.json)를 쓴다.
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use pw_release::{SOURCE_VERSION, manifest, plan, publish_action};

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pw-release: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--이름 값` 짝. 모르는 인자 · 값 없는 인자는 오류다.
fn options(args: &[String], known: &[&str]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(name) = iter.next() {
        let key = name
            .strip_prefix("--")
            .filter(|k| known.contains(k))
            .ok_or_else(|| format!("unknown argument {name}"))?;
        let value = iter.next().ok_or_else(|| format!("{name} needs a value"))?;
        out.push((key.to_owned(), value.clone()));
    }
    Ok(out)
}

fn get<'a>(options: &'a [(String, String)], key: &str) -> Option<&'a str> {
    options
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn required<'a>(options: &'a [(String, String)], key: &str) -> Result<&'a str, String> {
    get(options, key).ok_or_else(|| format!("--{key} is required"))
}

fn flag(options: &[(String, String)], key: &str) -> Result<bool, String> {
    match required(options, key)? {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("--{key} must be true or false, not {other}")),
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let (command, rest) = args
        .split_first()
        .ok_or("usage: pw-release plan|manifest|publish-action …")?;
    match command.as_str() {
        "plan" => {
            let options = options(rest, &["ref", "on-main"])?;
            let plan = plan(
                required(&options, "ref")?,
                SOURCE_VERSION,
                flag(&options, "on-main")?,
            )
            .map_err(|e| e.to_string())?;
            print!("{}", plan.outputs());
            Ok(())
        }
        "manifest" => {
            let options = options(
                rest,
                &["ref", "on-main", "repo", "dir", "date", "out", "notes"],
            )?;
            let plan = plan(
                required(&options, "ref")?,
                SOURCE_VERSION,
                flag(&options, "on-main")?,
            )
            .map_err(|e| e.to_string())?;
            let value = manifest(
                &plan,
                required(&options, "repo")?,
                &PathBuf::from(required(&options, "dir")?),
                get(&options, "notes").unwrap_or(""),
                required(&options, "date")?,
            )
            .map_err(|e| e.to_string())?;
            let out = required(&options, "out")?;
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n";
            std::fs::write(out, text).map_err(|e| format!("cannot write {out}: {e}"))
        }
        "publish-action" => {
            let options = options(rest, &["existing"])?;
            println!(
                "{}",
                publish_action(required(&options, "existing")?).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        other => Err(format!(
            "unknown command {other} (plan | manifest | publish-action)"
        )),
    }
}
