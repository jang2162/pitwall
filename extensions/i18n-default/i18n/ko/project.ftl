### 프로젝트 창: 툴 윈도우 이름과 열기 액션, 에디터 탭, 파일 트리, 상태바, 백그라운드 작업, 알림, 런타임 환경, 프로젝트 신뢰, 파일 감시.

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
# 이미지 뷰어(M1-22)
editor-image-fit = 맞춤
editor-image-actual = 실제 크기
editor-image-zoom = 배율
# $width $height: 픽셀
editor-image-dimensions = { $width } × { $height } px
# $message: 오류
editor-image-read-failed = 이미지를 읽지 못했다: { $message }
# Markdown 뷰어(M1-22)
editor-markdown-mode = Markdown 보기
editor-markdown-editor = 에디터
editor-markdown-preview = 미리보기
editor-markdown-split = 나란히
editor-markdown-image-too-large = 이미지가 50 MiB 를 넘어 그리지 않는다: { $size }
editor-markdown-image-limit = 이 문서의 이미지가 100 MiB · 50 장 한도를 넘어 그리지 않는다
# $path: 열려던 경로
editor-open-failed = 파일을 열지 못했다: { $path }
editor-copy-path-failed = 경로를 복사하지 못했다
editor-tab-pinned = 고정됨
editor-tab-pin = 탭 고정
editor-tab-unpin = 탭 고정 해제
editor-switcher-label = 탭 전환기
# 탭 머리의 미저장 표시(●)의 이름.
editor-tab-unsaved = 저장하지 않은 편집
# $path: 저장하려던 파일
editor-save-failed = 저장하지 못했다: { $path }

## 외부 변경 배너(src/editor/disk-banner.tsx). 디스크의 파일이 이 탭의 내용과 달라졌을 때.

editor-disk-changed = 디스크에서 바뀌었다. 저장하지 않은 편집이 있어 받지 않았다.
editor-disk-take = 디스크 것 받기
editor-disk-keep = 내 것 유지
editor-disk-compare = 비교
editor-disk-compare-close = 비교 닫기
editor-disk-compare-mine = 내 것(편집 중)
editor-disk-compare-disk = 디스크
editor-disk-deleted = 디스크에서 지워졌다. 저장하면 다시 만든다.
# $path: 같은 내용으로 새로 생긴 파일
editor-disk-renamed = 다른 이름으로 옮겨진 것 같다: { $path }
editor-disk-open-renamed = 새 이름으로 열기
editor-disk-dismiss = 닫기

## 미저장 확인(src/editor/unsaved-dialog.tsx). 탭 · 창을 닫거나 앱을 끝낼 때.

editor-unsaved-title = 저장하지 않은 편집
# $count: 파일 수, $files: 파일 이름(줄마다 하나)
editor-unsaved-body =
    { $count ->
        [1] 이 파일의 편집을 저장하지 않았다:
       *[other] 이 파일 { $count }개의 편집을 저장하지 않았다:
    }
    { $files }
editor-unsaved-save = 저장
editor-unsaved-discard = 저장 안 함
editor-unsaved-cancel = 취소
# 「저장」 이 끝나지 않았다(충돌 · 실패). 그 탭의 배너 · 알림을 본다.
editor-unsaved-save-incomplete = 저장하지 못한 파일이 있어 닫지 않았다

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
# ⌘S: 활성 탭의 문서를 저장한다(자동 저장 없음, 결정 M1-D13).
action-editor-save = 저장
action-editor-save-all = 모두 저장

## 파일 트리(src/file-tree). 프로젝트 툴 윈도우의 내용이다.

file-tree-label = 프로젝트 파일
file-tree-no-root = 프로젝트 루트를 읽는 중…
file-tree-loading = 읽는 중…
# $message: Host 의 오류 문장
file-tree-read-failed = 읽지 못했다: { $message }
file-tree-outside = 루트 밖을 가리킴
file-tree-ignored-hide = 무시된 파일 숨기기
file-tree-ignored-show = 무시된 파일 보이기
file-tree-search-label = 트리에서 찾기
file-tree-search-none = 맞는 항목이 없다
file-tree-copy-relative-path = 상대 경로 복사
action-file-tree-select-in = 프로젝트 뷰에서 선택
action-file-tree-toggle-ignored = 무시된 파일 숨기기 / 보이기
action-file-tree-new-file = 새 파일
action-file-tree-new-folder = 새 폴더
action-file-tree-rename = 이름 바꾸기(프로젝트 뷰)
action-file-tree-delete = 삭제(프로젝트 뷰)
action-file-tree-open-in-terminal = 터미널에서 열기
# 「다음에서 열기」(⌥F1) 팝업의 첫 항목.
open-with-project-view = 프로젝트 뷰
open-with-project-view-no-file = 열린 파일이 없다
open-with-project-view-outside = 이 파일은 프로젝트 트리 밖에 있다

