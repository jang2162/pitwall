//! `relay-live` 시험 표지(18-m7-plan M7-05 ③): `PW_TEST_RELAY_SSH_URL` 이 비어 있지 않으면 `cfg(pw_live_relay)` 를 켠다.
//!
//! live 시험(`tests/live.rs`)은 이 표지로 단다 — 변수가 없으면 시험이 건너뛰어지는 것이 아니라 **목록에 없다**. 그래서
//! `cargo nextest run -E 'test(/live_relay_/)' --no-tests=fail` 은 sshd 없이 돌리면 실패한다(`crates/pw-db-driver/live_cfg.rs` 와
//! 같은 꼴). 변수를 바꾸면 이 크레이트의 시험만 다시 컴파일된다.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(pw_live_relay)");
    println!("cargo::rerun-if-env-changed=PW_TEST_RELAY_SSH_URL");
    if std::env::var_os("PW_TEST_RELAY_SSH_URL").is_some_and(|v| !v.is_empty()) {
        println!("cargo::rustc-cfg=pw_live_relay");
    }
}
