//! 구성 사이의 참조 — `before` 와 복합 구성(`compound`)의 `configs`(17-m6-plan M6-21 — 결정 M6-D5 · D67).
//!
//! 구성 하나를 띄우는 것은 **사슬**([`Chain`])이다: 띄울 단계(프로세스 · `sql` 구성 — [`ChainStep`])와 그 차례([`Node`]).
//!
//! - `before = [a, b]` 는 `a` 가 끝 코드 0 으로 끝난 뒤 `b`, 그 뒤 본 구성이다([`Node::Sequence`] — 마지막 것 앞은 모두 끝을
//!   기다린다). `a` 가 다시 `before` · 복합 구성이면 그 사슬을 그 자리에 편다.
//! - 복합 구성의 `sequence` 는 같은 차례 사슬, `parallel` 은 함께 띄운다([`Node::Parallel`]).
//! - **쓸 수 없음**([`check`] — 읽을 때 `config::read` 가, 에이전트의 `run_config_upsert` 는 묻기 전에 본다): 없는 구성 · 쓸 수
//!   없는 구성을 가리킴, 순환(자기 자신 포함), 깊이 [`MAX_DEPTH`] 초과, 단계 [`MAX_STEPS`] 초과, 같은 `sql` 구성이 한 사슬에 두 번
//!   (창의 위험 확인은 한 번만 쓰인다 — 두 단계에 같은 확인을 쓰지 않는다).
//!
//! 같은 프로세스 구성은 한 사슬에 두 번 들 수 있다(두 구성이 같은 `build` 를 `before` 로 둔 것) — 두 번 돈다.

use std::collections::{BTreeMap, BTreeSet};

use super::{CompoundMode, RunConfig, RunKindConfig};

/// 사슬의 깊이 상한(구성 → `before` · `configs` 의 구성 → …). 넘으면 쓸 수 없다.
pub const MAX_DEPTH: usize = 8;
/// 사슬 하나의 단계 수 상한. 넘으면 쓸 수 없다(같은 구성을 여러 번 부르는 그물이 부풀지 않게).
pub const MAX_STEPS: usize = 32;

/// 띄울 단계 하나 — 프로세스 구성 또는 `sql` 구성(복합 구성이 아니다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainStep {
    pub key: String,
    pub config: RunConfig,
    /// 사슬의 맨 위 구성인가(입력의 프로필은 이 단계에만 얹는다).
    pub top: bool,
}

/// 차례.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// [`Chain::steps`] 의 그 단계.
    Step(usize),
    /// 차례로: 마지막 것 앞은 모두 끝 코드 0 으로 끝나야 다음이다.
    Sequence(Vec<Node>),
    /// 함께.
    Parallel(Vec<Node>),
}

/// 구성 하나를 띄우는 사슬.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub key: String,
    /// 띄울 단계(펼친 차례 — 왼쪽부터).
    pub steps: Vec<ChainStep>,
    /// 사슬이 지난 복합 구성(열쇠 순, 하나씩). 프로세스가 없어 단계가 아니지만 「사람이 본 구성」 의 대상이다(D68 — `configs` ·
    /// `mode` 도 구성의 필드).
    pub compounds: Vec<(String, RunConfig)>,
    pub root: Node,
}

impl Chain {
    /// 맨 위 프로세스 구성을 뺀 `before` 사슬(디버그 시작이 먼저 돌린다 — 맨 위 구성은 디버그로 띄운다). 맨 위가 복합 구성이거나
    /// `before` 가 없으면 `None`.
    pub fn before_part(&self) -> Option<Chain> {
        let Node::Sequence(items) = &self.root else {
            return None;
        };
        let top = self.steps.len().checked_sub(1)?;
        if !matches!(items.last(), Some(Node::Step(i)) if *i == top) || !self.steps[top].top {
            return None;
        }
        let mut items = items.clone();
        items.pop();
        Some(Chain {
            key: self.key.clone(),
            steps: self.steps[..top].to_vec(),
            compounds: self.compounds.clone(),
            root: Node::Sequence(items),
        })
    }

