### MCP — 상태바 항목 · 웰컴 창의 연결 수(16-m5-plan M5-10)와 「에이전트 활동」 툴 윈도우(M5-13). 키는 apps/desktop/src/mcp/strings.ts 가 화면에 잇는다.
### 승인 다이얼로그(M5-14) · 설정 > MCP(M5-15)도 이 파일에 `mcp-…` 묶음으로 더한다.
### MCP 메시지(툴 설명 · 오류 문장)는 번역하지 않는다 — 에이전트가 읽는 영어 계약이다.

## 상태바 항목(10-ui.md 「S4. 프로젝트 창」 그림의 「MCP 2」)

# $count: 이 프로젝트를 세션 프로젝트로 가진 MCP 세션 수(0 이면 항목이 없다)
mcp-status-label = MCP { $count }
# $count: 위와 같다
mcp-status-hint = 이 프로젝트에 연결된 에이전트 { $count }개 — 눌러서 「에이전트 활동」 열기

## 「에이전트 활동」 패널의 「연결된 에이전트」 칸(M5-13 — M5-10 의 세션 목록 팝업에서 옮겼다)

mcp-sessions-title = 연결된 에이전트
mcp-sessions-empty = 이 프로젝트에 연결된 에이전트가 없다
mcp-sessions-unknown-client = 알 수 없는 클라이언트
# $time: 시작 시각(지역 시간)
mcp-sessions-started = { $time } 시작
# $count: 그 세션이 받은 툴 호출 수
mcp-sessions-calls = 호출 { $count }회

## 웰컴 창 아래 줄(10-ui.md 「S1. 웰컴」 그림)

# $count: 이 앱에 열린 MCP 세션 수 전부
mcp-welcome-connections = MCP: 연결 { $count }

## 「에이전트 활동」 툴 윈도우(16-m5-plan M5-13)

mcp-activity-approvals-title = 승인 대기
mcp-activity-approvals-empty = 기다리는 승인 요청이 없다
mcp-activity-log-title = 호출 로그
# 감사 로그는 이 프로젝트의 행과 프로젝트 없는 행을 함께 보인다(설정 > MCP 의 툴별 「최근 호출 수」 는 이 프로젝트의 행만 센다)
mcp-activity-log-scope = 이 프로젝트의 호출과 프로젝트 없는 호출
mcp-activity-column-time = 시각
mcp-activity-column-client = 클라이언트
mcp-activity-column-tool = 툴
mcp-activity-column-args = 인자
mcp-activity-column-outcome = 결과
mcp-activity-column-duration = 소요
mcp-activity-column-approval = 승인
mcp-activity-outcome-ok = 성공
mcp-activity-outcome-error = 실패
mcp-activity-outcome-denied = 거절
# 실행 전에 쓴 행의 끝이 아직 없다 — 도는 중이거나 끝을 쓰기 전에 프로세스가 죽었다
mcp-activity-outcome-started = 끝을 모름
mcp-activity-approval-auto = 자동
mcp-activity-approval-once = 이번만
mcp-activity-approval-session = 세션 동안
mcp-activity-approval-window = 창이 열린 동안
# 승인을 지나지 않은 호출(read · 거절)
mcp-activity-approval-none = 없음
# 프로젝트 없는 호출의 표지
mcp-activity-no-project = 프로젝트 없음
# $ms: 걸린 시간(밀리초)
mcp-activity-duration = { $ms } ms
mcp-activity-filter-outcome = 결과 거르기
mcp-activity-filter-tool = 툴 거르기
mcp-activity-filter-client = 클라이언트 거르기
mcp-activity-filter-all-outcomes = 모든 결과
mcp-activity-filter-all-tools = 모든 툴
mcp-activity-filter-all-clients = 모든 클라이언트
mcp-activity-loading = 읽는 중…
mcp-activity-empty = 호출 기록이 없다
mcp-activity-empty-filtered = 거르기에 맞는 호출이 없다
# $message: 오류 문장
mcp-activity-load-failed = 감사 로그를 읽지 못했다: { $message }
mcp-activity-clear = 기록 지우기
mcp-activity-clear-title = 이 프로젝트의 호출 기록을 지울까?
mcp-activity-clear-body = 이 프로젝트의 감사 로그 행을 모두 지운다. 프로젝트 없는 호출의 행은 남는다. 되돌릴 수 없다.
mcp-activity-clear-confirm = 지우기
mcp-activity-clear-cancel = 취소
mcp-activity-clear-failed = 호출 기록을 지우지 못했다
