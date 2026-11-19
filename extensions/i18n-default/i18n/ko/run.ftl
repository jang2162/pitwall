# 「실행」 툴 윈도우 · 콘솔 · 상태바 실행 수 · 닫기 확인(17-m6-plan M6-06, src/run).
# M6-07 · M6-08 · M6-09 · M6-18 이 실행 화면의 문구를 여기에 더한다.

## 「실행」 툴 윈도우

run-tabs-label = 실행 탭
run-empty = 아직 실행한 것이 없다. 실행 선택기에서 구성을 띄운다.
run-no-host = 실행을 보일 수 없다: Host 에 연결되지 않았다
# $state: running, stopping, exited
run-state =
    { $state ->
        [running] 실행 중
        [stopping] 끝내는 중
       *[exited] 끝남
    }
# $code: 끝 코드
run-exit-code = 끝 { $code }
# $signal: 실행을 끝낸 신호(예: 「Terminated: 15」)
run-exit-signal = { $signal }
run-launched-by-window = 이 창에서 띄움
# $client: 에이전트의 클라이언트 이름(예: 「claude-code」)
run-launched-by-agent = 에이전트가 띄움({ $client })
run-agent-unknown = 알 수 없는 클라이언트
# $time: 시작 시각
run-started-at = { $time } 시작
# $name: 실행 이름
run-close-tab = 「{ $name }」 닫기
run-rerun = 다시 실행
run-stop = 중지
run-scroll-lock = 스크롤 고정
run-clear = 콘솔 지우기
run-search = 찾기
run-search-input = 콘솔에서 찾기
run-search-next = 다음 찾기
run-search-previous = 이전 찾기
run-search-close = 찾기 닫기
run-stop-failed = 실행을 중지하지 못했다
run-rerun-failed = 다시 실행하지 못했다
# $code: 끝 코드, 신호로 끝났으면 none. $signal: 신호 이름($code 가 none 일 때)
run-exited-line =
    { $code ->
        [none] [프로세스가 { $signal } 로 끝남]
       *[other] [프로세스가 코드 { $code } 로 끝남]
    }
# $bytes: 바이트 수
run-dropped-line = [앞 출력 { $bytes } 바이트가 버려졌다]
# $message: Host 가 알린 까닭
run-attach-failed = [출력을 보이지 못했다: { $message }]

## 포트 칩(M6-08 의 포트, src/run/run-ports.tsx 가 그린다)

# $port: TCP 포트 번호
run-port-label = :{ $port }
# $url: 칩이 여는 주소(http://localhost:<port>, IPv6 에만 듣으면 http://[::1]:<port>)
run-port-open = { $url } 열기
# 모든 주소(0.0.0.0 · ::)에 듣는 포트 — 다른 기기에서도 닿는다
run-port-all-addresses = 모든 주소

## 상태바

# $count: 이 프로젝트에서 실행 중인 것
run-status-label = 실행 { $count }
# $count: 위와 같다
run-status-hint = 이 프로젝트에서 { $count }개 실행 중 — 눌러서 「실행」 툴 윈도우 열기

## 실행이 남은 채 프로젝트 창을 닫을 때(M6-06 ⑥)

# $count: 아직 실행 중인 것
run-close-title = 실행 중 { $count }개
run-close-body = 이 창을 닫으면 그 실행과 자식 프로세스를 끝낸다.
run-close-human = 사람이 띄움
# $client: 에이전트의 클라이언트 이름
run-close-agent = 에이전트가 띄움({ $client })
run-close-confirm = 끝내고 닫기
run-close-cancel = 취소

## 실행 선택기 · 액션(M6-07, src/run/run-picker.tsx · run-actions.ts)