## 파일 조작(src/file-tree/file-ops.ts · file-dialogs.tsx, M1-21). 트리의 우클릭 메뉴 · ⇧F6 · ⌫ · 드래그 · Finder 드롭.

file-ops-menu-new-file = 새 파일…
file-ops-menu-new-folder = 새 폴더…
file-ops-menu-rename = 이름 바꾸기…
file-ops-menu-delete = 삭제…
file-ops-menu-open-in-terminal = 터미널에서 열기
file-ops-name-new-file = 새 파일
file-ops-name-new-folder = 새 폴더
file-ops-name-rename = 이름 바꾸기
# $folder: 만들 자리의 폴더 이름
file-ops-name-in = 「{ $folder }」 안에
file-ops-name-label = 이름
file-ops-name-empty = 이름을 입력한다
file-ops-name-separator = 이름에 / 나 \ 를 쓸 수 없다
file-ops-name-dots = 이 이름은 쓸 수 없다
file-ops-ok = 확인
file-ops-cancel = 취소
file-ops-delete-title = 삭제
# $count: 항목 수, $name: 항목이 하나일 때 그 이름
file-ops-delete-body =
    { $count ->
        [1] 「{ $name }」 을(를) 휴지통으로 옮긴다.
       *[other] 항목 { $count }개를 휴지통으로 옮긴다.
    }
# $files: 파일 이름(줄마다 하나). 그 탭은 닫지 않고 「지워짐」 배너로 남는다 — 저장하면 다시 만든다.
file-ops-delete-unsaved =
    저장하지 않은 편집이 있는 파일이 있다. 그 탭은 열린 채로 남는다:
    { $files }
file-ops-delete-confirm = 휴지통으로 옮기기
file-ops-permanent-title = 휴지통을 쓸 수 없음
# $files: 항목 이름(줄마다 하나), $reason: 휴지통의 오류 문장
file-ops-permanent-body =
    이 항목을 휴지통에 넣지 못했다({ $reason }). 영구히 지우면 되돌릴 수 없다:
    { $files }
file-ops-permanent-confirm = 영구 삭제
# $name: 겹치는 이름
file-ops-exists = 「{ $name }」 이(가) 이미 있다 — 아무것도 바꾸지 않았다
# $message: 오류 원문
file-ops-create-failed = 만들지 못했다: { $message }
file-ops-rename-failed = 이름을 바꾸지 못했다: { $message }
file-ops-move-failed = 옮기지 못했다: { $message }
file-ops-copy-failed = 복사하지 못했다: { $message }
file-ops-delete-failed = 지우지 못했다: { $message }
# $files: 항목 이름(줄마다 하나)
file-ops-delete-permission =
    권한이 없어 지우지 못했다:
    { $files }
file-ops-drop-task = 드롭한 항목 복사
# $files: 원본 이름(줄마다 하나)
file-ops-drop-conflicts =
    같은 이름이 있어 복사하지 않았다:
    { $files }
file-ops-drop-skipped =
    특수 파일(FIFO · 소켓 · 장치)이라 건너뛰었다:
    { $files }
file-ops-drop-cancelled = 드롭한 항목 복사를 멈췄다 — 끝낸 것은 남는다
file-ops-drop-failed = 드롭한 항목을 복사하지 못했다: { $message }

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
# Host 가 보내는 작업 제목(`task-title-*` id).
task-title-indexing-files = 파일 목록 만드는 중
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
# 상태바 항목(src/runtime/runtime-status.tsx): 어느 제공자가 환경을 만들었는지, 물러났으면 경고.
status-runtime-mise = mise
# $files: mise 설정 파일(줄마다 하나)
status-runtime-mise-hint =
    런타임 환경을 mise 로 만들었다. 누르면 다시 읽는다.
    { $files }
# $problem: loginShell | notInstalled | notTrusted | failed
status-runtime-fallback =
    { $problem ->
        [notInstalled] mise 없음
        [notTrusted] mise 신뢰 필요
        [failed] mise 실패
       *[loginShell] 로그인 셸 못 읽음
    }
