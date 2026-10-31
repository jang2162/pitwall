### DB 안전장치 화면 부품(apps/desktop/src/db/safety, 14-m3-plan.md M3-19): 연결 표지 · 위험 쿼리 확인 · 거절 문장 · TLS 표지 ·
### 세션 알림 · 상태바 `db` 항목.

## 연결 표지 — 색만으로 가르지 않게 라벨을 함께 둔다(10-ui.md 「접근성」).

db-label-production = 프로덕션
db-label-read-only = 읽기 전용

## 연결 결과의 TLS 상태(`ServerInfo.tls`).

db-tls-unverified = 서버 인증서를 검증하지 않음
db-tls-unverified-hint = TLS 로 붙었지만 서버 인증서나 이름을 확인하지 않았다 — 가로챈 서버와 구별하지 못한다. 연결 설정의 sslmode 를 verify-full 로 바꾸면 확인한다
db-tls-plaintext = 평문
db-tls-plaintext-hint = TLS 없이 붙었다 — 비밀번호 · 쿼리 · 결과가 암호화되지 않은 채 네트워크를 지난다

## 위험 등급(`RiskGrade`).

db-grade-read = 조회
db-grade-write = 쓰기
db-grade-unbounded-write = 범위 없는 쓰기
db-grade-ddl = 스키마 변경
db-grade-destructive = 파괴

## 위험 쿼리 확인 다이얼로그의 제목(가장 높은 등급).

db-confirm-title-read = 실행하기 전에 확인한다
db-confirm-title-write = 데이터를 바꾸는 문장이다
db-confirm-title-unbounded-write = 범위 없이 여러 행을 바꿀 수 있다
db-confirm-title-ddl = 스키마를 바꾸는 문장이다
db-confirm-title-destructive = 되돌릴 수 없는 문장이다
db-confirm-title-unread = 분석하지 못한 문장이 있다 — 무엇을 하는지 몰라 되돌릴 수 없는 것으로 다룬다

## 위험 쿼리 확인 다이얼로그.

# $grade: 등급 이름
db-confirm-grade = 등급: { $grade }
# $connection: 연결 이름
db-confirm-connection = 연결: { $connection }
# $connection: 연결 이름
db-confirm-production = { $connection } 은 프로덕션 연결이다 — 조회가 아닌 문장은 모두 확인을 받는다
# $connection: 연결 이름
db-confirm-name-prompt = 실행하려면 연결 이름 { $connection } 을 그대로 친다
db-confirm-name = 연결 이름
db-confirm-name-mismatch = 연결 이름과 다르다
# $count: 문장 수
db-confirm-statements = { $count ->
        [one] 문장 하나
       *[other] 문장 { $count }개
    }
# $count: 보이지 않은 문장 수
db-confirm-more = …그리고 { $count }개 더
db-confirm-unparsed = 분석하지 못한 문장
# $index: 문장 번호(1부터)
db-confirm-statement = { $index }번 문장
db-confirm-cancel = 취소
db-confirm-run = 실행
db-confirm-rejected-hash-mismatch = 확인한 뒤 실행할 문장이 바뀌어 다시 묻는다
db-confirm-rejected-grade-too-low = 확인한 것보다 위험한 문장이 있어 다시 묻는다
db-confirm-rejected-name-mismatch = 친 연결 이름이 달라 다시 묻는다
db-confirm-rejected-other = 앞의 확인을 받지 않아 다시 묻는다

## 트랜잭션 효과(`DbTxEffect`)와 취소 확인(3002).

db-effect-implicit-commit = 이 문장은 열린 트랜잭션을 먼저 커밋한다 — 실패해도 앞의 변경은 커밋된다
db-cancel-rollback-title = 취소하면 트랜잭션 전체가 롤백된다
db-cancel-rollback-body = 이 쓰기를 멈추면 DB 가 열린 트랜잭션 전체를 되돌린다 — 이 탭에서 커밋하지 않은 변경이 모두 사라진다
db-cancel-rollback-confirm = 멈추고 롤백

