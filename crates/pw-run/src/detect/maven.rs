//! Maven 태스크(17-m6-plan M6-05 ② — 결정 M6-D8 · D70, 메인 판정 「A」). Maven 에는 태스크 목록 명령이 없다 — 사람이 「태스크
//! 읽기」 를 누르면 [`read`] 가 `mvn help:effective-pom` 을 돌려 빌드 플러그인의 execution goal 을 접두사 관례(`maven-X-plugin` · `X-maven-plugin`
//! → `X`)로 `<접두사>:<goal>` 로 모으고, 라이프사이클 단계와 합쳐 캐시한다([`super::cache`]). 누르기 전에는 프로세스를 띄우지 않고
//! 라이프사이클 단계와 흔한 플러그인 goal(루트 `pom.xml` 에 그 플러그인이 보일 때 `spring-boot:run` · `quarkus:dev`)만 보인다.
//!
//! effective POM 은 0700 임시 폴더 안의 파일로 받고([`tempfile`]) 읽은 뒤 폴더째 지운다 — 설정의 값(저장소 주소 · 속성)이 든다.
//! XML 은 [`MAX_EFFECTIVE_POM`] 까지만 읽고, DTD(`<!DOCTYPE`)가 있으면 읽지 않으며 엔티티를 펼치지 않는다 — quick-xml 은 엔티티를
//! 스스로 펼치지 않고 `GeneralRef` 로 넘기며, 이 파서는 그런 값을 버린다.

use std::fs;
use std::io::Read as _;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

use super::cache::BuildFiles;
use super::gradle::build_tasks;
use super::{DetectContext, DetectExec, Found, TaskDetector, file};
use crate::config::RunKindConfig;

pub const ID: &str = "maven";
pub const SOURCE: &str = "pom.xml";

pub const WRAPPER: &str = if cfg!(windows) { "mvnw.cmd" } else { "mvnw" };

/// 라이프사이클 단계(읽기 전후 모두 보인다).
pub const LIFECYCLE: [&str; 6] = ["clean", "compile", "test", "package", "verify", "install"];

/// 루트 `pom.xml` 에 플러그인이 보이면 더하는 흔한 goal.
const COMMON_GOALS: [(&str, &str); 2] = [
    ("spring-boot-maven-plugin", "spring-boot:run"),
    ("quarkus-maven-plugin", "quarkus:dev"),
];

/// effective POM 파일의 크기 상한.
pub const MAX_EFFECTIVE_POM: u64 = 16 * 1024 * 1024;

pub struct MavenDetector;

impl TaskDetector for MavenDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn sources(&self) -> &'static [&'static str] {
        &[
            SOURCE,
            "maven-wrapper.properties",
            "maven.config",
            "extensions.xml",
        ]
    }

    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String> {
        if !cx.entries.contains(SOURCE) {
            return Ok(Found::default());
        }
        let mut found = Found::default();
        let names = match super::pressed_list(cx, ID, BuildFiles::Maven, &mut found) {
            Some(names) => names,
            None => common(cx)?,
        };
        build_tasks(ID, &cx.root.join(SOURCE), names, &mut found, |name| {
            RunKindConfig::Maven { tasks: vec![name] }
        });
        Ok(found)
    }
}

fn lifecycle() -> Vec<String> {
    LIFECYCLE.iter().map(|s| (*s).to_owned()).collect()
}

fn common(cx: &DetectContext<'_>) -> Result<Vec<String>, String> {
    let mut names = lifecycle();
    if let Some((_, text)) = file::read(cx.root, SOURCE)? {
        for (plugin, goal) in COMMON_GOALS {
            if text.contains(plugin) {
                names.push(goal.to_owned());
            }
        }
    }
    Ok(names)
}

/// 루트의 래퍼(있으면 절대 경로), 없으면 맨 이름 `mvn`.
fn program(root: &Path) -> String {
    let wrapper = root.join(WRAPPER);
    if wrapper.is_file() {
        wrapper.display().to_string()
    } else {
        "mvn".to_owned()
    }
}

/// 「태스크 읽기」: 라이프사이클 단계 + effective POM 의 플러그인 goal.
pub fn read(root: &Path, exec: &dyn DetectExec) -> Result<Vec<String>, String> {
    let dir = tempfile::Builder::new()
        .prefix("pitwall-maven-")
        .tempdir()
        .map_err(|e| format!("cannot make a temporary folder: {e}"))?;
    let out = dir.path().join("effective-pom.xml");
    let argv = vec![
        program(root),
        "--batch-mode".to_owned(),
        "--quiet".to_owned(),
        "help:effective-pom".to_owned(),
        format!("-Doutput={}", out.display()),
    ];
    exec.output(ID, &argv, root)?;
    let text = read_bounded(&out)?;
    drop(dir);
    let mut names = lifecycle();
    names.extend(plugin_goals(&text)?);
    Ok(names)
}

