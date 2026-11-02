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
action-tool-window-last = 마지막 툴 윈도우로
action-focus-editor = 에디터로 포커스
action-layout-preset-default = 레이아웃: 기본
action-layout-preset-git-review = 레이아웃: Git 리뷰
action-layout-preset-db = 레이아웃: DB
action-tool-window-stretch-left = 툴 윈도우 경계를 왼쪽으로
action-tool-window-stretch-right = 툴 윈도우 경계를 오른쪽으로
action-tool-window-stretch-up = 툴 윈도우 경계를 위로
action-tool-window-stretch-down = 툴 윈도우 경계를 아래로

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
# 일반 파일이 아닌 것(FIFO · 장치 · 소켓)은 열면 멈추거나 끝이 없다
editor-special-file = 일반 파일이 아니어서(FIFO · 장치 · 소켓) 열지 않는다
# $size: 보일 크기(단위 포함, 예: 1.0 MiB)
editor-show-head = 앞부분 { $size } 보기
editor-show-hex = hex 로 보기
# $shown: 보인 크기, $size: 파일 크기(둘 다 단위 포함)
editor-head-notice = 앞부분 { $shown } 만 보인다(전체 { $size }) · 읽기 전용
editor-binary-as-text = 바이너리 파일을 텍스트로 보인다(UTF-8 로 읽을 수 없는 바이트는 �) · 읽기 전용
editor-read-only-large = 5 MiB 를 넘는 파일이라 읽기 전용으로 연다(구문 강조 · 줄바꿈 끔)
# 에디터 팝업(M4-07)의 액션 줄 — 화면 읽기 프로그램이 읽는 이름.
lsp-popup-actions = 팝업 동작
# $encoding: 짐작한 인코딩 이름(예: EUC-KR, UTF-16LE)
editor-read-only-encoding = { $encoding } 로 읽었다 · UTF-8 이 아니라 읽기 전용으로 연다
editor-read-only-lossy = UTF-8 로 읽었다 · 깨진 바이트가 있어(�) 읽기 전용으로 연다
# hex 뷰어(M1-23)
editor-hex-label = hex · 읽기 전용 · 16바이트/줄
editor-hex-goto-title = 오프셋으로 이동
editor-hex-goto-input = 오프셋(4096 · 0x1000)
# $current: 지금 오프셋, $last: 마지막 바이트의 오프셋(둘 다 0x 16진수)
editor-hex-goto-hint = 지금 { $current } · 마지막 { $last }
editor-hex-goto-invalid = 10진수(4096)나 16진수(0x1000 · 1000h)로 적는다
# $message: 오류 원문
editor-hex-read-failed = 바이트를 읽지 못했다: { $message }
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
# 저장하지 못한 알림의 본문: 편집하는 사이 디스크의 파일이 UTF-8 이 아니게 됐거나 쓰기 권한이 없어졌다.
editor-save-read-only = 이 파일은 이제 읽기 전용이라 쓰지 않았다. 편집은 탭에 남아 있다.

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

## 에디터 조작(M1-18): 찾기 · 바꾸기 막대(src/editor/find-bar.tsx), 줄로 이동(goto-line.tsx), 편집 키(editor-ops.ts),
## 상태바의 에디터 항목(editor-status.tsx).

