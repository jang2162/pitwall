### 네이티브 메뉴(apps/desktop/src-tauri/src/menu.rs). Rust 가 그린다. $app 은 앱 이름이다.

menu-app = Pitwall
menu-file = 파일
menu-edit = 편집
menu-window = 윈도우
menu-help = 도움말

menu-about = { $app } 정보
menu-settings = 설정…
menu-install-shell-command = Shell 명령 설치
menu-services = 서비스
menu-hide = { $app } 가리기
menu-hide-others = 기타 가리기
menu-show-all = 모두 보기
menu-quit = { $app } 종료

menu-welcome-window = 웰컴 창
# ⌘W(키맵 액션 window.close): 에디터 탭이 있으면 탭, 없으면 창을 닫는다.
menu-close = 닫기

menu-undo = 실행 취소
menu-redo = 실행 복귀
menu-cut = 오려두기
menu-copy = 복사하기
menu-paste = 붙여넣기
menu-select-all = 전체 선택

menu-minimize = 최소화
menu-maximize = 확대/축소
menu-fullscreen = 전체 화면 전환

menu-find-action = 액션 찾기…
menu-copy-diagnostics = 진단 정보 복사
menu-check-updates = 업데이트 확인…

## 「Shell 명령 설치」 결과 대화상자(apps/desktop/src-tauri/src/shell_command.rs). $target: 명령을 설치한 자리.

shell-command-title = Shell 명령 설치
shell-command-installed = 'pitwall' 명령을 { $target } 에 설치했다.

    터미널에서 `pitwall <경로>` 를 실행하면 Pitwall 에서 연다.
shell-command-already = { $target } 의 'pitwall' 명령이 이미 이 앱을 연다.
# $previous: 옛 링크가 가리키던 것
shell-command-replaced = 'pitwall' 명령을 { $target } 에 설치했다.

    { $previous } 를 가리키던 링크를 바꿨다.
# $error: 실패한 이유
shell-command-failed = 'pitwall' 명령을 설치하지 못했다.

    { $error }

## 업데이트 대화상자(apps/desktop/src-tauri/src/update.rs). $app: 앱 이름, $current: 실행 중인 버전, $version: 새 버전.

update-title = 소프트웨어 업데이트
update-available = { $app } { $version } 이 나왔다. 지금 버전은 { $current } 이다.

    설치하고 { $app } 를 다시 시작할까?
update-install = 설치하고 다시 시작
update-later = 나중에
update-none = { $app } { $current } 가 최신 버전이다.
# $error: 실패한 이유
update-check-failed = 업데이트를 확인하지 못했다.

    { $error }
# $error: 실패한 이유
update-install-failed = 업데이트를 설치하지 못했다.

    { $error }
# deb · rpm 설치본은 스스로 업데이트하지 않는다.
update-package-manager = { $app } { $current } 는 시스템 패키지(deb · rpm)로 설치되어 스스로 업데이트하지 않는다.

    패키지 관리자로 업데이트하거나 릴리스 페이지에서 새 패키지를 받는다.
update-dev-build = 개발 빌드는 업데이트를 확인하지 않는다.
