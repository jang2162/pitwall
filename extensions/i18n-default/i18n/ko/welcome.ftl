### 웰컴 창(S1 · S2 · S3, docs/design/10-ui.md §2.1~2.3). 키는 apps/desktop/src/welcome/strings.ts 가 화면에 잇는다.

welcome-nav-label = 웰컴
welcome-nav-projects = 프로젝트
welcome-nav-remote-hosts = 원격 호스트
welcome-nav-extensions = 확장
# 아직 없는 탭에 붙는 설명.
welcome-nav-not-yet = 아직 없다

welcome-list-label = 프로젝트 목록
welcome-list-search = 프로젝트 검색
welcome-list-empty = 등록된 프로젝트가 없다. 새 프로젝트를 만들거나 폴더를 연다.
welcome-list-no-match = 맞는 프로젝트가 없다
welcome-list-favorite = 즐겨찾기
# 행의 현재 브랜치(스크린 리더 이름 · 툴팁). git 을 실행하지 않고 HEAD 를 읽는다(git.projectBranches)
welcome-list-branch = 브랜치 { $name }

welcome-button-new-project = 새 프로젝트…
welcome-button-open-folder = 폴더 열기…

welcome-menu-open = 열기
# $platform: mac | win | linux
welcome-menu-reveal =
    { $platform ->
        [mac] Finder 에서 보기
        [win] 탐색기에서 보기
       *[other] 파일 관리자에서 보기
    }
welcome-menu-remove = 목록에서 제거

welcome-action-new-project = 새 프로젝트
welcome-action-open-folder = 폴더 열기
welcome-action-open = 선택한 프로젝트 열기
# $platform: mac | win | linux
welcome-action-reveal =
    { $platform ->
        [mac] 선택한 프로젝트를 Finder 에서 보기
        [win] 선택한 프로젝트를 탐색기에서 보기
       *[other] 선택한 프로젝트를 파일 관리자에서 보기
    }
welcome-action-remove = 선택한 프로젝트를 목록에서 제거
welcome-action-create = 프로젝트 만들기

welcome-drop = 폴더를 놓으면 새 프로젝트를 만든다

# $reason: 레지스트리 파일을 새로 만든 이유, $backup: 옛 파일 경로
welcome-storage-recovered = 프로젝트 목록 파일이 손상돼 새로 만들었다({ $reason }). 옛 파일: { $backup }
welcome-storage-read-only = 프로젝트 목록을 읽기만 한다 — 바꿀 수 없다({ $reason })
welcome-storage-memory = 프로젝트 목록 파일을 열지 못했다 — 목록이 비어 보이고 바꿀 수 없다({ $reason })

welcome-find-label = 프로젝트 찾기
# $path: 열려는 파일 · 폴더
welcome-find-title = { $path } 를 열 프로젝트
welcome-find-search = 폴더 경로
welcome-find-empty = 맞는 후보가 없다
welcome-find-loading = 후보를 찾는 중…
welcome-find-recommended = 추천
welcome-find-worktree-main = 워크트리의 메인 저장소
welcome-find-pick-folder = 폴더 직접 선택…
welcome-find-file-only = 프로젝트 없이 파일만 열기

# $name: 만들 프로젝트 이름
welcome-create-title = 새 프로젝트: { $name }
welcome-create-name = 이름
welcome-create-root = 루트
welcome-create-change-root = 변경
welcome-create-choose-root = 선택…
welcome-create-root-none = 고르지 않음
welcome-create-config-mode = 설정 저장
welcome-create-repository = .pitwall/ 에 저장(커밋)
welcome-create-personal = 개인 전용
welcome-create-detected = 감지됨
welcome-create-detected-empty = 감지된 항목이 없다
welcome-create-cancel = 취소
welcome-create-submit = 만들기

# $path: 열려던 파일
welcome-notice-file-not-in-project = 만든 프로젝트가 { $path } 를 담지 않아 프로젝트 창만 열었다

