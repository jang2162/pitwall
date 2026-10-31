### DB 연결 편집 다이얼로그(apps/desktop/src/db/connection, 14-m3-plan.md M3-14): 일반 · SSH 터널 · SSL · 옵션 탭, 저장 위치,
### [연결 테스트].

## 액션(액션 찾기에 보이는 이름).

action-db-connection-add = DB 연결 추가
action-db-connection-edit = DB 연결 편집

## 제목 · 불러오기.

db-conn-title-new = 새 DB 연결
db-conn-title-edit = 연결 편집: { $name }
db-conn-loading = 불러오는 중…
db-conn-load-failed = 연결 설정을 읽지 못했다: { $message }
db-conn-not-found = 연결 「{ $id }」 이 없다 — 설정 파일이 지워졌거나 다른 워크트리의 것이다
db-conn-invalid = 설정 파일이 틀려 이 다이얼로그로 고칠 수 없다 — 파일을 직접 고친다: { $message }
db-conn-config-problem = 프로젝트 설정을 읽지 못했다: { $problem }

## 탭.

db-conn-tab-general = 일반
db-conn-tab-ssh = SSH 터널
db-conn-tab-ssl = SSL
db-conn-tab-options = 옵션

## 일반 탭.

db-conn-field-id = 연결 id
db-conn-field-id-hint = 설정 파일 이름(.pitwall/db/<id>.toml). 저장한 뒤에는 바꿀 수 없다
db-conn-field-name = 이름
db-conn-field-driver = 드라이버
db-conn-driver = { $driver ->
    [postgres] PostgreSQL
    [mysql] MySQL / MariaDB
    [sqlite] SQLite
    [redis] Redis
   *[other] { $driver }
}
db-conn-field-host = 호스트
db-conn-field-host-tunnel = 터널을 쓰면 바스티언에서 본 이름이다
db-conn-field-port = 포트
db-conn-field-port-default = 기본 { $port }
db-conn-field-database = 데이터베이스
db-conn-field-database-redis = DB 번호
db-conn-field-user = 사용자
db-conn-field-path = 파일
db-conn-field-path-hint = 상대 경로는 프로젝트 루트 기준이다
db-conn-required = 「{ $field }」 을 채운다

## 비밀번호 — 값은 키체인에만 넣고(`db.secret.set`) 설정 파일 · 화면 상태에 두지 않는다.

db-conn-password = 비밀번호
db-conn-password-source-none = 없음
db-conn-password-source-keychain = 키체인
db-conn-password-source-env = 환경 변수
db-conn-password-stored = 저장됨 — 바꾸려면 새로 입력한다
db-conn-password-missing = 저장된 비밀번호가 없다
db-conn-password-target-changed = 저장한 비밀번호의 대상(호스트 · 포트 · 사용자 · 터널 · TLS)이 지금 설정과 달라 쓰지 않는다 — 다시 입력한다
db-conn-password-unavailable = 키체인을 쓸 수 없다(잠김 등)
db-conn-password-after-approval = 연결 전에 확인할 것이 있어 저장된 비밀번호는 그 뒤에 본다
db-conn-password-untrusted = 프로젝트를 신뢰해야 비밀번호를 저장하고 연결을 테스트할 수 있다
db-conn-password-hint = 키체인에만 저장한다 — 설정 파일에는 쓰지 않는다
db-conn-env-variable = 변수 이름
db-conn-env-hint = 연결할 때 이 프로젝트의 런타임 환경에서 읽는다. 처음 쓸 때와 대상이 바뀌면 확인을 묻는다
db-conn-state-problem = 확인 기록을 읽지 못해 모든 확인을 다시 묻는다 — 「비밀번호 필요」 는 이 탓일 수 있다. 기록 파일을 고치거나 지운다: { $problem }

## SSH 터널 탭.

