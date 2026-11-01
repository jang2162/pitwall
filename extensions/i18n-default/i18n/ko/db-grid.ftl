### DB 결과 그리드(M3-16, apps/desktop/src/db/grid): 상태 줄 · 필터 · 복사 · 셀 상세. SQL 콘솔 · 테이블 데이터 · Redis 화면이 함께 쓴다.

db-grid-label = 쿼리 결과

## 상태 줄

# $count: 받은 행 수
db-grid-loading = 불러오는 중… { $count }행
# $count: 행 수
db-grid-rows = { $count }행
# $visible: 거른 뒤 보이는 행 수, $total: 받은 행 수
db-grid-filtered = { $visible } / { $total }행
# $ms: 걸린 시간(밀리초)
db-grid-elapsed = { $ms } ms
# $count: 받은 행 수, $size: 받은 셀 크기(예: 64.0 MiB)
db-grid-capped = 상한에서 멈춤({ $count }행 · { $size })
db-grid-capped-hint = 그리드는 행 상한(설정 [db] grid_max_rows)이나 받은 크기 상한([db] grid_max_bytes) 중 먼저 닿는 데까지만 받는다. 더 보려면 쿼리를 좁힌다.
# $count: 바뀐 행 수
db-grid-affected = { $count }행 바뀜
db-grid-cancelled = 취소됨
# $message: 오류 문장
db-grid-failed = 실패: { $message }
db-grid-no-columns = 결과 행이 없는 문장이다

## 필터(보유한 행 위에서 — 서버로 다시 묻지 않는다)

db-grid-filter = 받은 행에서 거르기
db-grid-filter-all-columns = 모든 열
db-grid-filter-column = 거를 열
db-grid-filter-clear = 필터 지우기

## 복사(⌘C 는 TSV, ⌘⇧C 는 형식을 고른다)

db-grid-copy = 복사
db-grid-copy-as = 다른 형식으로 복사…
db-grid-copy-as-title = 다른 형식으로 복사
db-grid-format-csv = CSV
db-grid-format-json = JSON
db-grid-format-insert = INSERT 문
db-grid-format-tsv = TSV
# $format: 형식 이름, $count: 행 수
db-grid-copied = { $format } 로 { $count }행을 복사했다
db-grid-copy-nothing = 고른 셀이 없다
# $row: 행 번호(1부터), $column: 열 이름
db-grid-copy-truncated = 잘린 셀(행 { $row } · { $column })이 있어 INSERT 문으로 복사할 수 없다 — 셀 상한에서 잘린 값은 원래 값이 아니다
db-grid-copy-unsupported = 이 DB 에 그 값을 적을 리터럴이 없어 INSERT 문으로 복사할 수 없다
db-grid-copy-nul = NUL 글자가 든 값은 이 DB 의 리터럴로 적을 수 없다
db-grid-copy-too-large = 고른 셀이 너무 많아 INSERT 문으로 복사할 수 없다
# $limit: 크기 상한(예: 1.0 GiB)
db-grid-copy-clipboard-too-large = 복사할 글이 클립보드 상한({ $limit })보다 크다 — 셀을 덜 고른다
# $message: 오류 문장
db-grid-copy-failed = 복사하지 못했다: { $message }

## 셀 값

# 화면 읽기 프로그램이 읽는 NULL(글자 「NULL」 과 갈린다)
db-grid-null-value = NULL 값

## 셀 상세 패널(더블클릭 · ↩)

db-grid-detail = 셀 상세
db-grid-detail-show = 셀 상세 보기
db-grid-detail-close = 셀 상세 닫기
db-grid-detail-none = 셀을 고르면 값이 여기 보인다
db-grid-detail-text = 글
db-grid-detail-json = JSON
db-grid-detail-hex = Hex
db-grid-detail-json-invalid = JSON 으로 읽지 못해 글로 보인다
# $size: 원래 크기, $received: 받은 앞부분 크기
db-grid-detail-truncated = 셀 상한에서 잘렸다 — 원래 { $size }, 받은 앞부분 { $received }
# $count: 더 있는 항목 수
db-grid-detail-more = … { $count }개 더
# $shown: 보이는 앞부분 크기, $size: 받은 크기
db-grid-detail-hex-partial = 앞 { $shown } 만 보인다(받은 것 { $size })
db-grid-detail-expand = 펼치기
db-grid-detail-collapse = 접기