action-editor-find = 찾기
action-editor-replace = 바꾸기
action-editor-goto-line = 줄로 이동
action-editor-undo = 실행 취소
action-editor-redo = 실행 복귀
action-editor-duplicate-line = 줄 복제
action-editor-delete-line = 줄 삭제
action-editor-move-line-up = 줄 위로 옮기기
action-editor-move-line-down = 줄 아래로 옮기기
action-editor-comment-line = 줄 주석 넣기 / 걷기
action-editor-select-next-occurrence = 다음 일치 항목에 커서 추가
action-editor-unselect-occurrence = 마지막 커서 추가 취소
action-editor-select-all-occurrences = 모든 일치 항목 선택
action-editor-add-cursor-above = 위 줄에 커서 추가
action-editor-add-cursor-below = 아래 줄에 커서 추가
action-editor-toggle-column-mode = 컬럼 선택 모드
action-editor-single-cursor = 커서 하나로
# $cap: 모든 일치 선택의 상한(멀티 커서, M4-14)
editor-occurrences-capped = 일치가 { $cap }개를 넘어 커서 주변 { $cap }곳만 골랐다
action-editor-open-as-text = 텍스트로 열기
action-editor-open-as-hex = hex 로 열기
action-editor-open-as-default = 기본 뷰어로 열기
action-hex-goto-offset = 오프셋으로 이동

editor-find-label = 파일 안 찾기
editor-find-search = 찾을 내용
editor-find-replace = 바꿀 내용
editor-find-case = 대소문자 구분
editor-find-word = 단어 단위
editor-find-regexp = 정규식
editor-find-previous = 이전 일치(⇧↩)
editor-find-next = 다음 일치(↩)
editor-find-replace-one = 바꾸기
editor-find-replace-all = 모두 바꾸기
editor-find-close = 닫기(Esc)
editor-find-invalid = 틀린 정규식
editor-find-none = 결과 없음
# 큰 문서에서 아직 일치를 하나도 못 찾은 채 세는 중
editor-find-counting = 세는 중…
# $total: 일치 수(상한에서 멈췄으면 「9999+」 처럼 글자)
editor-find-count = { $total }개
# $current: 고른 일치의 차례, $total: 일치 수
editor-find-count-current = { $current }/{ $total }

## 내비게이션 기록(M4-09, src/editor/navigation-ui.tsx): 뒤로 · 앞으로(⌘[ · ⌘]) · 「최근 위치」 팝업(⌘⇧E).

nav-action-back = 뒤로
nav-action-forward = 앞으로
nav-action-recent-locations = 최근 위치
nav-recent-empty = 아직 기록된 위치가 없다

editor-goto-title = 줄로 이동
editor-goto-input = 줄[:열]
# $cursor: 지금 커서(줄:열), $lines: 문서의 줄 수
editor-goto-hint = 지금 { $cursor } · 모두 { $lines }줄
editor-goto-invalid = 「줄」 이나 「줄:열」 로 적는다(예: 120:5)
editor-goto-go = 이동
editor-goto-cancel = 취소

# $line · $column: 커서
status-editor-cursor = { $line }:{ $column }
# $chars: 고른 글자 수, $breaks: 고른 범위의 줄바꿈 수
status-editor-selection =
    { $breaks ->
        [0] { $chars }자
       *[other] { $chars }자, 줄바꿈 { $breaks }개
    }
status-editor-cursor-hint = 줄로 이동
# $count: 커서 수(둘 이상 — 멀티 커서, M4-14)
status-editor-cursors = { $count } 커서
# 컬럼 선택 모드가 켜져 있다(⌘⇧8)
status-editor-column-mode = 컬럼
status-editor-column-mode-hint = 컬럼 선택 모드: ⇧+화살표와 드래그가 사각 선택이다
# $ending: lf | crlf | cr | mixed
status-editor-line-ending =
    { $ending ->
        [lf] LF
        [crlf] CRLF
        [cr] CR
       *[other] 섞임
    }
