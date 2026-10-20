### 모든 창이 함께 쓰는 문자열: 액션, 액션 찾기, 테마 · 언어 고르기, 키맵 파일 오류, 다음에서 열기.

# 그 언어로 쓴 그 언어의 이름. 언어 고르기는 언어마다 그 언어의 이 메시지로 이름을 보여 준다.
language-name = 한국어

window-title-project = 프로젝트
window-title-settings = 설정

action-find = 액션 찾기
action-settings = 설정
action-cli-install = Shell 명령 설치
action-window-close = 창 닫기
# $theme: light | dark | system
action-theme =
    { $theme ->
        [light] 테마: 라이트
        [dark] 테마: 다크
       *[system] 테마: 시스템
    }
# $name: 그 언어로 쓴 그 언어의 이름(그 언어의 language-name)
action-language = 언어: { $name }
action-language-system = 언어: 시스템

find-action-label = 액션 찾기
find-action-placeholder = 액션 이름
find-action-empty = 맞는 액션이 없다
# $reason: OS 가 그 키를 가져가는 이유
find-action-reserved = OS 예약 키({ $reason }) — 앱에 오지 않는다

theme-switcher-label = 테마
theme-system = 시스템 테마
theme-light = 라이트 테마
theme-dark = 다크 테마

# $error: keymap.toml 의 문제
keymap-file-error = 키맵 파일 오류 — 이전 키맵을 쓰는 중: { $error }
# $warnings: 경고들을 "; " 로 이은 것
keymap-file-warning = 키맵 파일 경고: { $warnings }

## 설정 창의 외관과 「외부 도구 > Orca 에서 Pitwall 열기」(docs/design/04-features.md F8-6). 키는 apps/desktop/src/settings/strings.ts 가 잇는다.

settings-appearance = 외관
settings-theme = 테마
settings-language = 언어
settings-language-system = 시스템
# $error: 설정을 저장하지 못한 이유
settings-language-failed = 언어를 바꾸지 못했다: { $error }

