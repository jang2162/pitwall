### 모든 창이 함께 쓰는 문자열: 액션, 액션 찾기 · Search Everywhere, 테마 · 언어 고르기, 키맵 파일 오류, 다음에서 열기.

# 그 언어로 쓴 그 언어의 이름. 언어 고르기는 언어마다 그 언어의 이 메시지로 이름을 보여 준다.
language-name = 한국어

window-title-project = 프로젝트
window-title-settings = 설정

action-find = 액션 찾기
action-settings = 설정
action-cli-install = Shell 명령 설치
action-window-close = 창 닫기
# $theme: light | dark | system
action-theme =
    { $theme ->
        [light] 테마: 라이트
        [dark] 테마: 다크
       *[system] 테마: 시스템
    }
# $name: 그 언어로 쓴 그 언어의 이름(그 언어의 language-name)
action-language = 언어: { $name }
action-language-system = 언어: 시스템

find-action-label = 액션 찾기
find-action-placeholder = 액션 이름
find-action-empty = 맞는 액션이 없다
# $reason: OS 가 그 키를 가져가는 이유
find-action-reserved = OS 예약 키({ $reason }) — 앱에 오지 않는다

theme-switcher-label = 테마
theme-system = 시스템 테마
theme-light = 라이트 테마
theme-dark = 다크 테마

# $error: keymap.toml 의 문제
keymap-file-error = 키맵 파일 오류 — 이전 키맵을 쓰는 중: { $error }
# $warnings: 경고들을 "; " 로 이은 것
keymap-file-warning = 키맵 파일 경고: { $warnings }


## Search Everywhere(⇧⇧)와 그 탭 — 파일로 이동(⌘⇧O) · 최근 파일(⌘E) · 프로젝트 스위처(⌘⇧P). src/search-everywhere.

action-search-everywhere = 모든 곳에서 찾기
action-goto-file = 파일로 이동
action-recent-files = 최근 파일
action-project-switcher = 프로젝트 전환
search-everywhere-label = 모든 곳에서 찾기
search-recent-files-label = 최근 파일
search-tab-all = 전체
search-tab-files = 파일
search-tab-actions = 액션
search-tab-projects = 프로젝트
search-tab-branches = 브랜치
search-tab-db-objects = DB 객체
search-placeholder-all = 파일 · 심볼 · 액션 · 프로젝트 · 브랜치 · DB 객체
search-placeholder-files = 파일 이름 — 「이름:줄」 이면 그 줄로
search-placeholder-projects = 프로젝트 이름
search-placeholder-branches = 브랜치 · 태그 이름
search-placeholder-db-objects = 테이블 · 뷰 이름(탐색기에서 읽은 것만)
search-placeholder-recent-files = 파일 이름
search-empty = 맞는 것이 없다
search-more = 더 보기
search-no-recent-files = 이 창에서 연 파일이 아직 없다
search-files-searching = 찾는 중…
search-files-busy = 파일 검색이 바쁘다 — 잠시 뒤 다시 찾는다
# $message: 실패 이유
search-failed = 찾지 못했다: { $message }
# $shown: 보이는 수, $matched: 맞은 수
search-files-truncated = { $matched }개 중 { $shown }개
# $line: 갈 줄(1부터)
search-files-at-line = { $line }줄
search-recent-badge = 최근
search-current-project = 이 창
search-db-table = 테이블
search-db-view = 뷰
# $key: ⌘↩ · Ctrl+Enter
search-db-open-data = { $key } 데이터
search-db-none = 캐시된 이름이 없다 — DB 탐색기에서 연결을 펼치면 그 이름이 여기에 보인다
search-db-untrusted = 신뢰하지 않은 프로젝트라 DB 객체를 보이지 않는다
search-db-truncated = 캐시된 이름이 많아 앞부분만 찾는다

## 심볼로 이동(⌘⌥O) · 클래스로 이동(⌘O) · Search Everywhere 의 심볼 탭(M4-16). src/search-everywhere/symbol-tab.tsx.