status-editor-line-ending-hint = 줄 구분 기호(저장할 때 그대로 둔다)
status-editor-encoding-hint = 파일 인코딩
# $encoding: 인코딩 이름(UTF-8)
status-editor-encoding-lossy = { $encoding }(깨진 바이트 있음)
status-editor-language-hint = 이 파일의 언어 바꾸기
language-plain-text = 일반 텍스트
language-popup-label = 언어 고르기
language-popup-placeholder = 언어 이름
language-popup-empty = 맞는 언어가 없다
# $language: 판별한 언어 이름
language-popup-detect = 자동 판별로 되돌리기({ $language })
language-popup-popular = 자주 쓰는 언어
language-popup-all = 모든 언어

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
task-title-indexing-symbols = 심볼 색인 만드는 중
task-title-git-continue = git 작업 계속하는 중
task-title-git-abort = git 작업 중단하는 중
task-title-git-skip = git 작업의 단계 건너뛰는 중
task-title-git-commit = git 커밋하는 중
task-title-git-stage = 변경 스테이징하는 중
task-title-git-unstage = 스테이징 푸는 중
task-title-git-discard = 변경 버리는 중
task-title-git-stash-push = stash 만드는 중
task-title-git-stash-apply = stash 적용하는 중
task-title-git-stash-pop = stash 꺼내는 중
task-title-git-stash-drop = stash 버리는 중
task-title-git-status = git 상태 읽는 중
task-title-git-resolve = git 충돌 해결하는 중
task-title-git-merge = git merge 하는 중
task-title-git-cherry-pick = git cherry-pick 하는 중
task-title-git-revert = git revert 하는 중
task-title-git-rebase = git rebase 하는 중
task-title-git-reset = git reset 하는 중
task-title-git-checkout = 체크아웃하는 중
task-title-git-branch-create = 브랜치 만드는 중
task-title-git-branch-rename = 브랜치 이름 바꾸는 중
task-title-git-branch-delete = 브랜치 지우는 중
task-title-git-tag-create = 태그 만드는 중
task-title-git-fetch = fetch 하는 중
task-title-git-pull = pull 하는 중
task-title-git-push = push 하는 중
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

## Git. 쓰기를 끈 경고는 Host 가 알림(notification.show)으로 한 번 낸다(crates/pw-git). 본문은 그 git 실행 파일의 경로다.

# $version: 그 git 의 버전(읽지 못했으면 그 이유), $required: 쓰기에 필요한 가장 낮은 버전
git-too-old = git { $version } 은(는) { $required } 보다 낮아 커밋 · 브랜치 같은 변경 작업을 끈다. 보기는 그대로 된다

## DB. 아무도 다시 붙지 않은 열린 트랜잭션을 Host 가 롤백하면 알림(notification.show)으로 낸다(crates/pw-db/src/session.rs). 같은 롤백은
## 다음에 창이 열릴 때 db.session.unattendedRollbacks 로도 읽힌다.

# $connection: 연결 이름, $minutes: 창이 돌아오기를 기다린 시간(분)
db-unattended-transaction-rolled-back = 연결 { $connection } 의 열린 트랜잭션을 롤백했다 — { $minutes }분 안에 다시 붙은 창이 없었다
# $connection: 연결 이름
db-closed-transaction-rolled-back = 연결 { $connection } 의 열린 트랜잭션을 롤백했다 — 창의 연결이 닫혔다

## git 인증(src/git/git-prompts.tsx). git 이나 ssh 가 암호 · 토큰 · 키 암호 · 호스트 키 확인을 묻는다.
## 다이얼로그의 질문은 git · ssh 가 준 문장 그대로이고 번역하지 않는다.

git-askpass-title = git 이 인증 정보를 묻는다
git-askpass-confirm-title = git 이 확인을 묻는다
git-askpass-notice-title = git 이 보안 키를 기다린다
git-askpass-input = 답
git-askpass-submit = 확인
git-askpass-cancel = 취소
git-askpass-yes = 예
git-askpass-no = 아니오
git-askpass-dismiss = 닫기

## git 상태(src/git/git-status-mark.tsx). 파일 트리 행 · 에디터 탭의 글자(M · A · D …)가 읽히는 이름과 툴팁.

