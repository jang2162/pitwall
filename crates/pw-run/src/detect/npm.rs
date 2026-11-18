//! `package.json` 의 `scripts` → 종류 `npm` 의 가상 구성(패키지 관리자는 실행 때 lockfile 로 — [`crate::config::argv`]).
//!
//! 정의 본문([`script_definition`])은 그 script 와 npm 이 앞뒤로 함께 돌리는 `pre<이름>` · `post<이름>` 의 본문이다 — 사람이 본
//! `dev` 의 `postdev` 가 바뀌어도 다시 묻는다.

use std::path::Path;

use serde_json::{Map, Value};

use super::{DetectContext, DetectedTask, Found, TaskDetector, file};
use crate::config::{RunConfig, RunKindConfig};

pub const ID: &str = "npm";
pub const SOURCE: &str = "package.json";

pub struct NpmDetector;

impl TaskDetector for NpmDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn sources(&self) -> &'static [&'static str] {
        &[SOURCE]
    }

    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String> {
        if !cx.entries.contains(SOURCE) {
            return Ok(Found::default());
        }
        let Some((path, text)) = file::read(cx.root, SOURCE)? else {
            return Ok(Found::default());
        };
        let Some(scripts) = scripts(&text, SOURCE)? else {
            return Ok(Found::default());
        };
        let mut found = Found::default();
        let mut not_text = 0;
        for (name, body) in &scripts {
            if !body.is_string() {
                not_text += 1;
                continue;
            }
            found.tasks.push(DetectedTask {
                detector: ID,
                name: name.clone(),
                source: path.clone(),
                config: RunConfig {
                    name: name.clone(),
                    kind: RunKindConfig::Npm {
                        script: name.clone(),
                    },
                    cwd: None,
                    env: Default::default(),
                    env_file: Vec::new(),
                    profiles: Default::default(),
                    debug: None,
                },
                definition: definition(&scripts, name).unwrap_or_default(),
            });
        }
        if not_text > 0 {
            found.skipped.push(format!(
                "{SOURCE}: skipped {not_text} script(s) whose value is not a string"
            ));
        }
        Ok(found)
    }
}

/// `package.json` 글의 `scripts`. 없으면 `None`, JSON 이 아니거나 `scripts` 가 객체가 아니면 까닭(줄 · 칸만 — 내용은 싣지 않는다).
fn scripts(text: &str, name: &str) -> Result<Option<Map<String, Value>>, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        format!(
            "{name}: not valid JSON at line {}, column {}",
            e.line(),
            e.column()
        )
    })?;
    match value.get("scripts") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map.clone())),
        Some(_) => Err(format!("{name}: `scripts` is not an object")),
    }
}

/// `script` 의 정의 본문: `{"pre<s>":…,"<s>":…,"post<s>":…}`(있는 것만, 이 순서). script 가 없거나 글이 아니면 `None`.
fn definition(scripts: &Map<String, Value>, script: &str) -> Option<String> {
    scripts.get(script)?.as_str()?;
    let mut out = String::from("{");
    let mut first = true;
    for key in [
        format!("pre{script}"),
        script.to_owned(),
        format!("post{script}"),
    ] {
        let Some(body) = scripts.get(&key).and_then(Value::as_str) else {
            continue;
        };
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&Value::String(key).to_string());
        out.push(':');
        out.push_str(&Value::String(body.to_owned()).to_string());
    }
    out.push('}');
    Some(out)
}

/// 종류 `npm` 구성의 정의 본문(「사람이 본 구성」 해시 — M6-11): 작업 폴더 `cwd`(루트 안의 실제 경로)에서 루트까지 올라가며 처음
/// 만나는 `package.json`(패키지 관리자가 쓰는 것)의 그 script 본문. 감지된 태스크의 [`DetectedTask::definition`] 과 같은 꼴이다.
/// `package.json` 이 없거나 그 script 가 없으면 `None`, 읽지 못하면(루트 밖 링크 · 너무 큼 · JSON 아님) 까닭.
pub fn script_definition(root: &Path, cwd: &Path, script: &str) -> Result<Option<String>, String> {
    let real = |p: &Path| std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    let (root_real, cwd_real) = (real(root)?, real(cwd)?);
    let Ok(below) = cwd_real.strip_prefix(&root_real) else {
        return Err(format!("{} is outside the project root", cwd.display()));
    };
    let mut dir = below.to_path_buf();
    loop {
        let rel = dir.join(SOURCE);
        let rel = rel.to_string_lossy();
        if let Some((_, text)) = file::read(root, &rel)? {
            return Ok(scripts(&text, &rel)?.and_then(|s| definition(&s, script)));
        }
        if !dir.pop() {
            return Ok(None);
        }
    }
}
