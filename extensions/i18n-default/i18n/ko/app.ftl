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

settings-appearance = 외관
settings-theme = 테마
settings-language = 언어
settings-language-system = 시스템
# $error: 설정을 저장하지 못한 이유
settings-language-failed = 언어를 바꾸지 못했다: { $error }

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