settings-external-tools = 외부 도구
settings-orca-title = Orca 에서 Pitwall 열기
settings-orca-intro = Orca 의 Settings → General → Open in menu 에 앱을 추가하고 아래 값을 적는다. Orca 는 워크트리 폴더나 파일의 절대 경로를 명령 뒤에 붙여 실행한다.
settings-orca-label = 라벨
settings-orca-command = 명령
settings-orca-copy = 복사
# $value: 복사할 값
settings-orca-copy-value = { $value } 복사
settings-orca-copied = 복사했다
# $error: 클립보드에 쓰지 못한 이유
settings-orca-copy-failed = 복사하지 못했다: { $error }
settings-orca-checking = 검사하는 중…
# $error: 검사가 실패한 이유
settings-orca-check-failed = 검사하지 못했다: { $error }
settings-orca-recheck = 다시 검사
settings-orca-installed = 설치됨
settings-orca-not-installed = 설치 안 됨
settings-orca-other-link = 다른 곳을 가리킴
settings-orca-not-a-link = 설치할 수 없음
settings-orca-on-path = PATH 에 있음
settings-orca-not-on-path = PATH 에 없음
settings-orca-other-on-path = PATH 의 pitwall 이 다른 것
# 설치 위치에는 없지만 PATH 의 pitwall 이 이미 이 앱이다
settings-orca-works-from-path = PATH 에 있음 (설치 위치 밖)
# $target: 설치 위치(/usr/local/bin/pitwall)
settings-orca-detail-installed = { $target } 가 이 앱을 가리킨다.
settings-orca-detail-missing = { $target } 에 Shell 명령이 없다.
# $points: 링크가 가리키는 경로
settings-orca-detail-other-link = { $target } 가 다른 곳을 가리킨다: { $points }
settings-orca-detail-not-a-link = { $target } 에 Pitwall 이 만들지 않은 파일이 있다. 「Shell 명령 설치」 는 이것을 바꾸지 않는다 — 직접 치운 뒤 다시 검사한다.
# $path: PATH 에서 찾은 pitwall 의 경로
settings-orca-detail-works-from-path = { $path } 가 이 앱을 띄우고 Orca 는 이것을 실행한다. { $target } 에 Shell 명령을 설치하지 않아도 된다.
# $path: PATH 에서 찾은 pitwall 의 경로
settings-orca-detail-other-on-path = PATH 에서 먼저 찾는 것은 { $path } 다. Orca 는 이것을 실행한다.
# $resolved: 그 경로의 심링크를 푼 실제 경로
settings-orca-detail-resolved = 실제 파일: { $resolved }
settings-orca-install = Shell 명령 설치
settings-orca-installing = 설치하는 중…
# $app: 이 앱 바이너리, $target: 설치 위치
settings-orca-source-development-build = 개발 빌드({ $app })는 { $target } 에 설치하지 않는다 — 이 Mac 의 pitwall 명령이 이 빌드를 가리키게 되고, 빌드를 지우면 끊긴다. 설치를 확인하려면 PITWALL_SHELL_COMMAND_TARGET 에 임시 폴더 안의 절대 경로를 주고 앱을 띄운다.
# $app: 이 앱 바이너리
settings-orca-source-transient = Pitwall 이 { $app } 에서 돌고 있다 — 디스크 이미지를 꺼내거나 Mac 을 재시작하면 사라지는 곳이다. Applications 폴더로 옮겨 연 뒤 설치한다.
# $target: 설치 위치
settings-orca-absolute = Orca 가 PATH 로 이 앱을 찾지 못한다. 명령 칸에 절대 경로 { $target } 를 적는다.
# $reason: 로그인 셸 환경을 읽지 못한 이유
settings-orca-path-fallback = 로그인 셸 환경을 읽지 못해 앱의 PATH 로 찾았다 — Orca 가 보는 PATH 와 다를 수 있다({ $reason }).
settings-orca-worktree = Orca 가 만든 워크트리에는 커밋하지 않은 .pitwall/*.local.toml 이 없다. Pitwall 은 그때 메인 저장소의 것을 읽는다. 워크트리에도 복사하려면 저장소 루트의 .worktreeinclude 에 .pitwall/project.local.toml 같은 리터럴 경로를 적는다(Orca 는 glob 을 받지 않는다).

## 다음에서 열기(⌥F1)와 기본 외부 도구로 열기(⌘⌥E).

action-open-with = 다음에서 열기…
action-open-with-default = 기본 외부 도구로 열기

open-with-label = 다음에서 열기
# $project: 프로젝트 이름
open-with-title = 다음에서 열기: { $project }
open-with-placeholder = 도구 이름
open-with-empty = 맞는 도구가 없다
open-with-loading = 도구 목록을 읽는 중…
open-with-default-badge = 기본
# $reason: open-with-problem-* 문장 하나
open-with-unavailable = 쓸 수 없음: { $reason }
open-with-no-default = 기본 도구가 정해지지 않았다. 여기서 고르거나 settings.toml 의 [open_with] default 를 정한다.
# $tool: 설정이 고른 도구 id
open-with-unknown-default = 기본 도구 「{ $tool }」 는 이 시스템에서 쓸 수 없다. 여기서 고른다.
# $tool: 도구 이름, $reason: open-with-problem-* 문장 하나
open-with-failed = { $tool } 로 열지 못했다: { $reason }
# $command: 실행한 명령줄(터미널에서 직접 해 볼 수 있게)
open-with-command = 명령: { $command }
open-with-dismiss = 닫기
open-with-default-tool = 기본 외부 도구
open-with-list-failed = 외부 도구 목록을 읽지 못했다: { $error }
open-with-problem-unknown-tool = 이 시스템에서 쓸 수 없는 도구다
# $message: settings.toml 의 문제
open-with-problem-invalid-tool = 설정이 잘못됐다({ $message })
# $variable: 중괄호 없는 템플릿 변수 이름(예: branch)
open-with-problem-missing-variable = 명령이 쓰는 {"{"}{ $variable }{"}"} 의 값이 여기에는 없다
# $path: 프로젝트 폴더
open-with-problem-project-root-missing = 프로젝트 폴더 { $path } 가 없다
# $program: 실행 파일 이름, $source: login | process(찾아본 PATH)
open-with-problem-not-found =
    { $source ->
        [login] 「{ $program }」 를 로그인 셸 PATH 에서 찾지 못했다
       *[process] 「{ $program }」 를 앱의 PATH 에서 찾지 못했다(로그인 셸 PATH 를 읽지 못했다)
    }
# $path: 번들 폴더
open-with-problem-bundle-path = { $path } 는 앱 번들이라 「open」 이 보여 주지 않고 실행한다 — 실행하지 않았다
# $program: 실행 파일 경로, $message: OS 오류
open-with-problem-spawn-failed = { $program } 를 띄우지 못했다({ $message })
# $code: 종료 코드, 신호로 끝났으면 "signal"
open-with-problem-exited =
    { $code ->
        [signal] 명령이 신호로 끝났다
       *[other] 명령이 코드 { $code } 로 끝났다
    }