fn read_bounded(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path)
        .map_err(|_| "`mvn help:effective-pom` wrote no effective POM".to_owned())?;
    let mut bytes = Vec::new();
    file.take(MAX_EFFECTIVE_POM + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read the effective POM: {e}"))?;
    if bytes.len() as u64 > MAX_EFFECTIVE_POM {
        return Err(format!(
            "the effective POM is larger than {} MiB; Pitwall does not read it",
            MAX_EFFECTIVE_POM / (1024 * 1024)
        ));
    }
    String::from_utf8(bytes).map_err(|_| "the effective POM is not UTF-8".to_owned())
}

/// `artifactId` → goal 접두사(`maven-X-plugin` · `X-maven-plugin` → `X`). 관례 밖이면 `None`.
pub fn prefix(artifact: &str) -> Option<&str> {
    let p = artifact
        .strip_prefix("maven-")
        .and_then(|a| a.strip_suffix("-plugin"))
        .or_else(|| artifact.strip_suffix("-maven-plugin"))?;
    (!p.is_empty()).then_some(p)
}

/// effective POM(한 프로젝트 `<project>` 또는 여러 모듈 `<projects>`)의 `project/build/plugins/plugin` 마다 `<접두사>:<goal>`.
/// `pluginManagement` · `reporting` · `profiles` 아래는 보지 않는다. 순서는 나온 차례, 중복은 하나.
pub fn plugin_goals(xml: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut path: Vec<String> = Vec::new();
    let mut artifact: Option<String> = None;
    let mut goals: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    // 지금 글이 엔티티 참조로 끊겼는가(그 값은 버린다).
    let mut tainted = false;
    let mut text = String::new();
    loop {
        let event = reader.read_event().map_err(|e| {
            format!(
                "the effective POM is not valid XML at byte {}: {e}",
                reader.error_position()
            )
        })?;
        match event {
            Event::DocType(_) => {
                return Err("the effective POM has a DOCTYPE; Pitwall does not read it".to_owned());
            }
            Event::Start(start) => {
                path.push(start.local_name().as_ref().to_owned());
                text.clear();
                tainted = false;
                if in_plugin(&path) && path.len() == plugin_depth(&path) {
                    artifact = None;
                    goals.clear();
                }
            }
            Event::Text(t) => text.push_str(&t.into_inner()),
            Event::CData(c) => text.push_str(&c.into_inner()),
            Event::GeneralRef(_) => tainted = true,
            Event::End(_) => {
                let value = std::mem::take(&mut text);
                let value = value.trim();
                if in_plugin(&path) && !tainted {
                    let rest = &path[plugin_depth(&path)..];
                    match rest {
                        [a] if a == "artifactId" => artifact = Some(value.to_owned()),
                        [e, x, g, goal]
                            if e == "executions"
                                && x == "execution"
                                && g == "goals"
                                && goal == "goal" =>
                        {
                            goals.push(value.to_owned());
                        }
                        _ => {}
                    }
                }
                if in_plugin(&path) && path.len() == plugin_depth(&path) {
                    if let Some(prefix) = artifact.as_deref().and_then(prefix) {
                        for goal in goals.drain(..) {
                            let name = format!("{prefix}:{goal}");
                            if !out.contains(&name) {
                                out.push(name);
                            }
                        }
                    }
                    goals.clear();
                    artifact = None;
                }
                tainted = false;
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// `path` 가 `…/project/build/plugins/plugin` 이나 그 아래인가.
fn in_plugin(path: &[String]) -> bool {
    plugin_at(path).is_some()
}

/// `plugin` 요소까지의 깊이(그 요소 포함).
fn plugin_depth(path: &[String]) -> usize {
    plugin_at(path).map_or(usize::MAX, |i| i + 1)
}

fn plugin_at(path: &[String]) -> Option<usize> {
    // 맨 위는 `project` 이거나 `projects/project` 다.
    let base = match path {
        [p, ..] if p == "project" => 0,
        [ps, p, ..] if ps == "projects" && p == "project" => 1,
        _ => return None,
    };
    let rest = &path[base + 1..];
    match rest {
        [b, ps, p, ..] if b == "build" && ps == "plugins" && p == "plugin" => Some(base + 3),
        _ => None,
    }
}
