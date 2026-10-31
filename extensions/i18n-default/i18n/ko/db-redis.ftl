### Redis 키 문서(M3-18, apps/desktop/src/db/redis): 위 막대 · 키 트리 · 값 뷰어 · 명령 콘솔.

# $connection: 연결 이름, $database: DB 번호
db-redis-title = { $connection } · DB { $database }
db-redis-action-open = Redis 키 열기
db-redis-open-bad-args = Redis 키 문서를 열 연결이 주어지지 않았다

## 위 막대

db-redis-connect = 연결
db-redis-disconnect = 연결 끊기
db-redis-connecting = 연결하는 중…
db-redis-not-connected = 연결하지 않았다 — [연결] 을 누르면 이 연결을 연다
db-redis-database = DB 번호
db-redis-pattern = 키 패턴
db-redis-pattern-placeholder = 패턴(예: user:*) — 비우면 모든 키
db-redis-separator = 구분자
db-redis-rescan = 다시 찾기
db-redis-stop = 멈춤
# $message: 까닭
db-redis-connect-failed = 연결하지 못했다: { $message }

## 키 트리

db-redis-keys = Redis 키
# $count: 받은 키 수
db-redis-scanning = 키 { $count }개 · 찾는 중…
# $count: 받은 키 수, $ms: 걸린 시간(밀리초)
db-redis-scanned = 키 { $count }개 · { $ms } ms
# $count: 받은 키 수
db-redis-scan-stopped = 키 { $count }개 · 멈춤
# $count: 받은 키 수
db-redis-scan-capped = 키 { $count }개에서 멈춤 — 트리가 쥐는 상한이다. 패턴으로 좁힌다
# $message: 까닭
db-redis-scan-failed = 키를 찾지 못했다: { $message }
db-redis-no-keys = 맞는 키가 없다
db-redis-bytes-key = 바이트 키

## 타입

db-redis-type-string = string
db-redis-type-hash = hash
db-redis-type-list = list
db-redis-type-set = set
db-redis-type-zset = zset
db-redis-type-stream = stream
db-redis-type-json = JSON

## 값 뷰어

db-redis-pick-key = 왼쪽에서 키를 고른다
db-redis-missing = 키가 없다 — 지워졌거나 만료됐다
# $ttl: 남은 수명(예: 12 s)
db-redis-ttl = TTL { $ttl }
db-redis-no-ttl = 만료 없음
db-redis-ttl-expired = 만료됨
# $size: 크기(예: 1.2 KiB)
db-redis-size = 크기 { $size }
# $count: 전체 원소 수
db-redis-total = 전체 { $count }
# $page: 쪽 번호(1부터)
db-redis-page = { $page }쪽
db-redis-prev-page = 앞 쪽
db-redis-next-page = 다음 쪽
db-redis-reload = 다시 읽기
db-redis-view-text = 텍스트
db-redis-view-json = JSON
db-redis-view-hex = hex
# $type: Redis 가 돌려준 타입 이름
db-redis-unsupported = 이 타입은 보이지 못한다: { $type }
# $message: 까닭
db-redis-read-failed = 값을 읽지 못했다: { $message }
db-redis-loading = 읽는 중…

## 열 이름(값 표)

db-redis-col-field = field
db-redis-col-value = value
db-redis-col-member = member
db-redis-col-score = score
db-redis-col-index = index
db-redis-col-id = id

## 키 동작(우클릭 · 값 뷰어)

db-redis-open-value = 값 보기
db-redis-edit-ttl = TTL 수정
db-redis-clear-ttl = TTL 지우기
db-redis-delete = 삭제
db-redis-rename = 이름 바꾸기
db-redis-copy-name = 이름 복사
# $key: 키 이름
db-redis-ttl-title = TTL 수정: { $key }
db-redis-ttl-seconds = 새 TTL(초)
db-redis-ttl-invalid = 1 이상의 정수(초)를 넣는다
# $key: 키 이름
db-redis-rename-title = 이름 바꾸기: { $key }
db-redis-rename-to = 새 이름
db-redis-rename-same = 지금 이름과 같다
db-redis-apply = 바꾸기
db-redis-cancel = 취소
# $target: 대상 키 이름
db-redis-overwrite-note = 대상 키 { $target } 이 이미 있다 — 이름을 바꾸면 그 키를 덮어쓴다
# $key: 키 이름
db-redis-deleted = 지웠다: { $key }
# $from: 옛 이름, $to: 새 이름
db-redis-renamed = 이름을 바꿨다: { $from } → { $to }
# $key: 키 이름
db-redis-ttl-changed = TTL 을 바꿨다: { $key }
# $key: 키 이름
db-redis-ttl-cleared = TTL 을 지웠다: { $key }
db-redis-key-gone = 키가 없어 바꾸지 않았다(지워졌거나 만료됐다)
# $message: 까닭
db-redis-action-failed = 하지 못했다: { $message }
db-redis-name-copied = 키 이름을 복사했다

## 명령 콘솔

db-redis-console = 명령
db-redis-console-placeholder = 명령(예: HGETALL user:1) — Enter 로 보낸다
db-redis-console-send = 보내기
db-redis-console-needs-session = 먼저 연결한다
db-redis-console-bytes-arg = UTF-8 이 아닌 인자(\xHH 로 만든 바이트)는 콘솔에서 보낼 수 없다
# $message: 까닭
db-redis-console-parse-failed = 명령을 나누지 못했다: { $message }
db-redis-console-cancelled = 확인을 취소해 보내지 않았다
db-redis-console-busy = 다른 확인이 떠 있어 보내지 않았다
# $message: 까닭
db-redis-console-key-too-large = 키가 커서 서버에 보내지 않았다 — 값 뷰어로 쪽 단위로 본다: { $message }
# $message: 까닭
db-redis-console-failed = 실패: { $message }
# $command: 명령 이름
db-redis-blocked-not-allowed = { $command } 은 콘솔에서 허용하지 않는 명령이다(모르는 명령 · 키 공간 전체 · 연결 상태를 바꾸는 명령)
# $command: 명령 이름
db-redis-blocked-blocking = { $command } 은 기다리는 동안 연결을 붙잡는 명령이라 허용하지 않는다
# $command: 명령 이름
db-redis-blocked-scan-count = { $command } 의 COUNT 가 너무 커 거절했다 — 키 트리를 쓴다
db-redis-reply-nil = (nil)
# $value: 정수
db-redis-reply-integer = (integer) { $value }
# $value: 실수
db-redis-reply-double = (double) { $value }
# $message: 서버의 오류 글
db-redis-reply-error = (error) { $message }
db-redis-reply-empty = (empty array)
# $elements: 원래 원소 수, $size: 원래 크기
db-redis-reply-truncated = …응답이 콘솔 상한에서 잘렸다(원래 원소 { $elements }개 · { $size })
