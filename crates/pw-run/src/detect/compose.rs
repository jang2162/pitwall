//! compose 파일의 `services` → 종류 `docker-compose` 의 가상 구성(`docker compose -f <파일> up <서비스>`). `docker` 를 부르지 않고
//! YAML 을 읽는다:
//!
//! - 파일은 compose 가 찾는 순서 `compose.yaml` · `compose.yml` · `docker-compose.yaml` · `docker-compose.yml` 의 첫 것이고, 구성은
//!   그 파일을 `file` 로 적는다 — 덮어쓰기 파일(`compose.override.yaml`)은 섞지 않는다(정의 본문이 그 파일 하나의 것이어야 한다).
//!   `include` · `extends` 는 따라가지 않는다.
//! - **YAML 은 이벤트로만 읽는다**(`yaml_rust2::parser`) — `YamlLoader` 는 별칭(`*a`)을 만날 때마다 앵커의 값을 복사해, 별칭을 겹친
//!   1 MiB 파일 하나로 메모리가 폭발한다. 여기는 노드를 한 번씩만 들고, 정의 본문을 쓸 때만 별칭을 펼치되 서비스마다의 길이 · 깊이 상한과 서비스 전부의 합계
//!   상한([`super::Budget`])을 쓰는 도중에 센다.
//! - 정의 본문: 그 서비스의 값(별칭 · `<<` 병합을 펼친 것)을 정규 글로 — 주석 · 공백 · 따옴표 꼴과 무관하고 태그(`!reset` 등)와
//!   「따옴표를 쓴 값인가」 는 든다. `command` · `entrypoint` · `environment` 가 바뀌면 본문이 바뀐다(결정 M6-D6 ⓒ).

use std::collections::HashMap;

use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use super::{
    Budget, DetectContext, DetectedTask, Found, TaskDetector, file, over_budget, pick_names,
};
use crate::config::{RunConfig, RunKindConfig};

pub const ID: &str = "compose";
/// compose 가 찾는 순서.
pub const SOURCES: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// 서비스 하나의 정의 본문 상한(별칭을 펼친 길이). 서비스 전부의 합계는 [`super::MAX_DEFINITION_BYTES`].
pub const MAX_DEFINITION: usize = 256 * 1024;
/// 정의 본문을 쓸 때의 깊이 상한(별칭을 지나는 것 포함).
const MAX_DEPTH: usize = 64;

pub struct ComposeDetector;

impl TaskDetector for ComposeDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn sources(&self) -> &'static [&'static str] {
        SOURCES
    }

    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String> {
        let Some(name) = SOURCES.iter().find(|n| cx.entries.contains(**n)) else {
            return Ok(Found::default());
        };
        let Some((path, text)) = file::read(cx.root, name)? else {
            return Ok(Found::default());
        };
        let mut found = Found::default();
        let services =
            services(&text, name, &mut found.skipped).map_err(|why| format!("{name}: {why}"))?;
        for (service, definition) in services {
            found.tasks.push(DetectedTask {
                detector: ID,
                config: RunConfig {
                    name: service.clone(),
                    kind: RunKindConfig::DockerCompose {
                        services: vec![service.clone()],
                        file: Some((*name).to_owned()),
                    },
                    cwd: None,
                    env: Default::default(),
                    env_file: Vec::new(),
                    profiles: Default::default(),
                    debug: None,
                    before: Vec::new(),
                },
                name: service,
                source: path.clone(),
                definition,
            });
        }
        Ok(found)
    }
}

/// 노드(별칭은 펼치지 않는다).
enum Node {
    Scalar {
        value: String,
        quoted: bool,
        tag: Option<String>,
    },
    Seq(Vec<usize>, Option<String>),
    Map(Vec<(usize, usize)>, Option<String>),
    Alias(usize),
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    /// 열린 묶음(노드 자리, 맵이면 아직 값을 받지 못한 키).
    open: Vec<(usize, Option<usize>)>,
    /// 앵커 id → 노드 자리.
    anchors: HashMap<usize, usize>,
    /// 문서마다의 뿌리.
    roots: Vec<usize>,
}

fn tag_text(tag: Option<Tag>) -> Option<String> {
    tag.map(|t| format!("{}{}", t.handle, t.suffix))
}

impl Builder {
    fn add(&mut self, node: Node, anchor: usize) -> usize {
        let at = self.nodes.len();
        self.nodes.push(node);
        if anchor > 0 {
            self.anchors.insert(anchor, at);
        }
        match self.open.last_mut() {
            None => self.roots.push(at),
            Some((parent, key)) => match &mut self.nodes[*parent] {
                Node::Seq(items, _) => items.push(at),
                Node::Map(entries, _) => match key.take() {
                    None => *key = Some(at),
                    Some(k) => entries.push((k, at)),
                },
                _ => {}
            },
        }
        at
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, event: Event, _mark: Marker) {
        match event {
            Event::Scalar(value, style, anchor, tag) => {
                let quoted = style != TScalarStyle::Plain;
                self.add(
                    Node::Scalar {
                        value,
                        quoted,
                        tag: tag_text(tag),
                    },
                    anchor,
                );
            }
            Event::Alias(id) => {
                self.add(Node::Alias(id), 0);
            }
            Event::SequenceStart(anchor, tag) => {
                let at = self.add(Node::Seq(Vec::new(), tag_text(tag)), anchor);
                self.open.push((at, None));
            }
            Event::MappingStart(anchor, tag) => {
                let at = self.add(Node::Map(Vec::new(), tag_text(tag)), anchor);
                self.open.push((at, None));
            }
            Event::SequenceEnd | Event::MappingEnd => {
                self.open.pop();
            }
            _ => {}
        }
    }
}