# $problem: 위와 같음, $reason: 오류 원문
status-runtime-fallback-hint =
    { $problem ->
        [notInstalled] 프로젝트에 mise 설정이 있지만 로그인 셸 PATH 에 mise 가 없다. 로그인 셸 환경을 쓴다.
        [notTrusted] mise 가 이 프로젝트의 설정을 신뢰하지 않는다. 프로젝트에서 `mise trust` 를 실행한 뒤 다시 읽는다. 로그인 셸 환경을 쓴다.
        [failed] mise 가 실패해 로그인 셸 환경을 쓴다.
       *[loginShell] 로그인 셸 환경을 읽지 못해 앱 자신의 환경을 쓴다.
    }
    { $reason }
    누르면 런타임 환경을 다시 읽는다.

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

## 파일 감시. 한도 경고는 Host 가 알림(notification.show)으로 낸다(crates/pw-fs/src/watch). 상태바 항목은 src/fs/file-watch.tsx.

# $root: 감시하는 루트, $seconds: 폴링 간격(초)
fs-watch-limit = 파일 감시 한도(fs.inotify.max_user_watches)를 넘어 일부 폴더는 { $seconds }초마다 확인한다

status-file-watch-polling = 파일 감시: 폴링
# $roots: 폴링으로 보는 루트(줄마다 하나)
status-file-watch-polling-hint =
    파일 감시 한도(fs.inotify.max_user_watches)를 넘어 일부 폴더는 폴링으로 확인한다. 그 폴더의 변경은 늦게 보인다.
    { $roots }

## 터미널 툴 윈도우

terminal-tabs-label = 터미널 탭
# $index: 같은 이름의 탭 번호(1부터). 1 이면 번호를 붙이지 않는다.
terminal-tab-name =
    { $index ->
        [1] 로컬
       *[other] 로컬 ({ $index })
    }
terminal-tab-exited = 끝남
terminal-new-tab = 새 터미널 탭
# $name: 탭 이름
terminal-close-tab = 「{ $name }」 닫기
terminal-rename-input = 탭 이름
# $code: 종료 코드. 신호로 끝났으면 none.
terminal-exited =
    { $code ->
        [none] [프로세스가 끝났다]
       *[other] [프로세스가 끝났다: 코드 { $code }]
    }
# $message: Host 가 알린 이유
terminal-start-failed = [터미널을 열지 못했다: { $message }]
terminal-no-host = Host 에 연결되지 않아 터미널을 열 수 없다
terminal-copy-failed = 터미널 선택을 복사하지 못했다
terminal-paste-failed = 클립보드를 터미널에 붙여 넣지 못했다
# 경로에 제어 문자가 있거나 셸을 모를 때(따옴표 규칙을 모른다)
terminal-drop-refused = 드롭한 경로를 붙이지 않았다: 그중 이 터미널의 셸에 안전하게 넣을 수 없는 경로가 있다
action-terminal-new-tab = 터미널: 새 탭
action-terminal-close-tab = 터미널: 탭 닫기
action-terminal-rename-tab = 터미널: 탭 이름 바꾸기
action-terminal-copy = 터미널: 복사
action-terminal-paste = 터미널: 붙여넣기

## 전체에서 찾기(src/search). ⌘⇧F 팝업과 「검색 결과」 툴 윈도우.

action-find-in-files = 전체에서 찾기
find-placeholder = 찾을 글자
find-match-case = 대소문자 구분
find-whole-word = 단어 단위
find-regex = 정규식
find-exclude-vendored = 벤더 · 생성 파일 빼기
find-scope-project = 프로젝트
find-scope-folder = 폴더
find-folder-label = 찾을 폴더(프로젝트 루트 기준)
find-mask-label = 파일 마스크
find-mask-placeholder = 예: *.rs, !*_test.rs
find-searching = 찾는 중…
# $lines: 일치한 줄 수, $files: 파일 수
find-summary = { $lines } 줄 · { $files } 파일
# 결과 상한(1만 줄)을 넘었을 때
find-more = 더 있음
find-cancelled = 멈춤
# $message: Host 가 알린 이유(잘못된 정규식 · 글롭 등)
find-failed = 찾지 못했다: { $message }
# Host 의 동시 검색 상한에 걸렸다
find-busy = 다른 검색이 돌고 있어 시작하지 못했다 — 잠시 뒤 다시 찾는다
find-empty = 맞는 것이 없다
find-stop = 멈추기
find-pin = 툴 윈도우에서 열기
find-preview-label = 미리보기
find-preview-loading = 읽는 중…
# $message: Host 가 알린 이유
find-preview-failed = 미리보기를 읽지 못했다: { $message }
find-results-empty = 고정한 검색 결과가 없다 — 전체에서 찾기 팝업의 「툴 윈도우에서 열기」 가 결과를 여기에 둔다
# $query: 찾은 글자
find-results-title = 「{ $query }」
find-in-folder = 이 폴더에서 찾기
