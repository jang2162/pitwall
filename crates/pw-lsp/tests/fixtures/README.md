# live 시험 픽스처

실 언어 서버 시험(`crates/pw-lsp/tests/live_lsp.rs` — `live_lsp_<서버>_…`, CI `lsp-live` 잡)이 여는 작은 프로젝트다. 주인은 M4-05 이고 기능 행(M4-06 · 08 · 10 · 11 · 12 · 13 · 16)은 이 위에 시험을 단다(`docs/design/15-m4-plan.md` §2 「공유 부품의 주인」).

| 폴더 | 서버 | 꼴 |
|---|---|---|
| `rust/` | rust-analyzer | 독립 Cargo 작업 공간(`[workspace]` — 저장소 작업 공간에 들지 않는다), 의존 없음 |
| `typescript/` | typescript-language-server | `package.json` · `tsconfig.json`(strict · `noUnusedLocals`), 의존 없음 — 서버에 묶인 `typescript` 를 쓴다 |
| `python/` | basedpyright · ruff | `pyproject.toml`(basedpyright 는 `standard` 검사 — 기본 `recommended` 는 진단이 많다), 의존 없음. 모듈은 루트에 바로(`calls.py` 가 `from shapes import …`) — M4-18 |
| `java/` | jdtls | Gradle(`java` 플러그인만 — Gradle 에 들어 있어 받지 않는다, 언어 수준 17), 저장소 · 의존 없음, wrapper 판 고정(`gradle/wrapper/gradle-wrapper.properties` — 서버의 Gradle 가져오기가 그 배포판을 받는다. jar · 스크립트는 두지 않는다). 도형은 `Shapes` 의 중첩 형이다(한 파일의 두 번째 최상위 형은 다른 파일에서 이름으로 찾지 못한다 — jdtls 실측) — M4-18 |
| `kotlin/` | kotlin-lsp | Gradle(Kotlin Gradle 플러그인 `kotlin("jvm")` 판 고정 + `mavenCentral()` — 그 플러그인 · kotlin-stdlib 와 그 전이 의존을 받는다 — 해시는 `gradle/verification-metadata.xml`(sha256, Gradle 의존 검증)이 고정한다. 플러그인 판을 올리면 픽스처 복사본에서 `gradle --write-verification-metadata sha256 help` 로 다시 만들고, kotlin-lsp 가 가져오기 때 더 받는 소스 jar(`annotations` · `kotlin-stdlib` 의 `-sources.jar` — 없으면 Gradle 이 가져오기를 멈춰 진단 · 완성이 빈다)의 sha256 을 손으로 더한다(Maven Central 의 `.sha1` 과 견준다)). kotlin-lsp 는 Kotlin 플러그인이 없는 빌드(빌드 파일 없음 · `java` 플러그인만)를 분석하지 않는다(hover · 완성 · 진단이 빈다 — 실측, 메인 판정 2026-11-03). wrapper 판은 `java/` 와 같다 — M4-18 |

## 규칙

- **기존 파일 · 앵커를 고치지 않는다.** 기능 행은 자기 이름의 **새 파일**만 더한다 — `rust/src/m4_<행 번호>_<무엇>.rs` · `typescript/src/m4_<행 번호>_<무엇>.ts`. 진단 수 · 사용처 수는 **자기 파일 안**으로만 단언한다(다른 행의 파일이 수를 바꾸지 않게).
- Rust 의 새 파일은 `lib.rs` 의 모듈 목록에 없으므로 크레이트에 들지 않는다. 자기 파일이 크레이트에 들어야 하는 시험은 **임시 복사본**의 `lib.rs` 끝에 `pub mod m4_<행>_<무엇>;` 한 줄을 더한다(저장소의 `lib.rs` 는 그대로 — 행끼리 같은 줄을 다투지 않게).
- 시험은 이 폴더를 **임시 폴더에 복사해** 연다 — 서버가 만드는 `target/` · 색인이 저장소를 더럽히지 않게.
- **rust-src 없이도 돈다**: 앵커는 이 크레이트의 타입 · 함수와 원시 타입(`u32`)에만 건다 — 표준 라이브러리 항목(`String` · `Vec`)의 hover · 정의는 rust-src 가 있어야 하므로 앵커로 쓰지 않는다(계획 리뷰 H2). CI 러너에는 rust-src 가 있지만 로컬 기기에 없어도 같은 결과여야 한다. live 시험 `live_lsp_rust_analyzer_fixture_without_rust_src` 가 rust-src 를 가린 채(rust-analyzer 설정 `cargo.sysrootSrc` 를 빈 폴더로 — 표준 라이브러리 항목의 정의가 풀리지 않는 것으로 가려진 것을 확인한다) 같은 시험을 돈다.
- 저장소의 포매터(oxfmt — `.oxfmtrc.json` 의 `ignorePatterns`)와 `cargo fmt`(작업 공간 밖)는 이 폴더를 보지 않는다 — 흐트러진 서식 파일이 그대로 남는다. oxlint 는 의도된 미사용 import 한 줄만 주석으로 끈다.

