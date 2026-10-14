//! 타입 생성용 메서드 레지스트리: 핸들러 없이 선언만으로 메서드와 그 타입을 모은다.
//!
//! `rpc_method!` 선언마다 [`MethodInfo`] 하나가 [`inventory`] 로 등록된다. [`methods`] 는 그것을 이름순으로
//! 돌려주고, 생성기(`crates/pw-rpc-gen`)가 TS 타입과 JSON Schema 를, MCP 툴 레지스트리가 툴 목록을 여기서 만든다.
//! [`Router`](crate::Router) 를 채울 필요가 없다 — 핸들러는 런타임 구성이고 타입 목록은 선언의 속성이다.
//!
//! **링크된 크레이트의 선언만 보인다.** 메서드를 선언한 크레이트가 실행 파일에 링크되지 않으면 그 메서드는
//! 오류 없이 목록에서 빠진다. 생성기에 메서드 크레이트를 더하는 법은 `crates/pw-rpc-gen/src/main.rs` 머리 주석.

use std::collections::HashMap;

use schemars::{Schema, SchemaGenerator};
use serde::Serialize;
use specta::Types;
use specta::datatype::DataType;

use crate::method::{MethodMeta, StreamMethod, UnaryMethod};

/// `serde_json::Value` 필드를 TS 로 내보낼 때 쓰는 표식. 값으로 쓰지 않는다.
///
/// 필드 타입은 `serde_json::Value` 로 두고 `#[specta(type = pw_rpc::JsonValue)]` 를 단다.
/// specta 의 `Value` 구현은 숫자를 i64/u64 로 표현해 TS 내보내기가 BigInt 금지로 실패한다.
#[derive(Serialize, specta::Type)]
#[serde(untagged)]
pub enum JsonValue {
    // untagged 단위 변형은 serde 로는 `null` 인데 specta 는 `"Null"` 로 그린다. 그래서 타입을 덮는다.
    #[specta(type = ())]
    Null,
    Bool(bool),
    Number(JsNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(std::collections::BTreeMap<String, JsonValue>),
}

/// 64비트 정수 필드를 TS 의 `number` 로 내보내는 표식: `#[specta(type = pw_rpc::JsNumber)]`.
///
/// specta 는 i64 · u64 를 TS 로 내보내기를 거부한다(`number` 는 2^53 까지만 정확하다). 값이 그 안에 머무는
/// 필드(세션 안의 일련번호, 클라이언트가 고른 id 등)에만 단다. 넘을 수 있는 값은 문자열로 선언한다.
/// `f64` 로 덮지 않는 이유: specta 는 NaN 을 고려해 `f64` 를 `number | null` 로 그린다.
pub type JsNumber = u32;

#[derive(Clone, Copy)]
struct TypeFns {
    ts: fn(&mut Types) -> DataType,
    schema: fn(&mut SchemaGenerator) -> Schema,
}

impl TypeFns {
    const fn of<T: specta::Type + schemars::JsonSchema>() -> Self {
        Self {
            ts: <T as specta::Type>::definition,
            schema: SchemaGenerator::subschema_for::<T>,
        }
    }
}

/// 선언된 메서드 하나: 메타데이터와 입출력 타입. `rpc_method!` 가 등록한다.
pub struct MethodInfo {
    pub meta: MethodMeta,
    params: TypeFns,
    output: TypeFns,
}

impl MethodInfo {
    /// 레지스트리를 거치지 않고 메서드 하나의 정보를 만든다. `rpc_method!` 가 등록할 때 쓰고,
    /// 시험용 선언만 골라 생성할 때(`pw-rpc-gen-fixtures`)도 쓴다.
    pub const fn unary<M: UnaryMethod>() -> Self {
        Self {
            meta: M::META,
            params: TypeFns::of::<M::Params>(),
            output: TypeFns::of::<M::Result>(),
        }
    }

    /// [`MethodInfo::unary`] 의 stream 메서드판.
    pub const fn stream<M: StreamMethod>() -> Self {
        Self {
            meta: M::META,
            params: TypeFns::of::<M::Params>(),
            output: TypeFns::of::<M::Item>(),
        }
    }

    /// 입력 타입의 specta 정의. 이름 있는 타입은 `types` 에 등록되고 참조가 돌아온다.
    pub fn params_type(&self, types: &mut Types) -> DataType {
        (self.params.ts)(types)
    }

    /// unary 는 결과, stream 은 청크 항목 타입의 specta 정의.
    pub fn output_type(&self, types: &mut Types) -> DataType {
        (self.output.ts)(types)
    }

    /// 입력 타입의 JSON Schema. 이름 있는 타입은 `generator` 의 정의로 들어가고 `$ref` 가 돌아온다.
    pub fn params_schema(&self, generator: &mut SchemaGenerator) -> Schema {
        (self.params.schema)(generator)
    }

    /// unary 는 결과, stream 은 청크 항목 타입의 JSON Schema.
    pub fn output_schema(&self, generator: &mut SchemaGenerator) -> Schema {
        (self.output.schema)(generator)
    }
}

inventory::collect!(MethodInfo);

/// 링크된 모든 크레이트에서 선언된 메서드를 이름순으로.
///
/// # Panics
/// 두 선언이 같은 이름을 쓰면 패닉한다(프로그래밍 오류). [`Router`](crate::Router) 의 중복 등록 검사와 같은 규칙이다.
pub fn methods() -> Vec<&'static MethodInfo> {
    let mut seen = HashMap::new();
    for info in inventory::iter::<MethodInfo> {
        let previous = seen.insert(info.meta.name, info);
        assert!(
            previous.is_none(),
            "rpc method `{}` declared twice",
            info.meta.name
        );
    }
    let mut all: Vec<_> = seen.into_values().collect();
    all.sort_by_key(|m| m.meta.name);
    all
}

#[doc(hidden)]
pub mod __private {
    pub use inventory;
}