git-status-added = 추가됨
git-status-modified = 수정됨
git-status-deleted = 삭제됨
git-status-renamed = 이름 바뀜
git-status-copied = 복사됨
git-status-type-changed = 종류 바뀜
git-status-untracked = 추적 안 됨
git-status-conflicted = 충돌
git-status-conflict-both-modified = 충돌: 양쪽에서 고침
git-status-conflict-both-added = 충돌: 양쪽에서 만듦
git-status-conflict-both-deleted = 충돌: 양쪽에서 지움
git-status-conflict-added-by-us = 충돌: 이쪽에서 만듦
git-status-conflict-added-by-them = 충돌: 그쪽에서 만듦
git-status-conflict-deleted-by-us = 충돌: 이쪽에서 지움
git-status-conflict-deleted-by-them = 충돌: 그쪽에서 지움
# $change: 변경 이름(git-status-*)
git-status-staged = 스테이징됨: { $change }
# $change: 변경 이름(git-status-*)
git-status-unstaged = 작업 트리: { $change }
# 폴더의 색: 그 아래에 바뀐 파일이 있다
git-status-folder = 아래에 바뀐 파일이 있다

## 브랜치(src/git/branch-*.tsx, M2-13). 상태바의 브랜치 항목 · 브랜치 팝업(⌃⇧B) · VCS 작업 팝업(⌃V).
action-git-branches = 브랜치…
action-git-vcs-operations = VCS 작업…
# 상태바 항목의 툴팁: 잡은 저장소(작업 트리 꼭대기)
branch-status-repository = 저장소: { $path }
# 스크린 리더 이름. $name: 「main ↑1↓2 MERGING」
branch-status-label = 브랜치 { $name }
branch-status-unsupported = 지원하지 않는 저장소
branch-status-unsupported-detail = 이 저장소의 ref 형식(reftable)은 아직 읽지 못해 git 기능을 끈다. 터미널의 git 은 그대로 쓸 수 있다
branch-status-detached = 분리된 HEAD
# 진행 중 작업의 표지(git 의 프롬프트와 같은 글). $kind: merge · rebase · cherryPick · revert · bisect · am
branch-status-operation = { $kind ->
        [merge] MERGING
        [rebase] REBASING
        [cherryPick] CHERRY-PICKING
        [revert] REVERTING
        [bisect] BISECTING
       *[other] AM
    }
branch-popup-label = 브랜치
branch-popup-placeholder = 브랜치 · 태그 · 워크트리 찾기
branch-popup-empty = 맞는 것이 없다
branch-popup-loading = 브랜치를 읽는 중…
branch-popup-no-repository = 이 프로젝트는 git 저장소가 아니다
branch-popup-failed = 브랜치를 읽지 못했다: { $message }
branch-group-favorites = 즐겨찾기
branch-group-local = 로컬
branch-group-remote = 원격
branch-group-tags = 태그
branch-group-worktrees = 워크트리
branch-new = 새 브랜치…
branch-current = 지금
branch-favorite = 즐겨찾기에 더하기
branch-unfavorite = 즐겨찾기에서 빼기
branch-worktree-main = 메인
branch-worktree-current = 이 창
branch-worktree-missing = 폴더 없음
branch-worktree-locked = 잠김
branch-worktree-other = 이 창에서 열 수 없음
branch-back = 뒤로
branch-checkout = 체크아웃
# 원격 브랜치를 따를 로컬 브랜치 이름을 묻는다(같은 이름이 이미 있을 때). $local: 원격 브랜치(origin/x)
branch-checkout-tracking = { $local } 를 따르는 로컬 브랜치 이름
branch-new-from = { $name } 에서 새 브랜치…
branch-merge = { $name } 를 { $current } 에 merge
branch-rebase = { $current } 를 { $name } 위로 rebase
branch-compare = { $current } 와 비교
branch-rename = 이름 바꾸기…
branch-delete = 삭제
branch-new-hint = { $start } 에서 만들고 그리로 체크아웃한다
branch-name-empty = 이름을 적는다
branch-name-bad = 쓸 수 없는 이름이거나 이미 있는 브랜치다
branch-merge-title = { $name } 를 { $current } 에 merge 한다
branch-rebase-title = { $current } 를 { $name } 위로 rebase 한다
branch-history-body = 충돌이 나면 멈추고 진행 중 작업으로 남는다(상태바에 표지가 붙는다)
branch-proceed = 진행
branch-result-checked-out = { $name } 로 체크아웃했다
branch-result-created = 브랜치 { $name } 를 만들었다
branch-result-renamed = 브랜치 이름을 { $name } 로 바꿨다
# $commit: 지운 브랜치가 가리키던 짧은 해시 — 같은 이름으로 다시 만들 수 있다
branch-result-deleted = 브랜치 { $name } 를 지웠다({ $commit })
branch-result-merged = { $name } 를 merge 했다
branch-result-rebased = { $name } 를 rebase 했다
branch-result-up-to-date = 이미 최신이다
branch-result-stopped = 충돌로 멈췄다 — 충돌을 풀고 계속한다
branch-result-failed = { $what } 실패
branch-compare-missing = 이 창에는 아직 비교 화면이 없다
vcs-popup-label = VCS 작업
vcs-popup-placeholder = 작업 찾기
vcs-popup-empty = 이 창에서 할 수 있는 git 작업이 없다

