//! 레지스트리: 핸들러를 붙이지 않아도 `rpc_method!` 선언만으로 메서드와 타입이 모인다.

mod common;

use pw_rpc::{MethodKind, Role, Tier, methods};
use schemars::generate::SchemaSettings;
use specta::Types;
use specta::datatype::DataType;

#[test]
fn declarations_are_listed_by_name_without_a_router() {
    let names: Vec<_> = methods().iter().map(|m| m.meta.name).collect();
    assert_eq!(
        names,
        [
            "test.count",
            "test.dangerStream",
            "test.echo",
            "test.fail",
            "test.panic",
            "test.restart",
            "test.writeSetting",
        ]
    );
    let restart = methods()
        .into_iter()
        .find(|m| m.meta.name == "test.restart")
        .unwrap();
    assert_eq!(restart.meta.kind, MethodKind::Unary);
    assert_eq!(restart.meta.tier, Tier::Exec);
    assert_eq!(
        restart.meta.roles.iter().collect::<Vec<_>>(),
        [Role::Owner, Role::Mobile]
    );
}

#[test]
fn method_info_carries_the_real_types() {
    let all = methods();
    let echo = all.iter().find(|m| m.meta.name == "test.echo").unwrap();
    let count = all.iter().find(|m| m.meta.name == "test.count").unwrap();

    let mut types = Types::default();
    assert!(matches!(
        echo.params_type(&mut types),
        DataType::Reference(_)
    ));
    assert!(matches!(
        count.output_type(&mut types),
        DataType::Primitive(_)
    ));
    // 본문이 없는 항목(`String` 등 내장 타입의 자리)은 내보내지 않으므로 뺀다.
    let names: Vec<_> = types
        .into_sorted_iter()
        .filter(|t| t.ty.is_some())
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(names, ["EchoParams"]);

    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let schema = echo.params_schema(&mut generator);
    assert_eq!(
        schema.get("$ref").and_then(|v| v.as_str()),
        Some("#/$defs/EchoParams")
    );
    let defs = generator.take_definitions(true);
    assert_eq!(defs["EchoParams"]["required"], serde_json::json!(["text"]));
}

#[test]
fn tier_and_role_read_back_from_their_wire_names() {
    // M0-17 설정 파일(툴별 on/off · 프리셋)이 이 이름으로 등급 · role 을 적는다.
    assert_eq!(
        serde_json::from_str::<Tier>("\"dangerous\"").unwrap(),
        Tier::Dangerous
    );
    assert_eq!(
        serde_json::from_str::<Role>("\"readonly\"").unwrap(),
        Role::Readonly
    );
    assert!(serde_json::from_str::<Tier>("\"Read\"").is_err());
}
