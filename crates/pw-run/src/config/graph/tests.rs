use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::config::{RunConfig, RunKindConfig, SqlTransaction};

fn shell(before: &[&str]) -> RunConfig {
    RunConfig {
        name: "x".to_owned(),
        kind: RunKindConfig::Shell {
            command: "true".to_owned(),
        },
        cwd: None,
        env: BTreeMap::new(),
        env_file: Vec::new(),
        profiles: BTreeMap::new(),
        debug: None,
        before: before.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn compound(configs: &[&str], mode: CompoundMode) -> RunConfig {
    RunConfig {
        kind: RunKindConfig::Compound {
            configs: configs.iter().map(|s| (*s).to_owned()).collect(),
            mode,
        },
        ..shell(&[])
    }
}

fn sql() -> RunConfig {
    RunConfig {
        kind: RunKindConfig::Sql {
            connection: "db".to_owned(),
            files: vec!["seed.sql".to_owned()],
            transaction: SqlTransaction::All,
        },
        ..shell(&[])
    }
}

fn set(items: &[(&str, RunConfig)]) -> BTreeMap<String, RunConfig> {
    items
        .iter()
        .map(|(k, c)| ((*k).to_owned(), c.clone()))
        .collect()
}

fn checked(items: &[(&str, RunConfig)]) -> BTreeMap<String, String> {
    check(&set(items), &BTreeSet::new())
}

fn chain(items: &[(&str, RunConfig)], key: &str) -> Chain {
    let configs = set(items);
    expand(&|k| configs.get(k), &|_| false, key).expect("expands")
}

#[test]
fn before_runs_first_and_compounds_unfold_in_place() {
    let c = chain(
        &[
            ("seed", sql()),
            ("api", shell(&["seed"])),
            ("web", shell(&[])),
            ("all", compound(&["api", "web"], CompoundMode::Parallel)),
        ],
        "all",
    );
    let keys: Vec<&str> = c.steps.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["seed", "api", "web"]);
    assert_eq!(
        c.root,
        Node::Parallel(vec![
            Node::Sequence(vec![Node::Step(0), Node::Step(1)]),
            Node::Step(2)
        ])
    );
    assert_eq!(c.order_text(), "(seed, then api) and web together");
    assert_eq!(c.compounds.len(), 1);
    assert!(
        c.steps.iter().all(|s| !s.top),
        "the compound itself is the top"
    );
}

#[test]
fn a_loop_is_unusable_wherever_it_closes() {
    // 자기 자신.
    let r = checked(&[("a", shell(&["a"]))]);
    assert!(r["a"].contains("loops back: a → a"), "{r:?}");
    // 둘 사이 — 둘 다 쓸 수 없다.
    let r = checked(&[("a", shell(&["b"])), ("b", shell(&["a"]))]);
    assert!(r["a"].contains("a → b → a"), "{r:?}");
    assert!(r["b"].contains("b → a → b"), "{r:?}");
    // 복합 구성을 거쳐 돈다.
    let r = checked(&[
        ("c", compound(&["a"], CompoundMode::Sequence)),
        ("a", shell(&["c"])),
    ]);
    assert!(r["c"].contains("loops back"), "{r:?}");
    assert!(r["a"].contains("loops back"), "{r:?}");
    // 순환을 가리키기만 하는 구성도 쓸 수 없다.
    let r = checked(&[
        ("a", shell(&["b"])),
        ("b", shell(&["a"])),
        ("top", shell(&["a"])),
    ]);
    assert!(r.contains_key("top"), "{r:?}");
}

#[test]
fn missing_unusable_deep_wide_and_doubled_sql_references_are_unusable() {
    let r = checked(&[("a", shell(&["nope"]))]);
    assert!(r["a"].contains("`nope`, which does not exist"), "{r:?}");

    let configs = set(&[("a", shell(&["broken"]))]);
    let r = check(&configs, &BTreeSet::from(["broken".to_owned()]));
    assert!(r["a"].contains("which cannot be used"), "{r:?}");

    // 깊이: c0 → c1 → … → c9.
    let mut deep: Vec<(String, RunConfig)> =
        (0..9).map(|i| (format!("c{i}"), shell(&[]))).collect();
    for (i, (_, config)) in deep.iter_mut().enumerate().take(8) {
        config.before = vec![format!("c{}", i + 1)];
    }
    let items: Vec<(&str, RunConfig)> = deep.iter().map(|(k, c)| (k.as_str(), c.clone())).collect();
    let r = checked(&items);
    assert!(r["c0"].contains("deeper than"), "{r:?}");
    assert!(
        !r.contains_key("c1"),
        "c1 → … → c8 is within the depth: {r:?}"
    );

    // 넓이: 같은 build 를 33번.
    let many: Vec<&str> = std::iter::repeat_n("build", MAX_STEPS + 1).collect();
    let r = checked(&[
        ("build", shell(&[])),
        ("all", compound(&many, CompoundMode::Parallel)),
    ]);
    assert!(r["all"].contains("more than"), "{r:?}");

    // 같은 sql 구성이 한 사슬에 두 번.
    let r = checked(&[
        ("seed", sql()),
        ("api", shell(&["seed"])),
        ("web", shell(&["seed"])),
        ("all", compound(&["api", "web"], CompoundMode::Parallel)),
    ]);
    assert!(r["all"].contains("runs twice"), "{r:?}");
    assert!(!r.contains_key("api"), "{r:?}");
}

#[test]
fn a_config_without_references_is_not_a_chain() {
    assert!(!is_chain(&shell(&[])));
    assert!(is_chain(&shell(&["a"])));
    assert!(is_chain(&compound(&["a"], CompoundMode::Sequence)));
    let c = chain(&[("a", shell(&[]))], "a");
    assert_eq!(c.root, Node::Step(0));
    assert!(c.steps[0].top);
}

#[test]
fn a_chain_and_its_before_part_carry_the_chosen_configs_name() {
    let mut app = shell(&["seed"]);
    app.name = "My App".to_owned();
    let c = chain(&[("seed", shell(&[])), ("app", app)], "app");
    assert_eq!(c.name, "My App");
    assert_eq!(c.before_part().expect("has before").name, "My App");
    let mut unnamed = compound(&["a", "b"], CompoundMode::Sequence);
    unnamed.name = String::new();
    let combo = chain(
        &[("a", shell(&[])), ("b", shell(&[])), ("combo", unnamed)],
        "combo",
    );
    assert_eq!(combo.name, "combo", "an empty name falls back to the key");
}