## git 문서(src/git/documents · src/git/diff, M2-15): diff 뷰 · 커밋 상세 · 두 커밋 비교 · 리비전의 파일. 해시는 앞 7글자다.

action-git-show-commit = 커밋 상세 열기
action-git-compare-commits = 두 커밋 비교
action-git-open-revision-file = 리비전의 파일 열기
action-diff-next-change = diff 에서 다음 차이
action-diff-previous-change = diff 에서 이전 차이
git-doc-commit-title = 커밋 { $hash }
git-doc-compare-title = { $a } ↔ { $b }
git-doc-revision-title = { $name } ({ $hash })
# 리비전 blame 문서(M2-17). $name: 파일 이름, $hash: 짧은 해시
git-doc-blame-title = { $name } (blame { $hash })
git-doc-loading = 읽는 중…
git-doc-failed = 열지 못했다: { $message }
git-doc-bad-args = { $action } 에 넘긴 값이 맞지 않다
git-doc-author = 작성자
git-doc-committer = 커미터
git-doc-parents = 부모
git-doc-no-parents = 없음(루트 커밋)
git-doc-shallow = 얕은 클론의 경계 — 부모가 저장소에 없다
git-doc-files = 바뀐 파일 { $count }개
git-doc-no-files = 바뀐 파일이 없다
git-doc-compare-heading = { $a } 에서 { $b } 까지
git-doc-revision-bar = 리비전 { $hash } · 읽기 전용
git-doc-pick-file = 왼쪽에서 파일을 고른다
git-diff-mode = diff 보기
git-diff-split = 나란히
git-diff-unified = 통합
git-diff-ignore-whitespace = 공백 무시
git-diff-next = 다음 차이
git-diff-previous = 이전 차이
git-diff-changes = 바뀐 곳 { $count }군데
git-diff-unchanged-lines = 바뀌지 않은 줄 $줄
git-diff-generated = 생성 파일이라 diff 를 접어 두었다(+{ $added } −{ $removed })
git-diff-expand = 펼치기
git-diff-too-large = 파일이 너무 커서 diff 를 보이지 않는다
git-diff-binary = 바이너리 파일이다 — 내용 diff 가 없다
git-diff-submodule = 서브모듈 커밋 { $old } → { $new }
git-diff-submodule-none = (없음)
git-diff-error = 이 파일을 읽지 못했다: { $message }
git-diff-no-content-change = 내용은 그대로다
git-diff-mode-change = 모드 { $old } → { $new }
git-diff-renamed = { $old } 에서 이름이 바뀜
git-diff-side-unreadable = 한쪽 내용을 텍스트로 읽을 수 없다
