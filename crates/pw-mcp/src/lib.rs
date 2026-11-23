//! Pitwall MCP 서버(`docs/design/05-mcp.md`).
//!
//! - [`catalog`]: `rpc_method!` 선언 중 `mcp` 를 단 것에서 만든 툴 목록(이름 · 그룹 · 설명 · 등급 · 입출력 스키마).
//! - [`exposure`]: 툴 노출 설정(`[mcp]` 프리셋 · `[mcp.tools]` · `[mcp.groups]` on/off, 전역 → 프로젝트).
//! - [`code`]: 이 크레이트의 애플리케이션 오류 코드(6001~).
//! - [`policy`]: 권한 등급별 정책 · `auto_approve` · `[mcp.policy]` 읽기.
//! - [`approval`]: 승인 요청의 라우팅(창의 `mcp.approvals` · `mcp.approval.answer`). 계획 · 해시의 꼴은
//!   `pw_core::registry::plan`(툴 크레이트가 이 크레이트에 의존하지 않는다).
//! - [`audit`]: 감사 로그(`$STATE/mcp/audit.db`)와 인자 요약 규칙.
//! - [`server`]: MCP 세션 하나(rmcp). 툴 호출을 Host 의 RPC 로 옮긴다.
//! - [`checkout`]: 다른 머신을 겨누는 호출 — `checkout` 입력 · 묶음 풀기 · 머신마다의 연결 풀(18-m7-plan M7-18).
//! - [`sessions`]: 열린 세션의 등록부와 상태 이벤트(`mcp.sessionsChanged` · `mcp.activity`).
//! - [`methods`]: 사람의 화면이 부르는 `mcp.*` RPC(세션 목록 · 툴 목록 · 감사 로그). 에이전트에게는 열지 않는다.
//! - [`marker`]: `tools/call` 결과의 `_meta` 에 싣는 결과 표지와 까닭 코드.
//! - [`socket`]: 실행 중인 앱의 로컬 소켓(unix). 위치 · 권한 · 워크트리 격리 · 핸드셰이크.
//! - [`http`]: Streamable HTTP(`http://127.0.0.1:<port>/mcp` — 18-m7-plan M7-25). 기본 꺼짐 · 루프백만 · Bearer 토큰 · `Origin` ·
//!   `Host` 검사. HTTP 세션 하나가 [`server::serve`] 세션 하나다 — 판정은 소켓 · stdio 와 같은 `Session` 이다.
//! - [`proxy`]: `pitwall mcp` 의 stdio ↔ 소켓 프록시.
//! - [`cli`]: `pitwall tool` 의 몸통 — Desktop 과 Relay(`pitwall-relay tool`)가 함께 쓰는 MCP 클라이언트 CLI.
//!
//! MCP 메시지(툴 설명 · 오류 문장)는 번역하지 않는다 — 에이전트가 읽는 계약이다.
//!
//! # 툴 메서드 규약
//!
//! 툴은 RPC 선언의 `mcp` 칸에서 나온다(결정 M5-D26 · M5-D23, `docs/design/16-m5-plan.md`).
//!
//! - **05 의 툴과 이름 · 등급 · 입력 꼴이 같은 RPC** 에는 `mcp` 를 그대로 단다. 이름이 규칙 이름(`project.list` → `project_list`)과
//!   같고 그룹이 첫 마디(`openWith.list` → `open_with`)면 `mcp: true`, 아니면 `mcp: { group: "project", name: "settings_get" }`.
//! - **다른 것**(UI 의 RPC 가 화면에 맞춘 꼴 · 등급이거나, 한 툴이 RPC 여럿을 묶는 것)은 그 기능 크레이트에 **툴 메서드**를 따로
//!   선언한다 — UI 의 RPC 와 같은 내부 함수 · 같은 관문을 부르는 얇은 선언이다. 관문을 따로 만들지 않는다.
//! - 그룹은 [`catalog::GROUPS`] 가운데 하나다(05 「툴 노출 설정」). 이름은 `[a-z0-9_]` 64자, 겹치면 안 된다 — 둘 다 시작 때
//!   [`Catalog::linked`] 가 본다.
//! - **입력은 struct** 다(`()` 도 되지만 뒤에 입력을 더하면 깨진다). 프로젝트 툴은 `projectId` 와 **선택 입력 `worktree`**
//!   (`Option<String>`, 결정 M5-D15 — 뒤에 `checkout` 을 깨지 않고 더할 자리)를 받는다. 체크아웃을 보지 않고 프로젝트 레코드만
//!   읽는 툴(`project_info`)은 `worktree` 를 달지 않는다 — 받기만 하고 쓰지 않는 입력은 에이전트를 속인다. 그런 툴은 데스크톱
//!   크레이트 툴 목록 시험의 `PROJECT_RECORD_TOOLS` 에 까닭과 함께 적는다. 입력이 struct 인 것은 예외가 없다.
//! - **쪽 단위 툴**은 선택 입력 `cursor` · `limit` 을 둘 다 받고, 결과(object)에 다음 쪽을 알리는 `nextCursor`(없으면 `null` —
//!   마지막 쪽)를 싣는다. `limit` 이 비면 `pw_rpc::DEFAULT_LIST_LIMIT`(100 — 툴마다 다르면 그 툴의 값, 결정 M5-D34). 어느 툴이 쪽 단위인지는 그 툴 행이 정한다 — 넘치는 응답은 세션의 응답 상한이 막는다.
//! - **등급은 그 툴이 도달할 수 있는 가장 높은 등급**이다(`pw_rpc::Tier` 의 규칙 그대로). 입력에 따라 등급이 오르면 툴을 둘로
//!   나눈다(`db_query` read · `db_execute` dangerous — 결정 M5-D4).
//! - **계획**(결정 M5-D30 — 16-m5-plan M5-12): `write` 이상 툴(과 호출마다 승인을 청하는 `read` 툴)은 입력을
//!   `pw_core::registry::plan::Planned<P>` 로 감싸고, 부작용 전에 [`ToolPlan`](pw_core::registry::plan::ToolPlan) 을 만들어
//!   `Planned::approve` 를 지난다. `approve` 뒤의 부작용은 계획을 만들 때 읽은 값(해시에 넣은 값)으로 한다 — 다시 읽지 않는다.
//!   계획의 `method` 는 그 툴의 RPC 이름, `tier` 는 선언 등급 이하여야 한다(세션이 보고 어기면 돌리지 않는다).
//! - **선언 등급과 계획 등급**(17-m6-plan 결정 M6-P9): 선언 등급은 **상한**이다 — 노출 · 프리셋 · 헤드리스 거절 · `readOnly`
//!   표지는 선언 등급으로 한다. 승인 판정(`auto_approve` · 세션 허용 · 매번 묻기)은 **계획 등급**으로 한다. 한 툴이 입력에 따라
//!   등급이 갈리면(`run_start` 의 프로세스 구성 `exec` · `sql` 구성의 조회만 `read` · 그 밖 `dangerous`) 선언은 가장 높은 등급으로
//!   두고 호출마다 계획 등급을 낸다. 계획 등급을 선언보다 **낮게** 내는 툴은 그 등급을 [`catalog::LOWERED_PLAN_TIERS`] 에 까닭과
//!   함께 적는다 — 세션은 목록에 없는 낮춤을 툴의 버그로 거절한다(런타임 강제). 데스크톱 크레이트의 회귀 표(`mcp_tests` 의
//!   `plan_tiers`)에도 (선언 등급, 계획 등급)을 적는다 — 표에 없는 계획 툴 · 목록과 다른 낮춤은 시험이 실패한다. 세션 허용 ·
//!   CLI 허용은 그 계획 등급에만 닿는다(낮은 계획에 준 허용이 같은 툴의 높은 계획에 닿지 않는다). 선언을 넘는 등급이 필요한 갈래(조건
//!   있는 브레이크포인트 등)는 툴을 둘로 나눈다(17-m6-plan M6-20).
//! - **정책 자동**(17-m6-plan M6-10 ③): 툴이 05 「권한 모델」 의 규칙(`exec` 의 「신뢰 + 사람이 본 구성」, `sql` 구성의 조회만)으로
//!   묻지 않아도 된다고 판정하면 계획 규칙에 `policy_auto: true` 를 단다. 세션은 묻지 않고 승인 표를 실어 다시 부르며 감사 행의
//!   승인 방식은 `policy` 다. 판정은 툴의 일이다 — 세션은 표지를 읽기만 한다. `dangerous` 계획에는 달 수 없다(세션이 툴의 버그로
//!   거절). 사람이 띄운 실행처럼 `auto_approve` 도 닿으면 안 되는 호출은 규칙의 `auto_approve: false` 로 막는다(결정 D69).
//! - **에이전트의 실행**(17-m6-plan M6-10 ① — 결정 M6-P10): 프로세스를 띄우는 툴은 계획을 만들 때 푼 실행 내용
//!   (`pw_core::registry::exec::RunSpec`)을 `PlanBuilder::runs` 로 계획에 싣고, `approve` 가 준 `Approved` 로
//!   `pw_core::registry::exec::agent_run_grant` 를 받아 그 허가의 `RunSpec` 만 띄운다 — 구성 · `.env` 를 다시 읽지 않는다.
//!   `exec_grant` 는 MCP 세션을 늘 거절한다(셸 · 외부 도구는 띄울 길이 없다).
//!   승인 표 없이 불리면 **아무것도 바꾸지 않고** 계획을 실은 「승인 필요」 로 돌아온다 — 입력이 틀려
//!   계획을 만들 수 없으면 그 오류를 그대로 낸다(승인을 묻기 전에 거절). 계획에는 서버가 채운 입력 전체와 툴이 더하는 대상 값
//!   (`PlanBuilder::target` — 고치기 전 파일 내용, DB 의 푼 대상 · 표지)을 넣는다. 해시 판정을 따로 두지 않는다. `write` 이상인데
//!   입력이 `Planned` 가 아니면 [`Catalog::linked`] 가 시작 때 실패한다.
//! - **감사 상세**: 감사 행의 인자 요약은 글자열을 80자에서 자르므로 툴이 남길 것이 따로 있으면 결과(object)의 최상위
//!   [`server::AUDIT`](`audit`) 칸에 글로 싣는다 — 세션이 그 글을 감사 행의 상세 칸으로 옮긴다(오류 · 거절이면 오류 `data`
//!   (object)의 최상위 `audit` 을 「오류 글 · 상세」 로, 그때는 `data` 를 통째로 싣지 않는다). 길이는 감사의 상세 상한
//!   ([`audit::MAX_DETAIL`])에서 잘린다. 결과에 남으므로 **에이전트에게도 보인다** — 툴 설명에 그렇다고 적고, 원문 · 값 · 비밀을
//!   넣지 않는다(수 · 등급 · 해시 · 경로만). 칸이 없는 툴은 성공 행의 상세가 비고 오류 행은 오류 글 그대로다(`db_query` 가 처음 쓴다
//!   — 16-m5-plan.md M5-07 ⑦).
//! - 툴 설명은 선언의 문서 주석이다 — 화면 문자열처럼 번역하지 않는다.
//! - 툴 메서드도 `rpc_method!` 선언이라 TS 생성물(`packages/rpc-client`)에 들어간다 — 웹뷰가 부르지 않는 메서드가 클라이언트
//!   타입에 보인다. 앱의 Host 에 핸들러를 붙이고(`generated_rpc_covers_the_router`), `mise run gen` 으로 생성물을 다시 만든다.
//! - 툴을 더하면 데스크톱 크레이트의 툴 목록(`apps/desktop/src-tauri/src/mcp_tests.rs` 의 `APP_TOOLS` · 코어 툴이면
//!   `HEADLESS_TOOLS`)에 이름을 더하고, 그 파일의 툴 왕복 도우미(`ToolHost`)로 진짜 세션에서 본다.

pub mod approval;
pub mod audit;
pub mod catalog;
pub mod checkout;
pub mod cli;
mod client;
pub mod code;
mod connections;
pub mod exposure;
pub mod http;
pub mod marker;
pub mod methods;
pub mod policy;
pub mod proxy;
pub mod server;
pub mod session_tools;
pub mod sessions;
pub mod settings;
#[cfg(unix)]
pub mod socket;

pub use catalog::{Catalog, Tool};
pub use client::{CallError, HostClient, Settled};
pub use connections::Connections;
pub use server::{HostKind, ServeError, Services, SessionRequest, serve};