action-goto-symbol = 심볼로 이동
action-goto-class = 클래스로 이동
search-tab-symbols = 심볼
search-classes-label = 클래스로 이동
search-placeholder-symbols = 함수 · 타입 · 메서드 이름
search-placeholder-classes = 클래스 · 구조체 · 인터페이스 · 열거 이름
search-symbols-busy = 심볼 인덱스가 바쁘다 — 잠시 뒤 다시 찾는다
search-symbols-waiting-server = 언어 서버의 결과를 기다리는 중…
search-symbols-suspended = 파일 목록이 취소돼 심볼을 찾지 못했다
search-symbols-rebuild = 다시 만들기
search-symbols-outside = 외부
# $line: 줄(1부터)
search-symbols-at = { $line }줄
search-symbol-kind-module = 모듈
search-symbol-kind-class = 클래스
search-symbol-kind-struct = 구조체
search-symbol-kind-interface = 인터페이스
search-symbol-kind-trait = 트레이트
search-symbol-kind-enum = 열거
search-symbol-kind-enumMember = 열거 항목
search-symbol-kind-typeAlias = 타입 별칭
search-symbol-kind-function = 함수
search-symbol-kind-method = 메서드
search-symbol-kind-constructor = 생성자
search-symbol-kind-field = 필드
search-symbol-kind-property = 속성
search-symbol-kind-constant = 상수
search-symbol-kind-variable = 변수
search-symbol-kind-macro = 매크로
search-symbol-kind-impl = 구현

## 설정 창의 외관과 「외부 도구 > Orca 에서 Pitwall 열기」(docs/design/04-features.md F8-6). 키는 apps/desktop/src/settings/strings.ts 가 잇는다.

settings-appearance = 외관
settings-theme = 테마
settings-language = 언어
settings-language-system = 시스템
# $error: 설정을 저장하지 못한 이유
settings-language-failed = 언어를 바꾸지 못했다: { $error }

