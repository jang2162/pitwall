### 언어 서버 상태(상태바 `lsp` 항목 · 상태 팝업 · 탭 위 배너)와 도구 관리(설정 창 「도구」 · 라이선스 · 출처 확인 다이얼로그 · 업데이트 알림) — 15-m4-plan M4-17.

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
