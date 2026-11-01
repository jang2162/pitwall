# 덮어쓰기 정의

`<Mason 패키지 이름>.toml` 하나가 그 패키지의 registry 정의 위에 Pitwall 이 받을 판 · 주소 · `bin` 을 덮어 고정한다.
형식과 규칙은 `crates/pw-tools/src/overrides.rs` 머리 주석(15-m4-plan.md M4-03 ⑦). 빌드가 이 폴더의 `*.toml` 을 모두
바이너리에 묶는다(`build.rs`).

- 덮어쓰기를 더하거나 고치면 `node scripts/tools-registry-bump.mjs bump <지금 판>` 으로 `registry.toml` 의 고정 해시를 다시
  적는다 — 받을 주소가 바뀌므로 해시도 바뀐다. `mise run lint` 가 덮어쓴 주소와 고정 표가 맞는지 본다.
- 지금은 비어 있다. jdtls 의 정식 마일스톤 · 판 고정 lombok 은 M4-18 이 더한다(결정 M4-D4).