settings-external-tools = 외부 도구
settings-orca-title = Orca 에서 Pitwall 열기
settings-orca-intro = Orca 의 Settings → General → Open in menu 에 앱을 추가하고 아래 값을 적는다. Orca 는 워크트리 폴더나 파일의 절대 경로를 명령 뒤에 붙여 실행한다.
settings-orca-label = 라벨
settings-orca-command = 명령
settings-orca-copy = 복사
# $value: 복사할 값
settings-orca-copy-value = { $value } 복사
settings-orca-copied = 복사했다
# $error: 클립보드에 쓰지 못한 이유
settings-orca-copy-failed = 복사하지 못했다: { $error }
settings-orca-checking = 검사하는 중…
# $error: 검사가 실패한 이유
settings-orca-check-failed = 검사하지 못했다: { $error }
settings-orca-recheck = 다시 검사
settings-orca-installed = 설치됨
settings-orca-not-installed = 설치 안 됨
settings-orca-other-link = 다른 곳을 가리킴
settings-orca-not-a-link = 설치할 수 없음
settings-orca-on-path = PATH 에 있음
settings-orca-not-on-path = PATH 에 없음
settings-orca-other-on-path = PATH 의 pitwall 이 다른 것
# 설치 위치에는 없지만 PATH 의 pitwall 이 이미 이 앱이다
settings-orca-works-from-path = PATH 에 있음 (설치 위치 밖)
# $target: 설치 위치(/usr/local/bin/pitwall)
settings-orca-detail-installed = { $target } 가 이 앱을 가리킨다.
settings-orca-detail-missing = { $target } 에 Shell 명령이 없다.
# $points: 링크가 가리키는 경로
settings-orca-detail-other-link = { $target } 가 다른 곳을 가리킨다: { $points }
settings-orca-detail-not-a-link = { $target } 에 Pitwall 이 만들지 않은 파일이 있다. 「Shell 명령 설치」 는 이것을 바꾸지 않는다 — 직접 치운 뒤 다시 검사한다.
# $path: PATH 에서 찾은 pitwall 의 경로
settings-orca-detail-works-from-path = { $path } 가 이 앱을 띄우고 Orca 는 이것을 실행한다. { $target } 에 Shell 명령을 설치하지 않아도 된다.
# $path: PATH 에서 찾은 pitwall 의 경로
settings-orca-detail-other-on-path = PATH 에서 먼저 찾는 것은 { $path } 다. Orca 는 이것을 실행한다.
# $resolved: 그 경로의 심링크를 푼 실제 경로
settings-orca-detail-resolved = 실제 파일: { $resolved }
settings-orca-install = Shell 명령 설치
settings-orca-installing = 설치하는 중…
# $app: 이 앱 바이너리, $target: 설치 위치
settings-orca-source-development-build = 개발 빌드({ $app })는 { $target } 에 설치하지 않는다 — 이 Mac 의 pitwall 명령이 이 빌드를 가리키게 되고, 빌드를 지우면 끊긴다. 설치를 확인하려면 PITWALL_SHELL_COMMAND_TARGET 에 임시 폴더 안의 절대 경로를 주고 앱을 띄운다.
# $app: 이 앱 바이너리
settings-orca-source-transient = Pitwall 이 { $app } 에서 돌고 있다 — 디스크 이미지를 꺼내거나 Mac 을 재시작하면 사라지는 곳이다. Applications 폴더로 옮겨 연 뒤 설치한다.
# $target: 설치 위치
settings-orca-absolute = Orca 가 PATH 로 이 앱을 찾지 못한다. 명령 칸에 절대 경로 { $target } 를 적는다.
# $reason: 로그인 셸 환경을 읽지 못한 이유
settings-orca-path-fallback = 로그인 셸 환경을 읽지 못해 앱의 PATH 로 찾았다 — Orca 가 보는 PATH 와 다를 수 있다({ $reason }).
settings-orca-worktree = Orca 가 만든 워크트리에는 커밋하지 않은 .pitwall/*.local.toml 이 없다. Pitwall 은 그때 메인 저장소의 것을 읽는다. 워크트리에도 복사하려면 저장소 루트의 .worktreeinclude 에 .pitwall/project.local.toml 같은 리터럴 경로를 적는다(Orca 는 glob 을 받지 않는다).

## 설정 창 「DB」(M3-20) — 값의 범위는 Host 가 판정한다.
settings-db-title = DB
settings-db-intro = 전역 settings.toml 의 [db] 다. 바꾼 값은 다음 실행 · 다음 연결부터 쓰인다.
settings-db-loading = 읽는 중…
settings-db-failed = 설정을 바꾸지 못했다: { $error }
settings-db-grid-max-rows = 그리드 보유 행 상한
settings-db-grid-max-bytes = 그리드 보유 바이트 상한
settings-db-cell-max-bytes = 셀 상한
settings-db-redis-response-timeout = Redis 응답 타임아웃
settings-db-redis-key-separator = Redis 키 구분자(처음 값)
settings-db-connect-timeout = 연결 타임아웃 기본값
settings-db-unit-rows = 행
settings-db-unit-bytes = 바이트
settings-db-unit-seconds = 초
settings-db-apply = 적용
settings-db-reset = 기본값으로
settings-db-default = 기본 { $value }
settings-db-range = 범위 { $min } ~ { $max }{ $unit }
settings-db-now = 지금 { $value }
settings-db-not-integer = 정수여야 한다
settings-db-out-of-range = { $min } ~ { $max } 사이의 정수여야 한다
settings-db-not-text = 글이어야 한다
settings-db-bad-separator = 1 ~ { $max }글자여야 하고 공백 · 제어 문자가 없어야 한다
settings-db-file-refused = settings.toml 의 { $key } 를 쓰지 않고 기본값을 쓴다 — { $reason }

## 다음에서 열기(⌥F1)와 기본 외부 도구로 열기(⌘⌥E).

action-open-with = 다음에서 열기…
action-open-with-default = 기본 외부 도구로 열기

open-with-label = 다음에서 열기
# $project: 프로젝트 이름
open-with-title = 다음에서 열기: { $project }
open-with-placeholder = 도구 이름
open-with-empty = 맞는 도구가 없다
open-with-loading = 도구 목록을 읽는 중…
open-with-default-badge = 기본
# $reason: open-with-problem-* 문장 하나
open-with-unavailable = 쓸 수 없음: { $reason }
open-with-no-default = 기본 도구가 정해지지 않았다. 여기서 고르거나 settings.toml 의 [open_with] default 를 정한다.
# $tool: 설정이 고른 도구 id
open-with-unknown-default = 기본 도구 「{ $tool }」 는 이 시스템에서 쓸 수 없다. 여기서 고른다.
# $tool: 도구 이름, $reason: open-with-problem-* 문장 하나
open-with-failed = { $tool } 로 열지 못했다: { $reason }
# $command: 실행한 명령줄(터미널에서 직접 해 볼 수 있게)
open-with-command = 명령: { $command }
open-with-dismiss = 닫기
open-with-default-tool = 기본 외부 도구
open-with-list-failed = 외부 도구 목록을 읽지 못했다: { $error }
open-with-problem-unknown-tool = 이 시스템에서 쓸 수 없는 도구다
# $message: settings.toml 의 문제
open-with-problem-invalid-tool = 설정이 잘못됐다({ $message })
# $variable: 중괄호 없는 템플릿 변수 이름(예: branch)
open-with-problem-missing-variable = 명령이 쓰는 {"{"}{ $variable }{"}"} 의 값이 여기에는 없다
# $path: 프로젝트 폴더
open-with-problem-project-root-missing = 프로젝트 폴더 { $path } 가 없다
# $program: 실행 파일 이름, $source: login | process(찾아본 PATH)
open-with-problem-not-found =
    { $source ->
        [login] 「{ $program }」 를 로그인 셸 PATH 에서 찾지 못했다
       *[process] 「{ $program }」 를 앱의 PATH 에서 찾지 못했다(로그인 셸 PATH 를 읽지 못했다)
    }
# $path: 번들 폴더
open-with-problem-bundle-path = { $path } 는 앱 번들이라 「open」 이 보여 주지 않고 실행한다 — 실행하지 않았다
# $program: 실행 파일 경로, $message: OS 오류
open-with-problem-spawn-failed = { $program } 를 띄우지 못했다({ $message })
# $code: 종료 코드, 신호로 끝났으면 "signal"
open-with-problem-exited =
    { $code ->
        [signal] 명령이 신호로 끝났다
       *[other] 명령이 코드 { $code } 로 끝났다
    }

## 언어 서버 상태(상태바 `lsp` 항목 · 상태 팝업 · 탭 위 배너)와 도구 관리(설정 창 「도구」 · 라이선스 · 출처 확인 다이얼로그 · 업데이트 알림) — 15-m4-plan M4-17.

## 상태바 항목. 상태마다 글자가 다르다(색만으로 가르지 않는다 — 10-ui §6).

lsp-status-starting = 시작 중
lsp-status-ready = 준비
# $title: 서버가 알린 진행 제목, $percent: 0 ~ 100
lsp-status-progress = { $title } { $percent }%
lsp-status-progress-unknown = { $title }…
lsp-status-error = 오류
lsp-status-crashed = 오류 — 멈춤
lsp-status-restart-needed = 다시 시작 필요
lsp-status-restart-suggested = 다시 시작 제안
lsp-status-not-installed = 설치 필요
lsp-status-runtime-missing = 런타임 없음
lsp-status-untrusted = 미신뢰
lsp-status-disabled = 꺼짐
lsp-status-stopped = 멈춤
# $count: 같은 문서를 맡는 다른 서버 수
lsp-status-more = +{ $count }
# $server: 서버 id
lsp-status-hint = 언어 서버 { $server } — 눌러서 상태 보기

## 상태 팝업

lsp-status-popup-title = 언어 서버
lsp-status-root = 루트
lsp-status-version = 판
lsp-status-found = 찾은 곳
lsp-status-found-settings = 설정의 경로
lsp-status-found-project-local = 프로젝트 안
lsp-status-found-path = 런타임 환경의 PATH
lsp-status-found-installed = Pitwall 설치본
lsp-status-unknown = —
lsp-status-restart = 다시 시작
lsp-status-stop = 멈춤
lsp-status-log = 로그 열기
lsp-status-disable = 이 프로젝트에서 끄기
lsp-status-install = 설치…
lsp-status-close = 닫기
# $file: 쓴 개인 설정 파일
lsp-status-disabled-note = { $file } 에 적었다 — 커밋되지 않는 개인 설정
lsp-status-action-failed = 언어 서버 동작 실패
# $server: 서버 id
lsp-status-log-title = { $server } 로그
lsp-status-log-empty = 로그가 없다
lsp-status-log-truncated = 앞부분은 생략했다(끝 256 KiB)
# $message: 띄우지 못한 까닭
lsp-status-failed-detail = 띄우지 못했다: { $message }
lsp-status-crashed-detail = 3분 안에 다섯 번 죽어 멈췄다 — [다시 시작] 으로 다시 띄운다

## 서버를 띄울 수 없는 까닭(배너 · 팝업)

# $tool: JDK · Node · Python, $needed: 판 범위(>=21), $install: mise 명령
lsp-runtime-missing = 서버용 { $tool } { $needed } 가 필요하다 — `{ $install }`(프로젝트 고정은 바꾸지 않는다)
lsp-untrusted = 프로젝트를 신뢰해야 언어 서버를 켠다
lsp-trust = 신뢰…

## 탭 위 배너

# $language: 언어 이름, $server: 서버 이름, $license: 라이선스(SPDX)
lsp-banner-not-installed = { $language } 언어 서버 { $server }({ $license })가 없다
lsp-banner-not-installed-plain = { $language } 언어 서버 { $server }가 없다
lsp-banner-install = 설치
lsp-banner-settings = 설정
lsp-banner-dont-ask = 이 프로젝트에서 묻지 않기
# $server: 서버 이름
lsp-banner-installing = { $server } 설치 중…
lsp-banner-failed = 설치하지 못했다: { $message }

## 라이선스 · 출처 확인 다이얼로그(처음 한 번 — 결정 M4-D2)

# $name: 패키지, $version: 판
tools-confirm-title = { $name } { $version } 설치
tools-confirm-update-title = { $name } { $version } 로 업데이트
tools-confirm-intro = 이 패키지를 처음 받는다. 출처와 라이선스를 확인한다.
tools-confirm-changed = 확인하는 사이 판 · 라이선스 · 출처가 바뀌었다. 바뀐 값을 다시 확인한다.
tools-confirm-description = 설명
tools-confirm-version = 판
tools-confirm-license = 라이선스
tools-confirm-source = 받을 곳
tools-confirm-integrity = 무결성
tools-confirm-integrity-pinned = Pitwall 이 고정한 SHA-256 으로 확인한다
tools-confirm-integrity-none = 체크섬 없음 — 받은 파일을 확인할 고정 해시가 없다
tools-confirm-size = 받을 크기
tools-confirm-size-unknown = 알 수 없음
tools-confirm-note = 참고
tools-confirm-homepage = 홈페이지
tools-confirm-cancel = 취소
tools-confirm-install = 설치
tools-confirm-update = 업데이트

## 설정 창 「도구」

tools-title = 도구
tools-intro = 언어 서버 · 포매터를 Pitwall 의 데이터 폴더(tools)에 설치한다(Mason registry). 패키지를 처음 받을 때 라이선스 · 출처를 묻는다.
tools-loading = 읽는 중…
# $error: 오류 원문
tools-load-failed = 읽지 못했다: { $error }
tools-installed = 설치된 도구
tools-installed-empty = 설치된 도구가 없다
tools-col-name = 이름
tools-col-version = 판
tools-col-language = 언어
tools-col-category = 분류
tools-col-license = 라이선스
tools-col-size = 크기
tools-col-checksum = 체크섬
tools-col-servers = 쓰는 서버
tools-col-actions = 작업
tools-checksum-pinned = 고정 해시
tools-checksum-none = 체크섬 없음
# $version: 이전 판
tools-previous = 이전 판 { $version }
# $problem: 쓸 수 없는 까닭
tools-problem = 다시 설치 필요: { $problem }
# $version: 받을 판
tools-update-to = { $version } 로 업데이트
tools-rollback = 이전 판으로
tools-uninstall = 제거
tools-log = 설치 로그
tools-log-empty = 설치 로그가 없다
tools-log-close = 닫기
tools-search = registry 검색
tools-search-placeholder = 이름 · 설명
tools-filter-category = 분류
tools-filter-language = 언어
tools-filter-all = 모두
tools-install = 설치
tools-installed-badge = 설치됨
tools-bundled = 기본 번들
tools-deprecated = 더 이상 관리되지 않음
# $reason: 지원하지 않는 까닭
tools-unsupported = 이 기기에서 설치할 수 없다({ $reason })
# $count: 보인 결과 수
tools-truncated = 결과가 많아 { $count }개만 보인다
tools-search-empty = 맞는 패키지가 없다
tools-registry = registry
# $version: 지금 판, $pinned: 고정 판
tools-registry-version = 판 { $version } · 고정 { $pinned }
# $time: 받은 때
tools-registry-checked = 마지막 확인 { $time }
tools-registry-snapshot = 내장 스냅숏(받은 적 없음)
tools-registry-latest = 최신 registry 사용(하루 한 번 확인)
tools-registry-fetching = 가져오는 중…
# $error: 오류 원문
tools-registry-error = 마지막 가져오기 실패: { $error }
# $name: 패키지
tools-busy = { $name } 작업 중…
# $name: 패키지, $message: 오류 원문
tools-failed = { $name }: { $message }
# $name: 패키지, $version: 판
tools-done = { $name } { $version }
# $runtime: node · python
tools-runtime-missing = { $runtime } 가 필요하다 — mise 로 설치한다(mise use -g { $runtime })

## 업데이트 알림 · 액션

action-tools-manage = 도구 관리 열기
# $count: 업데이트가 있는 도구 수
tools-updates-title = 도구 업데이트 { $count }개
