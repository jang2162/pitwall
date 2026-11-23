//! `pw-release` — 릴리스 워크플로의 판정(src/lib.rs). 사용법:
//!
//! ```text
//! pw-release plan --ref <GITHUB_REF> --event <GITHUB_EVENT_NAME> --on-main <true|false>
//!     이 실행의 버전 · 태그 · 채널 · 공개 여부 · MSI 버전 · Linux 번들 · 바이너리 업로드를 `key=value` 줄로 stdout 에
//!     낸다($GITHUB_OUTPUT 에 붙인다). 공개는 push 이벤트의 태그뿐이고, 그 태그가 소스 버전과 다르거나 main 밖이면 실패한다.
//!     `--on-main` 은 이 커밋이 origin/main 의 조상인지(`git merge-base --is-ancestor`).
//! pw-release manifest --ref <GITHUB_REF> --event <GITHUB_EVENT_NAME> --on-main <true|false> --repo <owner/name> --dir <산출물 폴더> --date <RFC 3339> --out <파일> [--notes <글>]
//!     updater 매니페스트(latest.json)를 쓴다.
//! pw-release relay-describe --file <바이너리> --platform <linux-x86_64|linux-aarch64|macos-aarch64|macos-x86_64> --out <항목.json>
//!     Relay 바이너리 하나의 항목(자산 이름 · sha256 · 크기)을 쓴다. 크기가 상한(`relay::MAX_BINARY_BYTES` — 18-m7-plan
//!     M7-P13)을 넘으면 실패한다. 크기는 stdout 에도 한 줄로 낸다(빌드마다 적는 기록).
//! pw-release relay-manifest --entries <항목 폴더> --out <relay-manifest.json> --complete <true|false> [--check <바이너리 폴더>]
//!     항목(`*.json`)들로 Relay 매니페스트를 쓴다. `--complete true`(릴리스)는 네 플랫폼이 다 있어야 한다. `--check` 를 주면 그
//!     폴더의 같은 이름 바이너리가 항목의 해시 · 크기와 같은지 다시 본다(공개 실행 — 올릴 바이트가 매니페스트의 바이트다).
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use pw_release::{SOURCE_VERSION, manifest, plan, relay};

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
        .ok_or("usage: pw-release plan|manifest|relay-describe|relay-manifest …")?;
    match command.as_str() {
        "plan" => {
            let options = options(rest, &["ref", "event", "on-main"])?;
            let plan = plan(
                required(&options, "ref")?,
                required(&options, "event")?,
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
                &[
                    "ref", "event", "on-main", "repo", "dir", "date", "out", "notes",
                ],
            )?;
            let plan = plan(
                required(&options, "ref")?,
                required(&options, "event")?,
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
        "relay-describe" => {
            let options = options(rest, &["file", "platform", "out"])?;
            let version = semver::Version::parse(SOURCE_VERSION).map_err(|e| e.to_string())?;
            let wanted = required(&options, "platform")?;
            let platform = relay::PLATFORMS
                .iter()
                .find(|p| p.asset == wanted)
                .ok_or_else(|| format!("unknown relay platform {wanted}"))?;
            let file = PathBuf::from(required(&options, "file")?);
            let entry = relay::describe(
                &file,
                &relay::asset_name(&version, platform),
                relay::MAX_BINARY_BYTES,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "relay binary size: {} {} bytes ({:.1} MiB, limit {:.1} MiB)",
                entry.name,
                entry.size,
                entry.size as f64 / 1048576.0,
                relay::MAX_BINARY_BYTES as f64 / 1048576.0
            );
            let out = required(&options, "out")?;
            let text = serde_json::to_string_pretty(&entry).map_err(|e| e.to_string())? + "\n";
            std::fs::write(out, text).map_err(|e| format!("cannot write {out}: {e}"))
        }
        "relay-manifest" => {
            let options = options(rest, &["entries", "out", "complete", "check"])?;
            let version = semver::Version::parse(SOURCE_VERSION).map_err(|e| e.to_string())?;
            let dir = PathBuf::from(required(&options, "entries")?);
            let mut entries = Vec::new();
            let listing = std::fs::read_dir(&dir)
                .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
            for item in listing {
                let path = item.map_err(|e| e.to_string())?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                let entry: relay::Entry = serde_json::from_str(&text)
                    .map_err(|e| format!("{} is not a relay entry: {e}", path.display()))?;
                entries.push(entry);
            }
            let manifest = relay::assemble(&version, &entries, flag(&options, "complete")?)
                .map_err(|e| e.to_string())?;
            if let Some(check) = get(&options, "check") {
                for entry in &entries {
                    let path = PathBuf::from(check).join(&entry.name);
                    let again =
                        relay::describe(&path, &entry.name, u64::MAX).map_err(|e| e.to_string())?;
                    if &again != entry {
                        return Err(format!(
                            "{} does not match its entry (built {} bytes {}, now {} bytes {})",
                            path.display(),
                            entry.size,
                            entry.sha256,
                            again.size,
                            again.sha256
                        ));
                    }
                }
            }
            let out = required(&options, "out")?;
            std::fs::write(out, manifest.to_bytes()).map_err(|e| format!("cannot write {out}: {e}"))
        }
        other => Err(format!(
            "unknown command {other} (plan | manifest | relay-describe | relay-manifest)"
        )),
    }
}