run-picker-label = 실행 구성
run-picker-none = 구성 없음
# $name: 구성 이름, $profile: 프로필 이름
run-picker-with-profile = { $name } · { $profile }
run-picker-choose = 실행 구성 고르기
run-picker-search = 구성 · 태스크 찾기
run-picker-empty = 맞는 구성이 없다
run-picker-group-configs = 구성
run-picker-group-detected = 감지됨
run-picker-group-unusable = 쓸 수 없음
run-picker-group-more = 더 보기
# $detector: 감지기(npm · make · gradle …), $source: 감지 출처 파일 이름
run-picker-detected-from = { $detector } · { $source }
run-picker-save-detected = 구성으로 저장
run-picker-edit = 구성 편집…
run-picker-new = 새 구성…
# $detector: gradle · maven · mise
run-picker-read-tasks = { $detector } 태스크 읽기
# $detector: 위와 같다
run-picker-reread-tasks = { $detector } 태스크 다시 읽기
# $detector: 위와 같다, $message: 까닭
run-picker-read-failed = { $detector } 태스크를 읽지 못했다: { $message }
run-picker-run = 실행
run-picker-debug = 디버그
run-picker-debug-none = 이 구성에는 [debug] 가 없다
# $message: 오류 원문
run-picker-debug-failed = 디버그를 시작하지 못했다: { $message }
run-picker-stop = 중지
# $reason: 쓸 수 없는 까닭(Host 의 글)
run-picker-unusable-reason = 쓸 수 없음: { $reason }
# $message: Host 가 알린 까닭
run-picker-list-failed = 실행 구성을 읽지 못했다: { $message }
# $message: Host 가 알린 까닭
run-start-failed = 실행하지 못했다: { $message }
run-action-current = 실행(현재 구성)
run-action-debug = 디버그(현재 구성)
run-action-choose = 실행 구성 골라 실행
run-action-stop = 실행 중지
run-action-edit = 실행 구성 편집

## 첫 실행 확인(M6-07 ⑥, src/run/run-confirm-dialog.tsx)

# $name: 구성 이름
run-confirm-title = { $name } 을(를) 처음 실행한다
run-confirm-body = 이 내용으로 실행한 적이 없다 — 에이전트가 쓰거나 밖에서 바뀐 구성일 수 있다. 실행될 내용을 보고 고른다.
run-confirm-kind = 종류
run-confirm-command = 명령
run-confirm-definition = 실행할 정의
run-confirm-cwd = 작업 폴더
run-confirm-profile = 프로필
run-confirm-env = 환경 변수
run-confirm-env-none = 없음
run-confirm-env-files = .env 파일
run-confirm-masked = 비밀로 보여 값을 가렸다
run-confirm-secret-warning = 명령에 비밀처럼 보이는 값(비밀번호가 든 주소 · 비밀 이름의 값)이 있다. 가리지 않고 그대로 보였다 — 무엇이 도는지 확인한다.
run-confirm-not-recorded = 이 구성은 내용을 기록할 수 없어 실행할 때마다 묻는다.
run-confirm-order = 차례:
run-confirm-run = 실행
run-confirm-cancel = 취소

## 구성 편집(M6-07 ③, src/run/config-editor.tsx)

run-editor-title-new = 새 실행 구성
# $name: 구성 이름
run-editor-title-edit = 실행 구성 편집 — { $name }
run-editor-id = id(파일 이름)
run-editor-id-hint = 영문자 · 숫자 · - · _, 64자까지
run-editor-name = 이름
run-editor-type = 종류
run-editor-cwd = 작업 폴더
run-editor-cwd-hint = 프로젝트 루트 기준. 비우면 루트
run-editor-env = 환경 변수
run-editor-env-name = 이름
run-editor-env-value = 값
run-editor-env-add = 변수 더하기
# $name: 변수 이름
run-editor-env-remove = { $name } 빼기
run-editor-env-file = .env 파일
run-editor-env-file-hint = 프로젝트 루트 기준, 한 줄에 하나(차례대로 얹는다)
run-editor-before = 먼저 실행
run-editor-before-hint = 구성 id, 한 줄에 하나 — 차례대로 끝 코드 0 으로 끝나야 이 구성을 띄운다
run-editor-list-hint = 한 줄에 하나
run-editor-location = 저장 위치
run-editor-location-shared = 공유(.pitwall/run/<id>.toml — 커밋된다)
run-editor-location-local = 개인(<id>.local.toml — 이 기기만)
run-editor-form = 폼
run-editor-toml = TOML 보기
run-editor-toml-hint = 저장하면 이 내용이 된다(읽기 전용)
# $path: 쓸 파일
run-editor-toml-path = { $path }
# $message: 미리보기를 만들지 못한 까닭
run-editor-toml-failed = 미리보기를 만들지 못했다: { $message }
# $field: 칸 이름
run-editor-required = { $field } 을(를) 채운다
run-editor-save = 저장
run-editor-cancel = 취소
run-editor-delete = 지우기
# $message: Host 가 알린 까닭
run-editor-save-failed = 저장하지 못했다: { $message }
# $message: Host 가 알린 까닭
run-editor-delete-failed = 지우지 못했다: { $message }
run-editor-kept = 프로필 · [debug] 같은 폼 밖의 값은 그대로 둔다.

## 설정 창 「실행」(M6-07 ③)

run-settings-title = 실행
run-settings-body = 실행 구성은 프로젝트마다 다르다. 프로젝트 창 타이틀바의 실행 구성 선택(▾) → 「구성 편집…」 에서 만들고 고친다.