## 까닭(`RiskReason`) — 문장마다 등급 옆에 보인다.

db-reason-unparsed = 분석하지 못한 문장이다 — 무엇을 하는지 알 수 없어 파괴로 본다
db-reason-too-large = 너무 커서 분석하지 않았다 — 파괴로 본다
db-reason-too-deep = 중첩이 너무 깊어 분석하지 않았다 — 파괴로 본다
db-reason-drop = 객체나 열 · 파티션을 지운다
db-reason-truncate = 표의 모든 행을 지운다
db-reason-copy-program = DB 서버에서 셸 명령을 돌린다
db-reason-ddl = 스키마나 권한을 만들거나 바꾼다
db-reason-missing-where = WHERE 가 없어 모든 행이 바뀐다
db-reason-unbounded-where = WHERE 가 범위를 한정하지 않는다(항상 참인 조건 · 서브쿼리)
db-reason-multi-table = 조인이나 여러 표에 걸친 변경이라 범위를 보장하지 않는다
db-reason-or-replace = 충돌한 다른 행을 지울 수 있다(OR REPLACE)
db-reason-merge = MERGE 는 여러 행을 넣고 바꾸고 지울 수 있다
db-reason-dml = 행을 넣거나 바꾸거나 지운다
db-reason-locking-read = 읽은 행을 잠근다(FOR UPDATE · FOR SHARE)
db-reason-side-effect-function = 부작용이 있는 함수를 부른다
db-reason-server-admin = 다른 세션이나 서버 전체에 닿는 관리 작업이다
db-reason-replication-reset = 바이너리 · 릴레이 로그와 복제 위치를 지운다 — 되돌릴 수 없다
db-reason-select-into = 조회 결과를 변수 · 파일 · 새 표에 쓴다
db-reason-transaction-control = 트랜잭션을 열거나 끝낸다
db-reason-session-state = 세션 설정을 바꾼다
db-reason-lock = 표를 잠그거나 잠금을 푼다
db-reason-procedural = 프로시저나 준비한 문장을 실행한다 — 안에서 무엇을 하는지 알 수 없다
db-reason-attach = 다른 DB 파일을 붙이거나 뗀다
db-reason-pragma = SQLite 설정(PRAGMA)을 읽거나 바꾼다
db-reason-maintenance = 표를 정리하거나 분석한다(VACUUM · ANALYZE · OPTIMIZE)
db-reason-data-transfer = 파일이나 외부에서 데이터를 들이거나 내보낸다
db-reason-other = 분류하지 못한 종류의 문장이라 쓰기로 본다

## 거절 문장 — Host 가 돌리지 않은 까닭(`data.reason`).

db-refusal-read-only-connection = 읽기 전용 연결이라 조회만 돌린다 — 이 문장은 돌리지 않았다
db-refusal-untrusted = 이 프로젝트를 신뢰하지 않아 DB 에 연결하지 않는다 — 프로젝트를 신뢰한 뒤 다시 연결한다
db-refusal-not-project-window = DB 연결은 그 프로젝트의 창에서만 쓴다
db-refusal-not-a-query = 조회 문장 하나만 돌린다 — 이 문장은 돌리지 않았다
# $message: Host 가 준 까닭
db-refusal-transaction-statement = 이 트랜잭션 문장은 돌릴 수 없다: { $message }
db-refusal-busy = 이 탭에서 다른 실행이 아직 돈다
db-refusal-config-changed = 연결 설정이 바뀌었다 — 다시 연결해야 새 문장을 돌린다
db-refusal-transaction-open = 열린 트랜잭션이 있어 자동 커밋으로 바꾸지 않는다 — 먼저 커밋하거나 롤백한다

## 세션 알림(`DbSessionNotice`).

