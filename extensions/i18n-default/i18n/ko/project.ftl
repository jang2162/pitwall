### 프로젝트 창: 툴 윈도우 이름과 열기 액션, 상태바, 백그라운드 작업, 알림.

## 툴 윈도우 이름. id 는 툴 윈도우 id(src/layout/project-layout.ts 의 PANELS)를 kebab-case 로 쓴 것이다.

tool-window-editor = 에디터
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
editor-area-placeholder = 에디터 영역

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
