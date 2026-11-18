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
