### 모든 창이 함께 쓰는 문자열: 액션, 액션 찾기, 테마 · 언어 고르기, 키맵 파일 오류.

# 그 언어로 쓴 그 언어의 이름. 언어 고르기는 언어마다 그 언어의 이 메시지로 이름을 보여 준다.
language-name = 한국어

window-title-project = 프로젝트
window-title-settings = 설정

action-find = 액션 찾기
action-settings = 설정
action-cli-install = Shell 명령 설치
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
