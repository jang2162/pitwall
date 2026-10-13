# 포맷과 린트

## 코드를 쓴 뒤 `cargo fmt` 와 `cargo clippy` 를 돌린다 — 스타일을 손으로 맞추지 않는다

코드를 고친 뒤 포매터와 린터를 돌리고, 린터 경고는 `#[allow]` 로 덮기 전에 코드를 고친다.

- **Why:** rustfmt 가 강제하는 들여쓰기·줄바꿈·`use` 정렬을 손으로 맞추면 다음 `cargo fmt` 에서 되돌려져 불필요한 diff 만 남는다. clippy 경고를 쌓아 두면 새 경고가 묻힌다.
- **How to apply:**
  ```bash
  cargo fmt --all                                   # 포맷 적용
  cargo fmt --all --check                           # 검사만 (적용하지 않고 diff 출력)
  cargo clippy --all-targets --all-features -- -D warnings   # 린트. 경고도 실패로 본다
  ```
  - 워크스페이스는 `--workspace` 를 붙여 모든 멤버를 본다. 안 붙이면 현재 패키지만 검사된다.
  - `--all-targets` 가 없으면 테스트·예제·벤치 코드는 린트되지 않는다.
  - 린트 수준은 `Cargo.toml` 의 `[lints]` 와 `clippy.toml` 이 정본이다. 어느 린트를 끌 때는 코드 한 곳에 `#[allow(clippy::<린트>)]` 와 이유 주석을 둔다. 크레이트 전체로 끄지 않는다.
