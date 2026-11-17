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
# $count: 목록에 그리지 않은 대기 요청 수(앞 줄만 그린다)
mcp-activity-approvals-more = 그리고 { $count }개 더
# $agent: 요청 에이전트, $tool: 툴 이름 — 줄을 누르면 승인 다이얼로그를 다시 연다
mcp-activity-approvals-open = { $agent }의 { $tool } 승인 다이얼로그 열기
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

## 승인 다이얼로그 · OS 알림(16-m5-plan M5-14)

mcp-approval-title = 에이전트가 승인을 기다린다
mcp-approval-not-this-project = 이 요청은 이 프로젝트의 것이 아니다 — 프로젝트 없는 쓰기라 가장 최근에 앞에 있던 창에 떴다
mcp-approval-agent = 에이전트
mcp-approval-unknown-agent = 이름을 알리지 않은 에이전트
mcp-approval-cli = CLI(pitwall tool)로 온 호출
mcp-approval-tool = 툴
mcp-approval-tier = 등급
mcp-approval-tier-read = 읽기
mcp-approval-tier-write = 쓰기
mcp-approval-tier-exec = 실행
mcp-approval-tier-dangerous = 위험
mcp-approval-summary = 요약
mcp-approval-agent-text = 아래 칸의 글은 에이전트가 보낸 것이다. 보이지 않는 글자는 ⟨U+…⟩ 로 보인다
# $count: 바뀔 것의 수
mcp-approval-changes = 바뀔 것 { $count }개
mcp-approval-no-changes = 보일 변경 목록이 없다 — 요약을 본다
mcp-approval-file-added = 새 파일
mcp-approval-file-deleted = 지우는 파일
mcp-approval-file-modified = 고치는 파일
mcp-approval-unchanged-lines = 바뀌지 않은 줄 $개
# $count: 보이는 앞부분의 글자 수
mcp-approval-text-cut = 글이 길어 앞 { $count }자만 보인다 — 승인은 전부에 닿는다
mcp-approval-invisible = 보이지 않는 글자 포함
mcp-approval-sql-connection = 연결
mcp-approval-sql-file = 파일
mcp-approval-sql-irreversible = 되돌릴 수 없다 — 트랜잭션 밖에서 커밋되는 문장이 있다
# $count: 문장 수
mcp-approval-sql-statements = 문장 { $count }개
# $n: 문장 번호(1 기준), $line: 시작 줄
mcp-approval-sql-statement = 문장 { $n } · { $line }줄
# $count: 접어 보이지 않은 조회 문장 수(조회가 아닌 문장은 늘 모두 보인다)
mcp-approval-sql-folded-reads = 그리고 조회 문장 { $count }개는 접었다 — 조회가 아닌 문장은 모두 보인다
# $time: 남은 시간(분:초)
mcp-approval-remaining = 남은 시간 { $time }
# $index: 이 요청의 차례, $total: 이 창의 대기 수
mcp-approval-position = { $total }개 중 { $index }
mcp-approval-locked = 잠시 뒤 누를 수 있다 — 뜬 순간의 키 · 클릭이 허용이 되지 않게 잠갔다
mcp-approval-deny = 거절
mcp-approval-once = 이번만 허용
mcp-approval-session = 이 세션 동안 허용
mcp-approval-window = 창이 열려 있는 동안 CLI 허용
mcp-approval-notify-title = Pitwall 승인 요청
# $agent: 요청 에이전트(이름을 알리지 않았으면 「에이전트」), $tool: 툴 이름
mcp-approval-notify-body = { $agent }가 { $tool } 승인을 기다린다
mcp-approval-notify-agent = 에이전트

## 설정 > MCP (16-m5-plan M5-15)