/// compose 글의 (서비스 이름, 정의 본문) — 이름순, [`pick_names`] 가 고른 것만. 형식 오류 · 상한 넘김은 까닭(줄 · 칸만 — 내용은
/// 싣지 않는다). 정의는 고른 서비스만 만들고, 합계를 만드는 도중에 [`Budget`] 으로 센다 — 한 앵커를 가리키는 서비스 수천이 그 수만큼
/// 펼쳐지지 않는다. `source` 는 까닭에 쓰는 파일 이름, 뺀 서비스의 까닭은 `skipped` 에 더한다.
pub fn services(
    text: &str,
    source: &str,
    skipped: &mut Vec<String>,
) -> Result<Vec<(String, String)>, String> {
    let mut builder = Builder::default();
    Parser::new_from_str(text)
        .load(&mut builder, false)
        .map_err(|e| {
            format!(
                "not valid YAML at line {}, column {}",
                e.marker().line(),
                e.marker().col() + 1
            )
        })?;
    let Some(&root) = builder.roots.first() else {
        return Ok(Vec::new());
    };
    let Node::Map(top, _) = &builder.nodes[root] else {
        return Err("the top level is not a mapping".to_owned());
    };
    let services = top.iter().find_map(|(k, v)| {
        matches!(&builder.nodes[*k], Node::Scalar { value, .. } if value == "services")
            .then_some(*v)
    });
    let Some(services) = services else {
        return Ok(Vec::new());
    };
    let entries = match &builder.nodes[services] {
        Node::Map(entries, _) => entries,
        Node::Scalar {
            value,
            quoted: false,
            ..
        } if value.is_empty() || value == "~" || value == "null" => {
            return Ok(Vec::new());
        }
        _ => return Err("`services` is not a mapping".to_owned()),
    };
    // 이름 → 값(같은 이름이 둘이면 앞의 것). `<<`(서비스 묶음 병합)는 서비스가 아니다.
    let mut values: HashMap<&str, usize> = HashMap::new();
    for (k, v) in entries {
        if let Node::Scalar { value: name, .. } = &builder.nodes[*k]
            && name != "<<"
        {
            values.entry(name.as_str()).or_insert(*v);
        }
    }
    let names = values.keys().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    let mut budget = Budget::default();
    let mut out = Vec::new();
    for name in pick_names(source, names, skipped) {
        let limit = MAX_DEFINITION.min(budget.left());
        let mut definition = String::new();
        write(&builder, values[name.as_str()], 0, limit, &mut definition).map_err(|e| match e {
            TooLong if limit < MAX_DEFINITION => over_budget(),
            TooLong => format!(
                "the service on line {} expands to more than {} KiB (through aliases)",
                line_of(text, &name),
                MAX_DEFINITION / 1024
            ),
            TooDeep => format!(
                "the service on line {} is nested deeper than {MAX_DEPTH} levels (through aliases)",
                line_of(text, &name)
            ),
        })?;
        budget.take(definition.len())?;
        out.push((name, definition));
    }
    Ok(out)
}

/// 정의를 쓰다 멈춘 까닭.
#[derive(Debug)]
enum WriteError {
    TooLong,
    TooDeep,
}
use WriteError::{TooDeep, TooLong};

/// 까닭에 이름을 싣지 않으려고 줄 번호만 — 그 이름이 키로 처음 나오는 줄.
fn line_of(text: &str, name: &str) -> usize {
    text.lines()
        .position(|l| {
            l.trim_start()
                .trim_start_matches(['"', '\''])
                .starts_with(name)
        })
        .map_or(0, |i| i + 1)
}

/// 노드를 정규 글로(별칭을 펼친다). 쓰는 **도중에** 길이가 `limit` 을 넘거나 깊이가 [`MAX_DEPTH`] 를 넘으면(스스로를 가리키는 별칭
/// 포함) 멈춘다.
fn write(
    builder: &Builder,
    at: usize,
    depth: usize,
    limit: usize,
    out: &mut String,
) -> Result<(), WriteError> {
    if depth > MAX_DEPTH {
        return Err(TooDeep);
    }
    if out.len() > limit {
        return Err(TooLong);
    }
    let tag = |out: &mut String, tag: &Option<String>| {
        if let Some(tag) = tag {
            out.push_str(tag);
            out.push(' ');
        }
    };
    match &builder.nodes[at] {
        Node::Scalar {
            value,
            quoted,
            tag: t,
        } => {
            tag(out, t);
            if *quoted {
                out.push('q');
            }
            if out.len() + value.len() > limit {
                return Err(TooLong);
            }
            out.push_str(&serde_json::Value::String(value.clone()).to_string());
        }
        Node::Seq(items, t) => {
            tag(out, t);
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(builder, *item, depth + 1, limit, out)?;
            }
            out.push(']');
        }
        Node::Map(entries, t) => {
            tag(out, t);
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(builder, *k, depth + 1, limit, out)?;
                out.push(':');
                write(builder, *v, depth + 1, limit, out)?;
            }
            out.push('}');
        }
        Node::Alias(id) => match builder.anchors.get(id) {
            Some(target) => write(builder, *target, depth + 1, limit, out)?,
            None => out.push_str("null"),
        },
    }
    Ok(())
}
