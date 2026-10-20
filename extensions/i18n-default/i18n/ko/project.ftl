### 프로젝트 창: 툴 윈도우 이름과 열기 액션, 에디터 탭, 상태바, 백그라운드 작업, 알림, 런타임 환경, 프로젝트 신뢰, 파일 감시.

## 툴 윈도우 이름. id 는 툴 윈도우 id(src/layout/project-layout.ts 의 PANELS)를 kebab-case 로 쓴 것이다.

tool-window-project = 프로젝트
tool-window-commit = 커밋
tool-window-database = DB 탐색기
tool-window-search = 검색
tool-window-structure = 구조
tool-window-run = 실행
tool-window-debug = 디버그
tool-window-problems = 문제
tool-window-terminal = 터미널
tool-window-git = Git
tool-window-query-results = 쿼리 결과
tool-window-search-results = 검색 결과
tool-window-notifications = 알림
tool-window-agent = 에이전트 활동

# $name: 툴 윈도우 이름(tool-window-*)
action-tool-window = { $name } 툴 윈도우
action-tool-window-hide-all = 모든 툴 윈도우 숨기기

tool-window-bar-left = 왼쪽 툴 윈도우
tool-window-bar-right = 오른쪽 툴 윈도우
# $name: 툴 윈도우 이름(tool-window-*)
tool-window-placeholder = { $name } — 툴 윈도우 자리

## 에디터 탭(src/editor). 탭 하나 = 파일 하나, 가운데 그룹이 에디터 그룹이다.

editor-empty = 열린 파일이 없다
editor-loading = 읽는 중…
# $message: 오류 원문
editor-read-failed = 파일을 읽지 못했다: { $message }
# $size: 파일 크기(단위 포함, 예: 72.4 MiB)
editor-too-large = 50 MiB 를 넘는 파일이라 열지 않는다({ $size })
editor-not-utf8 = UTF-8 이 아닌 텍스트 파일이라 아직 보이지 못한다
editor-read-only-large = 5 MiB 를 넘는 파일이라 읽기 전용으로 연다(구문 강조 · 줄바꿈 끔)
# $kind: image | hex | markdown | text — 그 파일의 뷰어 종류
editor-no-viewer =
    { $kind ->
        [image] 이미지 미리보기는 아직 없다
        [hex] 바이너리 파일이라 아직 보이지 못한다
       *[other] 이 파일의 뷰어는 아직 없다
    }
# $path: 열려던 경로
editor-open-failed = 파일을 열지 못했다: { $path }
editor-copy-path-failed = 경로를 복사하지 못했다
editor-tab-pinned = 고정됨
editor-tab-pin = 탭 고정
editor-tab-unpin = 탭 고정 해제
editor-switcher-label = 탭 전환기

action-editor-close-tab = 탭 닫기
# ⌘W · 메뉴 「닫기」: 에디터 탭이 있으면 탭, 없으면 창(10-ui §5.2).
action-editor-close-tab-or-window = 탭 또는 창 닫기
action-editor-close-other-tabs = 다른 탭 모두 닫기
action-editor-split-right = 오른쪽으로 분할
action-editor-toggle-pin = 탭 고정 / 고정 해제
action-editor-toggle-maximized = 에디터 최대화 / 복원
action-editor-copy-path = 경로 복사
# $platform: mac | win | linux
action-editor-reveal =
    { $platform ->
        [mac] Finder 에서 보기
        [win] 탐색기에서 보기
       *[other] 파일 관리자에서 보기
    }
action-editor-switcher = 탭 전환기

## 상태바

status-bar-label = 상태바
status-host-connecting = 연결 중…
status-host-failed = 연결 실패
# $appearance: light | dark — 지금 그려진 테마
status-appearance =
    { $appearance ->
        [light] 라이트
       *[dark] 다크
    }

## 백그라운드 작업

task-list-label = 백그라운드 작업
# $title: 작업 이름
task-cancel = 「{ $title }」 취소
# $count: 상태바에 보이지 않는 다른 작업 수
task-more = +{ $count }
# $title: 작업 이름
task-failed = 작업 실패: { $title }
task-start-failed = 작업을 시작하지 못했다
task-cancel-failed = 작업을 취소하지 못했다
action-task-demo = 데모 작업 실행
action-task-demo-invalid = 데모 작업 실행(잘못된 인자 — 오류 알림 확인용)

## 알림

notifications-empty = 알림이 없다
notifications-clear = 모두 지우기
# $count: 아직 보지 않은 오류 알림 수
status-notifications =
    { $count ->
        [0] 알림
       *[other] 알림 — 보지 않은 오류 { $count }개
    }
# $level: info | warning | error
notification-level =
    { $level ->
        [error] 오류
        [warning] 경고
       *[info] 정보
    }

# $title: 알림 제목, $count: 합친 횟수(2 이상)
notification-repeated = { $title } (×{ $count })

## 런타임 환경

action-runtime-reload = 런타임 환경 다시 읽기
runtime-reload-failed = 런타임 환경을 다시 읽지 못했다

## 프로젝트 신뢰. 미신뢰 프로젝트에서는 런타임 제공자(mise 등)를 돌리지 않고 로그인 셸 환경만 쓴다.

status-trust-needed = 런타임 환경 미적용(신뢰 필요)
status-trust-needed-hint = 이 프로젝트를 신뢰하기 전에는 로그인 셸 환경만 쓴다. 눌러서 신뢰한다
status-trust-unsavable = 런타임 환경 미적용(신뢰를 저장할 수 없음)
status-trust-unsavable-hint = 프로젝트 레지스트리에 쓸 수 없어(읽기 전용 · 메모리) 이 프로젝트를 신뢰할 수 없다. 로그인 셸 환경만 쓴다
# $name: 프로젝트 이름
trust-dialog-title = 「{ $name }」 프로젝트를 신뢰할까?
trust-dialog-body =
    신뢰하면 이 프로젝트의 런타임 설정(mise.toml 등)으로 환경을 만든다. 그 설정은 프로젝트의 스크립트를 실행할 수 있다.
    신뢰하지 않으면 로그인 셸 환경만 쓴다. 상태바나 액션에서 언제든 바꿀 수 있다.
trust-dialog-trust = 신뢰
trust-dialog-distrust = 신뢰하지 않음
action-project-trust = 이 프로젝트 신뢰…
action-project-untrust = 이 프로젝트 신뢰 해제
trust-save-failed = 프로젝트 신뢰를 저장하지 못했다

## 파일 감시. Host 가 알림(notification.show)으로 낸다(crates/pw-fs/src/watch).

# $root: 감시하는 루트, $seconds: 폴링 간격(초)
fs-watch-limit = 파일 감시 한도(fs.inotify.max_user_watches)를 넘어 일부 폴더는 { $seconds }초마다 확인한다
