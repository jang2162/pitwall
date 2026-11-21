//! 같은 이름(메서드 · 이벤트 토픽)을 두 번 선언하면 레지스트리가 거부한다. 다른 시험 바이너리를 오염시키지 않게 따로 둔다.

use pw_rpc::rpc_method;

rpc_method! {
    pub First: "dup.same" { params: (), result: (), tier: Read }
    pub Second: "dup.same" { params: (), result: (), tier: Read }
}

#[test]
#[should_panic(expected = "rpc method `dup.same` declared twice")]
fn duplicate_names_panic() {
    pw_rpc::methods();
}

pw_rpc::rpc_event!("dup.topic", roles: [Owner]);
pw_rpc::rpc_event!("dup.topic", roles: [Owner, Mobile, Readonly]);

#[test]
#[should_panic(expected = "event topic `dup.topic` declared twice")]
fn duplicate_topics_panic() {
    pw_rpc::events();
}
