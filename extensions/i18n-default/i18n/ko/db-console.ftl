### SQL 콘솔(M3-15, apps/desktop/src/db/console): 위 막대 · 결과 · 실행 계획 · 열린 트랜잭션 확인 · 액션.

db-console-title = SQL 콘솔
# $connection: 연결 이름
db-console-tab-title = { $connection } 콘솔

## 위 막대

db-console-connection = 연결
db-console-no-connection = 연결 고르기
db-console-database = DB
db-console-database-default = (설정의 DB)
db-console-schema = 스키마
db-console-schema-default = (서버 기본)
db-console-connect = 연결
db-console-disconnect = 연결 끊기
db-console-run = 실행
db-console-run-hint = 선택이 있으면 선택한 문장들, 없으면 커서가 있는 문장을 실행한다
db-console-cancel = 취소
db-console-cancel-hint = 도는 쿼리를 서버에서 멈춘다
db-console-explain = 실행 계획
db-console-mode = TX
db-console-commit = 커밋
db-console-rollback = 롤백
db-console-not-connected = 연결하지 않았다 — [연결] 을 누르면 세션을 연다.
db-console-failed = 오류 뒤 롤백만 되는 트랜잭션이다 — [롤백] 한다.
db-console-disconnected = 연결이 끊겼다 — 다시 붙으려면 [연결] 을 누른다.
db-console-config-changed = 연결 설정이 바뀌었다 — 커밋 · 롤백한 뒤 다시 연결한다.
db-console-choose-locked = 트랜잭션이 열려 있는 동안은 연결 · DB · 스키마를 바꾸지 않는다.

## 결과

db-console-results = 쿼리 결과
# $index: 문장 차례(1부터)
db-console-result-tab = 결과 { $index }
db-console-plan = 실행 계획
db-console-results-empty = 실행한 문장의 결과가 여기 보인다.
db-console-plan-cost = 비용
db-console-plan-rows = 행
db-console-plan-empty = 계획이 비었다.
db-console-resize = 콘솔과 결과의 경계

## 열린 트랜잭션 확인(탭 닫기 · 창 닫기 · 앱 종료)

db-console-close-title = 열린 트랜잭션이 있다
db-console-close-failed-title = 실패한 트랜잭션이 열려 있다
db-console-close-body = 이 콘솔의 수동 트랜잭션이 아직 끝나지 않았다. 닫기 전에 커밋하거나 롤백한다.
db-console-close-failed-body = 오류 뒤 롤백만 되는 트랜잭션이다. 닫기 전에 롤백한다.
db-console-close-cancel = 취소
db-console-close-rollback = 롤백
db-console-close-commit = 커밋

## 액션

db-console-action-open = 새 SQL 콘솔
db-console-action-run = SQL 실행
db-console-action-run-all = 콘솔 전체 실행
db-console-action-cancel = 쿼리 취소
db-console-action-explain = 실행 계획 보기

## 오류 · 알림

# $message: 까닭
db-console-load-failed = 콘솔 글을 읽지 못했다: { $message }
# $message: 까닭
db-console-save-failed = 콘솔 글을 저장하지 못했다: { $message }
# $message: Host 가 준 오류
db-console-error = { $message }
db-console-nothing-to-run = 실행할 문장이 없다.
db-console-explain-one = 실행 계획은 문장 하나씩 본다 — 문장 하나에 커서를 둔다.
db-console-unattended-title = 보는 사람이 없어 롤백한 트랜잭션
# $connection: 연결 이름, $since: 트랜잭션을 연 시각, $minutes: 보존한 시간(분)
db-console-unattended-body = { $connection } — { $since }에 연 트랜잭션을 { $minutes }분 기다린 뒤 롤백했다.
# $connection: 연결 이름
db-console-unattended-body-unknown = { $connection } — 창이 닫혀 열린 트랜잭션을 롤백했다.
