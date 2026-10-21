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
welcome-list-remote = 원격

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