mcp-settings-title = MCP
mcp-settings-intro = 전역 설정(settings.toml)의 [mcp] 를 고친다. 프로젝트 설정은 여기서 고치지 않는다 — 그 파일을 연다.
mcp-settings-loading = 읽는 중…
# $error: Host 의 오류 글
mcp-settings-failed = MCP 설정을 읽지 못했다: { $error }
mcp-settings-open-file = 파일로 열기
# $path: 설정 파일
mcp-settings-open-failed = { $path } 를 열지 못했다
# $warning: Host 가 쓰지 않은 값에 대해 남긴 글(영어 그대로)
mcp-settings-warning = 설정 파일에서 쓰지 않은 값: { $warning }
mcp-settings-preset = 프리셋
mcp-settings-preset-read-only = read-only — 조회 툴만
mcp-settings-preset-standard = standard — 조회 · 쓰기 툴
mcp-settings-preset-full = full — 실행 · DB 쓰기까지 모든 툴
mcp-settings-auto-approve = 묻지 않고 허용
mcp-settings-auto-approve-note = 위험 등급은 늘 묻는다. 다른 머신을 겨누는 호출은 자동 허용하지 않는다.
mcp-settings-retention = 에이전트 활동 기록 보관
mcp-settings-retention-unit = 일
# $days: 기본 일수
mcp-settings-retention-hint = 0 은 지우지 않음 · 기본 { $days }일
mcp-settings-apply = 적용
mcp-settings-reset = 기본값으로
mcp-settings-groups = 툴 그룹
mcp-settings-group-default = 프리셋을 따름
mcp-settings-group-on = 켬
mcp-settings-group-off = 끔
mcp-settings-tools = 툴
mcp-settings-search = 툴 검색
# $count: 보이는 툴 수
mcp-settings-tools-count = 툴 { $count }개
mcp-settings-no-tools = 맞는 툴이 없다.
mcp-settings-column-on = 켬
mcp-settings-column-name = 이름
mcp-settings-column-tier = 등급
mcp-settings-column-group = 그룹
mcp-settings-column-calls = 호출(30일)
mcp-settings-column-why = 까닭
mcp-settings-schema-show = 입력 스키마
mcp-settings-schema-hide = 스키마 접기
# $key: 프리셋 이름
mcp-settings-rule-preset = 프리셋 { $key }
mcp-settings-rule-tool = 이 툴에 정한 값
# $key: glob
mcp-settings-rule-glob = 패턴 { $key }
# $key: 그룹 이름
mcp-settings-rule-group = 그룹 { $key }
mcp-settings-rule-shared = 공유 프로젝트 설정이 껐다
mcp-settings-project = 프로젝트 덮어쓰기 보기
mcp-settings-project-none = 없음
mcp-settings-project-files = 프로젝트 설정 파일
mcp-settings-file-shared = .pitwall/project.toml (공유 — 좁히기만 한다)
mcp-settings-file-local = .pitwall/project.local.toml (개인)
mcp-settings-file-personal = project.toml (개인 전용 프로젝트)
mcp-settings-file-missing = 없음
mcp-settings-project-overrides = 이 프로젝트가 [mcp] 아래에 적은 값
mcp-settings-project-no-overrides = 이 프로젝트는 [mcp] 아래에 적은 것이 없다.
# $file: 어느 프로젝트 파일
mcp-settings-override-from = ({ $file })
mcp-settings-project-auto-approve = 이 프로젝트에서 묻지 않고 허용
mcp-settings-project-auto-approve-none = 없음
# $state: 켬 · 끔, $why: 정한 규칙
mcp-settings-overridden = 이 프로젝트: { $state } — { $why }
mcp-settings-overridden-badge = 프로젝트가 덮음
mcp-settings-state-on = 켬
mcp-settings-state-off = 끔
mcp-settings-register = 에이전트에 등록
mcp-settings-register-intro = 아래 명령을 실행하거나 에이전트 설정에 넣는다. Pitwall 은 에이전트의 설정을 고치지 않는다. pitwall 명령이 PATH 에 있어야 한다(Shell 명령 설치).
mcp-settings-register-claude-code = Claude Code (모든 프로젝트)
mcp-settings-register-codex = Codex
mcp-settings-register-cursor = Cursor (~/.cursor/mcp.json)
mcp-settings-copy = 복사
mcp-settings-copied = 복사했다
# $error: 복사하지 못한 까닭
mcp-settings-copy-failed = 복사하지 못했다: { $error }
mcp-settings-refused-unknown-preset = 프리셋이 아니다.
mcp-settings-refused-dangerous = 위험 등급은 늘 묻는다 — 묻지 않고 허용할 수 없다.
mcp-settings-refused-unknown-tier = 등급이 아니다.
mcp-settings-refused-unknown-group = 툴 그룹이 아니다.
mcp-settings-refused-unknown-tool = 이 앱의 툴이 아니다.
mcp-settings-refused-retention = 0 이상의 정수 일수를 넣는다.
mcp-settings-refused-bad-value = Host 가 이 값을 받지 않았다.