## 앵커 표

앵커는 표지 주석 `/*@이름*/` 이다. **표지 바로 뒤 글자가 그 앵커의 위치**다 — 시험은 파일에서 `/*@이름*/` 를 찾아 그 끝의 UTF-16 오프셋을 쓴다(표지 주석 자체는 코드에 영향이 없다). 같은 이름은 모든 언어에서 같은 뜻이다. **Python** 은 식 가운데 주석을 둘 수 없어 줄 끝 주석 `# @이름 "앞글"` 로 쓴다 — 그 줄의 코드(` # @` 앞)에서 처음 나오는 `앞글` 바로 뒤 글자가 위치다(한 주석에 `@이름 "앞글"` 을 여럿 둘 수 있다, `crates/pw-lsp/tests/live_matrix.rs` 의 `anchor`). Java · Kotlin 은 `/*@이름*/` 그대로다.

Python · Java · Kotlin(M4-18 — `live_matrix.rs`)이 쓰는 앵커는 아래 표의 rust · typescript 와 같은 이름이고 파일은 `shapes` · `calls` · `errors` · `messy`(Java 는 `src/main/java/fixture/{Shapes,Calls,Errors,Messy}.java`, Kotlin 은 `src/main/kotlin/fixture/…kt`, Python 은 루트의 `….py`)다: `def.shape` · `def.shape-area` · `impl.rect-shape` · `rename.scale`(정의 — rename 하면 정의 하나와 호출 파일의 호출 셋, Python 은 import 하나 더) · `call.scale-1` · `call.scale-2` · `call.area-1` · `signature.scale` · `complete.rect-members` · `error.type-mismatch`(오류 하나 — 그 진단이 이 자리에서 시작한다: Java 는 `"text"`, Kotlin 은 `=`, Python 은 `"text"`) · `format.messy`. `scale` 의 사용처(선언 포함)는 Python 다섯(정의 · import · 호출 셋), Java · Kotlin 넷(같은 패키지라 import 가 없다). Python 의 `errors.py` 에는 `unused.os`(쓰지 않는 `import os` — ruff F401, 「import 지우기」 빠른 수정)가 더 있다.

