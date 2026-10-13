# 툴체인

## rustfmt·clippy 는 `rust-toolchain.toml` 로 컴포넌트와 버전을 고정한다

저장소 루트에 `rust-toolchain.toml` 을 두고 컴포넌트를 적는다.

```toml
[toolchain]
channel = "<고정할 버전>"
components = ["rustfmt", "clippy"]
```

- **Why:** rustfmt 와 clippy 는 컴파일러 버전에 묶여 있고, 버전이 바뀌면 포맷 결과와 clippy 경고 목록이 달라진다. 사람과 CI 가 다른 버전을 쓰면 같은 파일이 한쪽에서는 「포맷 안 됨」, 다른 쪽에서는 새 경고로 판정된다. `components` 를 빼면 최소 프로파일 설치에서 `cargo fmt`·`cargo clippy` 가 「not installed」 로 실패한다.
- **How to apply:**
  - 지금 도는 버전은 `rustc --version`, `cargo fmt --version`, `cargo clippy --version` 으로 잰다.
  - 훅과 CI 는 같은 명령(`cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`)을 부른다. 훅에서는 `-D warnings` 를 빼지 않는다.
  - nightly 전용 rustfmt 옵션(`rustfmt.toml` 의 `unstable_features` 등)은 stable 에서 경고만 내고 무시된다. 쓰려면 툴체인도 nightly 로 고정해야 한다.