    /// 사람에게 보일 차례 한 줄(예: `seed, then (api and web together)`).
    pub fn order_text(&self) -> String {
        fn text(chain: &Chain, node: &Node) -> String {
            match node {
                Node::Step(i) => chain.steps[*i].key.clone(),
                Node::Sequence(items) => items
                    .iter()
                    .map(|n| wrap(chain, n))
                    .collect::<Vec<_>>()
                    .join(", then "),
                Node::Parallel(items) => format!(
                    "{} together",
                    items
                        .iter()
                        .map(|n| wrap(chain, n))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
            }
        }
        fn wrap(chain: &Chain, node: &Node) -> String {
            match node {
                Node::Step(_) => text(chain, node),
                _ => format!("({})", text(chain, node)),
            }
        }
        text(self, &self.root)
    }
}

/// 이 구성을 띄우는 것이 사슬인가(`before` 가 있거나 복합 구성) — 아니면 단계 하나다(사슬 없는 옛 길).
pub fn is_chain(config: &RunConfig) -> bool {
    !config.before.is_empty() || matches!(config.kind, RunKindConfig::Compound { .. })
}

/// 구성 `key` 의 사슬을 편다. `lookup` 은 쓸 수 있는 구성(열쇠 → 구성), `unusable` 은 쓸 수 없는 구성의 열쇠다. 오류 글에 값을
/// 쓰지 않는다(구성 id 만).
pub fn expand<'a>(
    lookup: &dyn Fn(&str) -> Option<&'a RunConfig>,
    unusable: &dyn Fn(&str) -> bool,
    key: &str,
) -> Result<Chain, String> {
    let config = lookup(key).ok_or_else(|| format!("run config `{key}` does not exist"))?;
    let mut walk = Walk {
        lookup,
        unusable,
        steps: Vec::new(),
        compounds: BTreeMap::new(),
        path: Vec::new(),
    };
    let root = walk.node(key, config, true)?;
    let mut seen_sql = BTreeSet::new();
    for step in &walk.steps {
        if matches!(step.config.kind, RunKindConfig::Sql { .. }) && !seen_sql.insert(&step.key) {
            return Err(format!(
                "the sql configuration `{}` runs twice in this chain; an sql step runs once per chain",
                step.key
            ));
        }
    }
    Ok(Chain {
        key: key.to_owned(),
        steps: walk.steps,
        compounds: walk.compounds.into_iter().collect(),
        root,
    })
}

struct Walk<'a, 'f> {
    lookup: &'f dyn Fn(&str) -> Option<&'a RunConfig>,
    unusable: &'f dyn Fn(&str) -> bool,
    steps: Vec<ChainStep>,
    compounds: BTreeMap<String, RunConfig>,
    /// 지금 펴는 길(순환 판정).
    path: Vec<String>,
}

impl<'a> Walk<'a, '_> {
    /// `key` 를 `from`(가리킨 구성과 필드)에서 찾아 편다.
    fn reference(&mut self, from: (&str, &str), key: &str) -> Result<Node, String> {
        let (owner, field) = from;
        if self.path.iter().any(|k| k == key) {
            let mut cycle = self.path.clone();
            cycle.push(key.to_owned());
            let start = cycle.iter().position(|k| k == key).unwrap_or(0);
            return Err(format!(
                "`{field}` of `{owner}` loops back: {}",
                cycle[start..].join(" → ")
            ));
        }
        let Some(config) = (self.lookup)(key) else {
            return Err(if (self.unusable)(key) {
                format!("`{field}` of `{owner}` refers to `{key}`, which cannot be used")
            } else {
                format!("`{field}` of `{owner}` refers to `{key}`, which does not exist")
            });
        };
        self.node(key, config, false)
    }

    fn node(&mut self, key: &str, config: &'a RunConfig, top: bool) -> Result<Node, String> {
        if self.path.len() >= MAX_DEPTH {
            return Err(format!(
                "the chain of `before` and compound configurations is deeper than {MAX_DEPTH} at `{key}`"
            ));
        }
        self.path.push(key.to_owned());
        let mut sequence = Vec::with_capacity(config.before.len() + 1);
        for before in &config.before {
            sequence.push(self.reference((key, "before"), before)?);
        }
        let own = match &config.kind {
            RunKindConfig::Compound { configs, mode } => {
                self.compounds
                    .entry(key.to_owned())
                    .or_insert_with(|| config.clone());
                let mut items = Vec::with_capacity(configs.len());
                for inner in configs {
                    items.push(self.reference((key, "configs"), inner)?);
                }
                match mode {
                    CompoundMode::Sequence => Node::Sequence(items),
                    CompoundMode::Parallel => Node::Parallel(items),
                }
            }
            _ => {
                if self.steps.len() >= MAX_STEPS {
                    return Err(format!(
                        "the chain starts more than {MAX_STEPS} configurations"
                    ));
                }
                self.steps.push(ChainStep {
                    key: key.to_owned(),
                    config: config.clone(),
                    top,
                });
                Node::Step(self.steps.len() - 1)
            }
        };
        self.path.pop();
        Ok(if sequence.is_empty() {
            own
        } else {
            sequence.push(own);
            Node::Sequence(sequence)
        })
    }
}

/// 구성들 사이의 참조를 본다(머리 주석 「쓸 수 없음」): 쓸 수 없게 되는 구성마다 그 까닭. `configs` 는 쓸 수 있는 구성, `unusable`
/// 은 이미 쓸 수 없는 구성의 열쇠.
pub fn check(
    configs: &BTreeMap<String, RunConfig>,
    unusable: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let lookup = |k: &str| configs.get(k);
    let is_unusable = |k: &str| unusable.contains(k);
    configs
        .iter()
        .filter(|(_, config)| is_chain(config))
        .filter_map(|(key, _)| {
            expand(&lookup, &is_unusable, key)
                .err()
                .map(|why| (key.clone(), why))
        })
        .collect()
}

#[cfg(test)]
mod tests;
