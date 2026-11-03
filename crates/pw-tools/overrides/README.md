# 덮어쓰기 정의

`<Mason 패키지 이름>.toml` 하나가 그 패키지의 registry 정의 위에 Pitwall 이 받을 판 · 주소 · `bin` 을 덮어 고정한다.
형식과 규칙은 `crates/pw-tools/src/overrides.rs` 머리 주석(15-m4-plan.md M4-03 ⑦). 빌드가 이 폴더의 `*.toml` 을 모두
바이너리에 묶는다(`build.rs`).

- 덮어쓰기를 더하거나 고치면 `node scripts/tools-registry-bump.mjs bump <지금 판>` 으로 `registry.toml` 의 고정 해시를 다시
  적는다 — 받을 주소가 바뀌므로 해시도 바뀐다. `mise run lint` 가 덮어쓴 주소와 고정 표가 맞는지 본다.
- `kotlin-lsp.toml`: 실행 파일 별칭 `kotlin-lsp` → `intellij-server`(언어 확장의 `command` 와 Mason `bin` 이름을 맞춘다).
- `jdtls.toml`: 같은 판의 정식 마일스톤(`download.eclipse.org/jdtls/milestones/<판>/`) · 판 고정 lombok(Maven Central)을 받고, 설치 폴더에 런처 `pitwall/jdtls`(셸 · cmd — `[launcher]`)를 두어 `java -jar plugins/org.eclipse.equinox.launcher_*.jar` 로 띄운다(registry 의 python 런처 대신 — 결정 M4-D4, 15-m4-plan.md M4-18).
