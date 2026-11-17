### MCP — 상태바 항목 · 세션 목록 팝업 · 웰컴 창의 연결 수(16-m5-plan M5-10). 키는 apps/desktop/src/mcp/strings.ts 가 화면에 잇는다.
### 「에이전트 활동」 패널(M5-13) · 승인 다이얼로그(M5-14) · 설정 > MCP(M5-15)도 이 파일에 `mcp-…` 묶음으로 더한다.
### MCP 메시지(툴 설명 · 오류 문장)는 번역하지 않는다 — 에이전트가 읽는 영어 계약이다.

## 상태바 항목(10-ui.md 「S4. 프로젝트 창」 그림의 「MCP 2」)

# $count: 이 프로젝트를 세션 프로젝트로 가진 MCP 세션 수(0 이면 항목이 없다)
mcp-status-label = MCP { $count }
# $count: 위와 같다
mcp-status-hint = 이 프로젝트에 연결된 에이전트 { $count }개 — 눌러서 목록 보기

## 세션 목록 팝업(「에이전트 활동」 패널 전까지)

mcp-sessions-title = 연결된 에이전트
mcp-sessions-empty = 이 프로젝트에 연결된 에이전트가 없다
mcp-sessions-unknown-client = 알 수 없는 클라이언트
# $time: 시작 시각(지역 시간)
mcp-sessions-started = { $time } 시작
# $count: 그 세션이 받은 툴 호출 수
mcp-sessions-calls = 호출 { $count }회
mcp-sessions-close = 닫기

## 웰컴 창 아래 줄(10-ui.md 「S1. 웰컴」 그림)

# $count: 이 앱에 열린 MCP 세션 수 전부
mcp-welcome-connections = MCP: 연결 { $count }