| 앵커 | rust | typescript | 무엇 · 기대 |
|---|---|---|---|
| `def.shape` | `src/shapes.rs` | `src/shapes.ts` | 트레이트 · 인터페이스 `Shape` 의 정의 이름 |
| `def.shape-area` | `src/shapes.rs` | `src/shapes.ts` | `Shape::area` 선언 — 구현으로 이동(⌘⌥B)의 출발점 |
| `def.rect` | `src/shapes.rs` | — | 구조체 `Rect` 의 정의 이름 |
| `impl.rect-shape` | `src/shapes.rs` | `src/shapes.ts` | `Rect` 의 `Shape` 구현 자리 — 구현 결과에 든다 |
| `rename.scale` | `src/shapes.rs` | `src/shapes.ts` | 함수 `scale` 의 정의. rename 하면 이 정의와 `calls` 의 import 하나 · 호출 셋(`call.scale-1` · `call.scale-2` · `signature.scale` 앞의 함수 이름)이 바뀐다 — 파일 둘 |
| `call.scale-1` | `src/calls.rs` | `src/calls.ts` | `scale` 호출. 정의로 이동하면 `rename.scale` |
| `call.scale-2` | `src/calls.rs` | `src/calls.ts` | `scale` 의 두 번째 호출. `scale` 의 사용처(선언 포함)는 다섯 — 정의 · import · 호출 셋(live 시험 `…_document_sync` 가 본다) |
| `call.area-1` | `src/calls.rs` | `src/calls.ts` | 메서드 호출 `area()`. hover 에 `area` 의 시그니처 |
| `signature.scale` | `src/calls.rs` | `src/calls.ts` | `scale(` 의 첫 인자 자리 — 시그니처 도움말의 활성 매개변수 0(`width`) |
| `complete.rect-members` | `src/calls.rs` | `src/calls.ts` | `r.` 바로 뒤 — 자동완성 후보에 `area` · `width` · `height` |
| `error.type-mismatch` | `src/errors.rs` | `src/errors.ts` | 타입 오류 하나(rust E0308 · ts TS2322) |
| `error.unresolved` | `src/errors.rs` | `src/errors.ts` | 없는 이름 하나(rust E0425 · ts TS2304) |
| `format.messy` | `src/messy.rs` | `src/messy.ts` | 흐트러진 서식 파일 — 포맷하면 바뀐다(정의 이름 자리) |
| `format.two-space` | — | `src/m4_13_two_space.ts` | 2칸 들여쓰기 · 여러 줄 JSDoc 파일(M4-13) — 웹뷰의 들여쓰기 판정이 2 를 내고(`apps/desktop/src/lsp/format.test.ts`), 공백 2칸으로 포맷하면 그대로다(인터페이스 이름 자리) |
| `unused.rect` | `src/unused.rs` | `src/unused.ts` | 쓰지 않는 import(rust 경고 `unused_imports` · ts TS6133) — 「import 지우기」 코드 액션 |
| `m4_06.type-error` | `src/m4_06_diagnostics.rs` | `src/m4_06_diagnostics.ts` | M4-06 의 타입 오류 하나 — rust 는 `"three"`(일곱 글자, rust-analyzer 의 pull 진단 `source: rust-analyzer` · flycheck 의 E0308), ts 는 이름 `total`(다섯 글자, TS2322). rust 시험은 임시 복사본의 `lib.rs` 에 `pub mod m4_06_diagnostics;` 를 더한다 |
| `rename.m4-12` | `src/m4_12_target.rs` | `src/m4_12_target.ts` | 함수 `relabel` 의 정의(M4-12). rename 하면 정의 하나와 `m4_12_user` 의 import 하나 · 호출 둘이 바뀐다 — 파일 둘, 편집 넷(rust 는 임시 복사본의 `lib.rs` 에 두 모듈을 더한다) |
| `nav.def` | `src/m4_08_nav.rs` | `src/m4_08_nav.ts` | 함수 `greet` 의 정의 이름(M4-08 — 시험이 임시 복사본의 rust `lib.rs` 에 모듈 한 줄을 더한다) |
| `nav.call-1` · `nav.call-2` | `src/m4_08_nav.rs` | `src/m4_08_nav.ts` | `greet` 호출 둘. 정의로 이동하면 `nav.def`, 사용처(선언 포함)는 이 파일 안에 셋 |
| `nav.vec` | `src/m4_08_nav.rs` | — | `Vec::new()` 의 `Vec` — 정의가 표준 라이브러리 소스(루트 밖)라 읽기 토큰으로 연다. rust-src 가 있어야 한다(live 시험 `live_lsp_rust_analyzer_navigation` 은 없으면 「rust-src 없음」 으로 실패한다) |
| `sym.gadget` | `src/m4_16_symbols.rs` | `src/m4_16_symbols.ts` | 구조체 · 클래스 `M416Gadget` 의 정의 이름(M4-16 — `workspace/symbol` 의 결과 자리, 심볼 인덱스의 이름 자리와 같은 줄 · 열이어야 두 출처가 하나로 합쳐진다. rust 는 시험이 임시 복사본의 `lib.rs` 에 모듈 한 줄을 더한다) |
| `sym.spin` | `src/m4_16_symbols.rs` | `src/m4_16_symbols.ts` | 트레이트 · 인터페이스 `M416Spin` — 클래스로 이동(타입 종류)에 든다 |
| `sym.build` | `src/m4_16_symbols.rs` | `src/m4_16_symbols.ts` | 함수 `m416_build_gadget` · `m416BuildGadget` — 클래스로 이동은 뺀다 |
| `sym.decorated` | — | `src/m4_16_decorated.ts` | 데코레이터(여러 줄 — `@m416Component({ … })`)가 붙은 클래스 `M416Panel` 의 이름(M4-16 — 서버는 데코레이터 줄 1열과 선언 전체 범위를 준다, 범위가 이 줄을 담는다) |
| `sym.split` | — | `src/m4_16_decorated.ts` | 이름이 다음 줄인 선언 `export class` / `M416Split` 의 이름(서버는 `class` 줄을 준다) |

`errors` · `unused` 파일은 일부러 오류 · 경고를 낸다 — 다른 파일의 진단을 세는 시험은 이 파일들을 빼고 센다(「자기 파일 안」 규칙).
