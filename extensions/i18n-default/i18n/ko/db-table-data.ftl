### 테이블 데이터(M3-17, apps/desktop/src/db/table-data): 위 막대 · 편집 · 변경 제출 미리보기 · 닫기 확인 · 액션.

db-table-data-title = 테이블 데이터
# $table: 테이블 이름
db-table-data-tab-title = { $table }

## 위 막대

db-table-data-connect = 연결
db-table-data-disconnect = 연결 끊기
db-table-data-where = WHERE
db-table-data-where-placeholder = 예: id > 10 AND name LIKE 'a%'
db-table-data-order-by = ORDER BY
db-table-data-order-by-placeholder = 예: created_at DESC
db-table-data-read = 다시 읽기
db-table-data-read-hint = WHERE · ORDER BY 로 다시 읽는다(칸에서 Enter)
db-table-data-add-row = 행 추가
db-table-data-delete-row = 행 삭제
db-table-data-submit = 제출
db-table-data-revert = 되돌리기
# $count: 변경 수(고친 행 · 추가 행 · 지울 행)
db-table-data-pending = 제출하지 않은 변경 { $count }개
db-table-data-not-connected = 연결하지 않았다 — [연결] 을 누르면 세션을 열고 테이블을 읽는다.
db-table-data-disconnected = 연결이 끊겼다 — 다시 붙으려면 [연결] 을 누른다.
db-table-data-pending-blocks-read = 제출하지 않은 변경이 있어 다시 읽지 않는다 — 제출하거나 되돌린 뒤 읽는다.
# $message: 오류 글
db-table-data-error = 오류: { $message }
# $statements: 문장 수, $rows: 바뀐 행 수
db-table-data-applied = 문장 { $statements }개를 적용했다 — 바뀐 행 { $rows }개.

## 편집이 꺼진 까닭

db-table-data-block-read-only = 읽기 전용 연결이라 고칠 수 없다.
db-table-data-block-no-key = 키(PK 또는 NOT NULL 열만의 유일 키)가 없는 테이블이라 고칠 수 없다 — 행을 정확히 가리킬 수 없다.
db-table-data-block-key-not-in-result = 결과에 키 열이 없어 고칠 수 없다.
# $message: 오류 글
db-table-data-block-failed = 키를 읽지 못해 고칠 수 없다: { $message }

## 변경 표지(그리드의 접근성 글)

db-table-data-mark-changed = 바뀜
db-table-data-mark-inserted = 추가할 행
db-table-data-mark-deleted = 지울 행

## 셀 편집

# $column: 열 이름
db-table-data-edit-label = { $column } 값
db-table-data-edit-null = NULL
db-table-data-edit-apply = 적용
db-table-data-edit-cancel = 취소
db-table-data-edit-key = 키 열은 고치지 않는다 — 키를 바꾸는 편집은 받지 않는다(행을 지우고 새로 추가한다).
db-table-data-edit-truncated = 잘린 값이라 고치지 않는다 — 원래 값 전체를 읽지 못했다.
db-table-data-edit-deleted = 지울 행은 고치지 않는다 — 행 삭제를 다시 눌러 되살린 뒤 고친다.
db-table-data-fk-null = 값이 NULL 이라 가리키는 행이 없다.
db-table-data-fk-pending = 추가 행 · 고친 칸의 값은 따라가지 않는다 — FK 는 읽은 값으로 따라간다.

## 변경 제출 미리보기

db-table-data-preview-title = 변경 제출 미리보기
db-table-data-preview-own = 한 트랜잭션으로 적용하고 커밋한다 — 영향 행이 1 이 아닌 문장이 하나라도 있으면 모두 롤백한다.
db-table-data-preview-open = 열린 트랜잭션 안에서 적용한다 — 실패하면 이 편집만 되돌리고, 커밋은 따로 한다.
# $count: 문장 수
db-table-data-preview-statements = 문장 { $count }개
db-table-data-preview-apply = 적용
db-table-data-preview-cancel = 취소
db-table-data-preview-applying = 적용하는 중…

## 적용 실패

# $change: 변경(「수정 1」 등), $rows: 영향 행 수
db-table-data-fail-row-count = { $change } 의 영향 행이 { $rows }개였다 — 다른 세션이 그 행을 바꾸거나 지웠을 수 있다.
db-table-data-fail-changed = 미리보기 뒤에 세션이 바뀌었다 — 다시 제출해 새 미리보기를 본다.
# $message: DB 의 오류 글
db-table-data-fail-db = DB 가 거절했다: { $message }
# $reason: Host 의 까닭 이름, $column: 열 이름(없으면 빈 글)
db-table-data-fail-invalid = 이 변경은 받지 않는다({ $reason }{ $column })
db-table-data-undone-edit = 아무것도 바뀌지 않았다(편집 트랜잭션을 롤백했다).
db-table-data-undone-savepoint = 이 편집만 되돌렸다 — 열린 트랜잭션의 앞선 변경은 남는다.
db-table-data-undone-transaction = 서버가 트랜잭션 전체를 끝냈다 — 세션 알림을 본다.
db-table-data-undone-unknown = 되돌렸는지 알 수 없다 — 다른 연결로 테이블을 확인한다.
# $index: 1부터
db-table-data-change-update = 수정 { $index }
db-table-data-change-insert = 추가 { $index }
db-table-data-change-delete = 삭제 { $index }

## 닫기 확인

db-table-data-close-title = 제출하지 않은 변경이 있다
# $table: 테이블 이름, $count: 변경 수
db-table-data-close-body = { $table } 의 변경 { $count }개를 제출하지 않았다. 버리고 닫으면 되돌릴 수 없다.
db-table-data-close-confirm = 버리고 닫기

## 액션

db-table-data-action-open = 테이블 데이터 열기
db-table-data-action-submit = 변경 제출
db-table-data-action-revert = 변경 되돌리기
db-table-data-action-add-row = 행 추가
db-table-data-action-delete-row = 행 삭제
