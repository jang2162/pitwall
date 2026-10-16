//! 설정 레이어 병합과 TOML → JSON 변환.
//!
//! 병합 규칙(`docs/design/03-architecture.md` 「5. 데이터 저장」 의 병합 규칙 항목이 이 함수를 가리킨다):
//! - 양쪽이 테이블이면 **키별로 재귀 병합**한다. 위 레이어에 없는 키는 아래 레이어 값이 남는다.
//! - 그 밖(스칼라 · 배열 · 테이블의 배열 · 타입이 서로 다른 값)은 **위 레이어 값이 통째로 이긴다.** 배열은 이어 붙이지 않는다.
//! - 위 레이어에서 키를 지우는 문법은 없다. TOML 에 null 이 없어서다.

use serde_json::{Map, Value};
use toml_edit::{Item, Table, TableLike};

/// `over` 를 `base` 위에 얹는다.
pub fn merge(base: &mut Map<String, Value>, over: Map<String, Value>) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Object(below)), Value::Object(above)) => merge(below, above),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// TOML 테이블을 JSON 객체로. 날짜 · 시각은 TOML 표기 그대로의 문자열, 유한하지 않은 float 은 null 이 된다(JSON 에 없다).
pub fn to_json(table: &Table) -> Map<String, Value> {
    table_like(table)
}

fn table_like(table: &dyn TableLike) -> Map<String, Value> {
    table
        .iter()
        .filter_map(|(k, item)| item_to_json(item).map(|v| (k.to_owned(), v)))
        .collect()
}

fn item_to_json(item: &Item) -> Option<Value> {
    match item {
        Item::None => None,
        Item::Value(value) => Some(value_to_json(value)),
        Item::Table(table) => Some(Value::Object(table_like(table))),
        Item::ArrayOfTables(array) => Some(Value::Array(
            array.iter().map(|t| Value::Object(table_like(t))).collect(),
        )),
    }
}

fn value_to_json(value: &toml_edit::Value) -> Value {
    use toml_edit::Value as V;
    match value {
        V::String(s) => Value::String(s.value().clone()),
        V::Integer(i) => Value::from(*i.value()),
        V::Float(f) => Value::from(*f.value()),
        V::Boolean(b) => Value::Bool(*b.value()),
        V::Datetime(d) => Value::String(d.value().to_string()),
        V::Array(a) => Value::Array(a.iter().map(value_to_json).collect()),
        V::InlineTable(t) => Value::Object(table_like(t)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use toml_edit::DocumentMut;

    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => panic!("object"),
        }
    }

    #[test]
    fn tables_merge_deeply_and_everything_else_is_replaced() {
        let mut base = obj(json!({
            "name": "base",
            "editor": {"tab": 4, "font": {"size": 12, "family": "mono"}},
            "list": [1, 2, 3],
            "shape": {"a": 1},
            "kept": true,
        }));
        merge(
            &mut base,
            obj(json!({
                "name": "over",
                "editor": {"font": {"size": 14}},
                "list": [9],
                "shape": "scalar wins over table",
            })),
        );
        assert_eq!(
            Value::Object(base),
            json!({
                "name": "over",
                "editor": {"tab": 4, "font": {"size": 14, "family": "mono"}},
                "list": [9],
                "shape": "scalar wins over table",
                "kept": true,
            })
        );
    }

    #[test]
    fn toml_converts_to_json() {
        let doc: DocumentMut = r#"
            s = "x"
            i = 1
            f = 1.5
            b = true
            d = 1979-05-27T07:32:00Z
            arr = [1, "two"]
            inline = { k = "v" }
            [t]
            n = 2
            [[aot]]
            a = 1
            [[aot]]
            a = 2
        "#
        .parse()
        .unwrap();
        assert_eq!(
            Value::Object(to_json(doc.as_table())),
            json!({
                "s": "x", "i": 1, "f": 1.5, "b": true, "d": "1979-05-27T07:32:00Z",
                "arr": [1, "two"], "inline": {"k": "v"}, "t": {"n": 2},
                "aot": [{"a": 1}, {"a": 2}],
            })
        );
    }
}
