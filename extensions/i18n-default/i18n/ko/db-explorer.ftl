### DB 탐색기 툴 윈도우(apps/desktop/src/db/explorer, 14-m3-plan.md M3-13).

db-explorer-label = DB 탐색기

## 툴바.

db-explorer-add = 연결 추가
db-explorer-refresh = 새로 고침
db-explorer-find = 찾기
db-explorer-find-placeholder = 테이블 · 뷰 이름(서버에서 찾는다)
db-explorer-find-clear = 찾기 지우기

## 빈 상태 · 목록 상태.

db-explorer-empty = 이 프로젝트에 DB 연결이 없다
db-explorer-untrusted = 이 프로젝트를 신뢰해야 연결할 수 있다 — 지금은 연결 목록만 보인다
# $error: 오류 글
db-explorer-list-failed = 연결 목록을 읽지 못했다: { $error }
# $problem: 설정을 읽지 못한 까닭
db-explorer-config-problem = 프로젝트 설정을 읽지 못했다: { $problem }
# $problem: 확인 기록을 읽지 못한 까닭(파일 경로 포함)
db-explorer-state-problem = 확인 기록을 읽지 못했다 — 그동안은 비밀번호 · 확인을 모두 다시 묻는다. 기록 파일을 고치거나 지운다: { $problem }

## 연결 상태(색이 아니라 글자로도 보인다).

db-explorer-status-idle = 연결 안 됨
db-explorer-status-connecting = 연결 중
db-explorer-status-connected = 연결됨
db-explorer-status-lost = 끊김
db-explorer-status-config-changed = 설정 바뀜 — 다시 연결
db-explorer-status-secret-required = 비밀 필요
db-explorer-status-approval-required = 확인 필요
db-explorer-status-untrusted = 미신뢰
db-explorer-status-invalid = 설정 오류
# $message: 끊긴 까닭
db-explorer-status-lost-hint = 끊겼다: { $message } — 다시 펼치면 연결한다
db-explorer-status-config-changed-hint = 연결 설정이 바뀌었다. 저절로 다시 붙지 않는다 — 펼치면 새 설정으로 연결한다

## 트리의 묶음.

db-explorer-group-tables = 테이블
db-explorer-group-views = 뷰
db-explorer-detail-columns = 열
db-explorer-detail-indexes = 인덱스
db-explorer-detail-foreign-keys = 외래 키
# $filter: 찾는 글
db-explorer-filtered = 「{ $filter }」 가 든 것만

## 상태 줄.

db-explorer-loading = 읽는 중…
db-explorer-more = 더 보기
db-explorer-none = 없음
# $error: 오류 글
db-explorer-failed = 읽지 못했다: { $error }

## 항목.

db-explorer-not-null = NOT NULL
db-explorer-primary-key = 기본 키
db-explorer-unique = 유일
db-explorer-partial = 부분

## 우클릭 메뉴.

db-explorer-menu-new-console = 새 콘솔
db-explorer-menu-ddl = DDL 보기
db-explorer-menu-reveal = DB 탐색기에서 선택
db-explorer-menu-row-count = 행 수
db-explorer-menu-copy-name = 이름 복사
db-explorer-menu-table-data = 테이블 데이터 열기
db-explorer-menu-open-redis = 열기
db-explorer-menu-edit = 연결 편집
db-explorer-menu-delete = 삭제
db-explorer-menu-connect = 연결
db-explorer-menu-disconnect = 끊기
db-explorer-row-count-other-database = 연결의 기본 DB 가 아닌 DB 의 행 수는 콘솔에서 센다

## 연결 삭제 확인.

# $name: 연결 이름
db-explorer-delete-title = 연결 { $name } 을 지운다
db-explorer-delete-body = 공유 · 개인 설정 파일과 이 연결의 저장한 비밀 · 확인 기록이 지워진다. 되돌릴 수 없다.
db-explorer-delete-confirm = 삭제

## 알림.

db-explorer-row-count-title = 행 수
# $name: 테이블 이름, $count: 행 수
db-explorer-row-count-body = { $name }: { $count } 행
# $action: 동작 이름, $error: 오류 글
db-explorer-action-failed = { $action } 하지 못했다: { $error }
# $name: 이름
db-explorer-copied = { $name } 을 복사했다

## DDL 문서(`⌘B`).

# $name: 객체 이름
db-explorer-ddl-title = { $name } DDL
db-explorer-ddl-read = DDL 읽기
db-explorer-ddl-reload = 다시 읽기
db-explorer-ddl-restored = 다시 연 탭이다 — 읽으면 이 연결의 메타데이터용 연결을 연다