# $what: 아래 welcome-error-* 중 하나, $message: 오류 문장
welcome-error-action = { $what } 실패: { $message }
welcome-error-open = 프로젝트 열기
welcome-error-remove = 목록에서 제거
welcome-error-pick-folder = 폴더 선택
# $platform: mac | win | linux
welcome-error-reveal =
    { $platform ->
        [mac] Finder 에서 보기
        [win] 탐색기에서 보기
       *[other] 파일 관리자에서 보기
    }
welcome-error-create = 프로젝트 만들기
welcome-error-find-project = 프로젝트 찾기 요청 읽기
welcome-error-open-file = 파일 열기
welcome-error-candidates = 루트 후보 찾기

# 원격 호스트 탭(S8 — 18-m7-plan M7-08). 목록은 설정의 [[remote_hosts]] 와 ~/.ssh/config 의 Host 이름이다.
welcome-remote-label = 원격 호스트 목록
welcome-remote-empty = 원격 호스트가 없다. ~/.ssh/config 에 Host 를 적거나 설정(settings.toml)에 [[remote_hosts]] 를 더한다.
welcome-remote-add-hint = 원격 호스트는 ~/.ssh/config 의 Host 와 설정의 [[remote_hosts]] 에서 온다 — 고른 뒤 [연결].
welcome-remote-source-settings = 설정
welcome-remote-source-ssh-config = ssh config
welcome-remote-connect = 연결
welcome-remote-disconnect = 끊기
welcome-remote-retry = 다시 시도
welcome-remote-show-log = 연결 로그
welcome-remote-hide-log = 로그 닫기
welcome-remote-trust = 이 머신을 새로 믿기
welcome-remote-stages = 연결 단계
welcome-remote-stage-ssh = SSH
welcome-remote-stage-install = 설치
welcome-remote-stage-daemon = 데몬
welcome-remote-stage-handshake = 핸드셰이크
welcome-remote-status-idle = 끊김
welcome-remote-status-by-user = 끊음
# $stage: 지금 단계 이름
welcome-remote-status-connecting = 붙는 중 — { $stage }
welcome-remote-status-connected = 연결됨
# $seconds: 다음 시도까지 초, $attempt: 몇 번째 시도
welcome-remote-status-reconnecting = 다시 붙는 중 — { $seconds }초 뒤 ({ $attempt }번째)
# $expected: 기억한 머신 id, $found: 지금 답한 머신 id
welcome-remote-status-machine-changed = 다른 머신이 답했다 — 기억한 { $expected }, 지금 { $found }. SSH 호스트 키가 바뀐 것과 같다 — 그 머신이 맞을 때만 새로 믿는다.
# $stage: 실패한 단계 이름
welcome-remote-failed = { $stage } 단계에서 실패
welcome-remote-failure-host-key-changed = 호스트 키가 바뀌었다 — ssh 가 거절했다. known_hosts 는 직접 고친다.
welcome-remote-failure-host-key-not-accepted = 호스트 키를 받지 않았다.
welcome-remote-failure-ssh = ssh 가 붙지 못했다(연결 · 인증).
welcome-remote-failure-unsupported = 지원하지 않는 원격 플랫폼이다 — 아무것도 올리지 않았다.
welcome-remote-failure-install = Relay 설치 · 페어링이 실패했다.
welcome-remote-failure-version-mismatch = Relay 의 판이 이 앱과 다르다.
welcome-remote-failure-rejected = Relay 가 이 기기를 받지 않았다.
welcome-remote-failure-link = 링크가 서지 않았거나 끊겼다.
welcome-remote-failure-already-connected = 이 머신은 다른 호스트 이름으로 이미 붙어 있다.
welcome-remote-failure-unavailable = 원격 호스트를 쓸 수 없다.
# $when: 마지막 링크 상태의 시각
welcome-remote-last-connected = 마지막: 연결됨 ({ $when })
welcome-remote-last-disconnected = 마지막: 끊김 ({ $when })
welcome-remote-last-failed = 마지막: 실패 ({ $when })
welcome-remote-master = SSH 마스터 연결 있음
# $name: 호스트 이름
welcome-remote-log-title = { $name } 연결 로그
welcome-remote-log-empty = 아직 기록이 없다