db-conn-ssh-use = SSH 터널로 붙는다
db-conn-ssh-host = 목적지
db-conn-ssh-pick = ~/.ssh/config 의 호스트
db-conn-ssh-direct = 직접 입력
db-conn-ssh-direct-placeholder = ssh://[user@]host[:port]
db-conn-ssh-no-hosts = ~/.ssh/config 에 고를 호스트가 없다 — 직접 입력한다
db-conn-ssh-untrusted = 프로젝트를 신뢰해야 ~/.ssh/config 의 호스트 목록을 읽는다
db-conn-ssh-hint = 시스템 ssh 가 ~/.ssh/config · 에이전트 · ProxyJump · known_hosts 를 그대로 쓴다. 처음 연결할 때 목적지를 확인한다

## SSL 탭.

db-conn-ssl-mode = sslmode
db-conn-ssl-auto = 호스트에 따라 정한다(루프백이면 disable, 아니면 verify-full)
db-conn-ssl-unverified = 서버 인증서를 검증하지 않는다
db-conn-ssl-plaintext = TLS 없이 붙는다(평문)
db-conn-ssl-unsupported = { $driver } 드라이버는 { $mode } 를 받지 않는다
db-conn-ssl-unsupported-mysql-verify-ca = 드라이버가 이름 검증만 건너뛸 수 없다 — verify-full 을 쓰거나 인증서 이름을 맞춘다
db-conn-ssl-unsupported-chosen = 고른 sslmode 를 이 드라이버가 받지 않는다 — 다른 값을 고른다
db-conn-ssl-rootcert = CA 인증서
db-conn-ssl-cert = 클라이언트 인증서
db-conn-ssl-key = 클라이언트 키
db-conn-ssl-path-hint = 상대 경로는 프로젝트 루트 기준이다. 처음 연결할 때 파일을 확인한다

## 옵션 탭.

db-conn-read-only = 읽기 전용
db-conn-read-only-hint = 조회만 돈다 — 분류와 DB 의 읽기 전용 트랜잭션 둘로 막는다
db-conn-production = 프로덕션
db-conn-production-hint = 색과 상관없이 빨강으로 표시하고, 조회가 아닌 문장은 모두 확인을 거친다
db-conn-color = 색
db-conn-color-none = 없음
db-conn-color-tag = { $tag ->
    [red] 빨강
    [orange] 주황
    [yellow] 노랑
    [green] 초록
    [blue] 파랑
    [purple] 보라
    [gray] 회색
   *[other] { $tag }
}
db-conn-connect-timeout = 연결 타임아웃(초)
db-conn-connect-timeout-hint = 1~600초. 비우면 10초

## 저장 위치.

db-conn-location = 저장 위치
db-conn-location-shared = 공유 — .pitwall/db/{ $id }.toml(커밋한다)
db-conn-location-local = 개인 — .pitwall/db/{ $id }.local.toml(커밋하지 않는다)
db-conn-location-local-hint = 개인 파일은 공유 파일 위에 덮인다 — 공유 파일에만 있는 키는 그대로 남는다

## 단추 · 결과.

db-conn-test = 연결 테스트
db-conn-testing = 테스트 중…
db-conn-save = 저장
db-conn-saving = 저장 중…
db-conn-cancel = 취소
db-conn-test-ok = 연결했다 — 서버 { $version }
db-conn-test-ok-no-version = 연결했다
db-conn-test-tls = { $tls ->
    [verified] TLS — 서버 인증서를 검증했다
    [unverified] TLS — 서버 인증서를 검증하지 않았다
    [plaintext] 평문 — TLS 없이 붙었다
   *[other] 파일 DB — TLS 없음
}
db-conn-test-draft = 저장하지 않은 초안으로 테스트한다 — 입력한 비밀번호는 이 테스트에만 쓴다
db-conn-test-failed = 연결하지 못했다: { $message }
db-conn-test-secret = 비밀번호가 필요하다 — 비밀번호 칸에 입력하고 다시 테스트한다
db-conn-test-approval = 연결하기 전에 확인이 필요하다({ $kind ->
    [env] 환경 변수
    [file] 프로젝트 밖 파일
    [tunnel] SSH 터널 목적지
   *[other] 인증서 파일
}) — 저장한 뒤 연결할 때 묻는다
db-conn-save-failed = 저장하지 못했다: { $message }
db-conn-secret-failed = 설정은 저장했지만 비밀번호를 저장하지 못했다: { $message }
db-conn-saved = 연결 「{ $name }」 을 저장했다