# $message: 서버 · 드라이버의 오류
db-notice-rolled-back = 트랜잭션이 롤백됐다: { $message }
db-notice-committed-implicitly = 이 문장이 열린 트랜잭션을 커밋했다(암묵 커밋) — 문장이 실패했어도 앞의 변경은 커밋됐다
db-notice-transaction-ended = 트랜잭션이 끝났다. 커밋됐는지 롤백됐는지 서버가 알리지 않았다 — 다른 연결로 확인하라
# $message: 그 단계의 오류
db-notice-transaction-ended-message = 트랜잭션이 끝났다. 커밋됐는지 롤백됐는지 서버가 알리지 않았다 — 다른 연결로 확인하라 ({ $message })
# $message: 묻지 못한 까닭
db-notice-transaction-unknown = 트랜잭션 상태를 서버에 묻지 못했다 — 열려 있는지 끝났는지 모른다 ({ $message })
db-notice-transaction-opened = 이 탭이 열지 않은 트랜잭션이 열렸다 — 커밋하거나 롤백할 때까지 이어진다
db-notice-connection-lost = 연결이 끊겼다
db-notice-connection-lost-rolled-back = 연결이 끊겼다 — 열린 트랜잭션은 DB 가 롤백했다
db-notice-connection-lost-outcome-unknown = 커밋 도중 끊겨 결과를 모른다 — 다른 연결로 확인하라
db-notice-config-changed = 연결 설정이 바뀌었다 — 다시 연결해야 새 문장을 돌린다
# $until: 보존이 끝나는 시각
db-notice-unattended = 창이 닫혀 열린 트랜잭션을 { $until } 까지 보존한다 — 그때까지 이 탭이 다시 붙지 않으면 롤백한다
db-notice-unattended-rolled-back = 아무도 다시 붙지 않아 열린 트랜잭션을 롤백했다

## 상태바 `db` 항목(지금 탭의 세션).

db-status-mode-auto = 자동 커밋
db-status-mode-manual = 수동 커밋
# $elapsed: 트랜잭션이 열린 뒤 지난 시간(분:초)
db-status-transaction = TX { $elapsed }
# $elapsed: 트랜잭션이 열린 뒤 지난 시간(분:초)
db-status-failed = TX 실패 { $elapsed } — 롤백만 된다
db-status-disconnected = 연결 끊김
db-status-config-changed = 설정 바뀜 — 다시 연결
# $connection: 연결 이름
# $mode: 커밋 방식
db-status-hint = { $connection } · { $mode }

## DB 접근 묻기(apps/desktop/src/db/safety/access-prompt.tsx, 14-m3-plan.md M3-13 — 3101 비밀 필요 · 3102 확인 필요).

db-access-cancel = 취소
# $connection: 연결 이름
db-access-secret-title = { $connection } 의 비밀번호
db-access-secret-missing = 저장한 비밀번호가 없다
db-access-secret-target-changed = 비밀번호를 저장할 때와 연결 대상이 다르다 — 그 비밀번호를 보내지 않았다. 이 대상으로 보낼 비밀번호를 다시 넣는다
db-access-secret-unavailable = 키체인을 쓸 수 없다(잠김 · 서비스 없음)
db-access-secret-label = 비밀번호
db-access-secret-save = 저장하고 연결
db-access-approve-title-env = 환경 변수의 값을 이 연결에 보낸다
db-access-approve-title-file = 프로젝트 밖의 SQLite 파일을 연다
db-access-approve-title-tunnel = SSH 터널을 연다
db-access-approve-title-cert-files = 인증서 파일을 읽어 TLS 에 쓴다
db-access-approve-hint = 커밋된 연결 설정이 정한 것이다. 확인한 것이 바뀌면 다시 묻는다
db-access-approve = 확인하고 연결
db-access-previous = 전에 확인한 것
db-access-now = 지금
db-access-variable = 환경 변수
db-access-path = 파일
db-access-destination = SSH 목적지
db-access-driver = 드라이버
db-access-host = 호스트
db-access-port = 포트
db-access-user = 사용자
db-access-tunnel = 터널
db-access-sslmode = sslmode
db-access-sslrootcert = 루트 인증서
db-access-sslcert = 클라이언트 인증서
db-access-sslkey = 클라이언트 키
db-access-none = (없음)
# $error: 오류 글
db-access-failed = 하지 못했다: { $error }
