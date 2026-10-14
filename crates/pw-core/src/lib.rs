//! Pitwall 코어. M0-07 이 채운다.

/// 씨앗 함수. 테스트 파이프라인이 실제로 도는지 확인하는 용도다.
pub fn crate_name() -> &'static str {
    "pw-core"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_pw_core() {
        assert_eq!(crate_name(), "pw-core");
    }
}