## 디버그(M6-18 — 「디버그」 툴 윈도우 · 브레이크포인트 거터 · 편집 · 목록, src/debug/)

debug-source-outside = 프로젝트 밖
debug-source-held = 디버거가 쥔 소스
# $adapter: 디버그 어댑터 id, $name: 어댑터가 그 소스에 준 이름(경로가 아니다)
debug-source-adapter = { $adapter } 소스 · { $name }
# $path: 실제 경로와 다를 때 프로그램이 보고한 경로
debug-source-reported = 보고된 경로 { $path }
debug-no-host = Host 에 연결되지 않았다
debug-no-session = 디버그 세션이 없다. 디버그 단추나 「디버그(현재 구성)」 로 시작한다.
debug-sessions-label = 디버그 세션
# $state: running · stopped · ended
debug-state = { $state ->
    [running] 실행 중
    [stopped] 멈춤
   *[ended] 끝남
}
# $reason: 어댑터가 준 까닭(breakpoint · step · exception · pause …) — 그대로 보인다
debug-stopped-reason = 멈춤: { $reason }
debug-resume = 계속
debug-pause = 일시 정지
debug-step-over = 넘기기
debug-step-into = 들어가기
debug-step-out = 나오기
debug-stop = 중지
debug-debugger = 디버거
debug-frames = 프레임
debug-threads = 스레드
debug-variables = 변수
debug-watches = 워치
debug-console = 콘솔
debug-no-frames = 프로그램이 멈추면 프레임이 보인다.
debug-no-variables = 프레임을 고르면 변수가 보인다.
debug-outside-frame = 프로젝트 밖 — 읽기 전용으로 연다
debug-add-watch = 워치 식 더하기…
debug-remove-watch = 워치 지우기
debug-console-input = 멈춘 프로그램 안에서 평가할 식
debug-console-hint = Enter 는 그 식을 프로그램 안에서 돌린다. ↑ ↓ 로 기록을 오간다.
# $count: 다음 쪽이 더 읽는 자식 수
debug-more = { $count }개 더…
debug-value-cut = (잘림 — 값이 더 길다)
debug-too-deep = (너무 깊어 보이지 않는다)
# $message: 오류 원문
debug-read-failed = 읽지 못했다: { $message }
debug-start-failed = 디버그를 시작하지 못했다
debug-control-failed = 디버거가 하지 못했다
# $file: 파일 이름, $line: 줄 번호
debug-bp-title = 브레이크포인트 · { $file }:{ $line }
debug-bp-enabled = 켬
debug-bp-condition = 조건(참일 때만 멈춘다)
debug-bp-log-message = 로그 메시지(멈추지 않는다 — { "{" }식{ "}" } 은 평가된다)
debug-bp-hit-condition = hit count
debug-bp-remove = 지우기
debug-bp-done = 닫기
debug-bp-plain = 브레이크포인트
# $text: 적은 식 · 메시지 그대로
debug-bp-describe-condition = 조건: { $text }
debug-bp-describe-log = 로그: { $text }
debug-bp-describe-hit = hit count: { $text }
debug-bp-disabled = 꺼 둠
# $message: 어댑터가 확인하지 못한 까닭(빌 수 있다)
debug-bp-unverified = 디버거가 확인하지 못했다 { $message }
debug-bp-agent = 에이전트가 둠
debug-bp-agent-expression = 에이전트가 둠 — 그 식은 프로그램 안에서 돈다
debug-bp-list-title = 브레이크포인트
debug-bp-list-empty = 브레이크포인트가 없다
debug-bp-list-unsaved = 저장 전 — 파일을 저장하면 디버거에 간다
# $message: Host 의 까닭(영어)
debug-bp-list-problem = 저장된 브레이크포인트를 읽지 못했다: { $message }
action-debug-step-over = 넘기기
action-debug-step-into = 들어가기
action-debug-step-out = 나오기
action-debug-resume = 프로그램 계속
action-debug-pause = 프로그램 일시 정지
action-debug-toggle-breakpoint = 줄 브레이크포인트 토글
action-debug-view-breakpoints = 브레이크포인트 보기
action-debug-evaluate = 식 평가

## 사슬(before · 복합 구성 — run-chain.tsx)
run-chain-failed = { $config } 이(가) { $step } 에서 멈췄다 — 남은 것은 띄우지 않았다: { $message }
run-chain-stopped = { $config } 을(를) { $step } 에서 멈췄다 — 남은 것은 띄우지 않았다
