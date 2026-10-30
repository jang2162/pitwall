### 커밋 툴 윈도우(⌘0, apps/desktop/src/git/commit): 바뀐 파일 그룹 · 체크 · 커밋 메시지 칸 · 훅 출력 · stash · diff 미리보기의
### hunk · 줄 체크(부분 스테이징).

git-commit-title = 커밋

## 위 막대

git-commit-refresh = 새로 고침
git-commit-rollback = 변경 되돌리기
git-commit-show-diff = diff 보기

## 그룹. 파일 체크 = 스테이징(스테이징됨 그룹은 체크돼 있고 누르면 스테이징을 푼다).

git-commit-group-conflicts = 충돌
git-commit-group-staged = 스테이징됨
git-commit-group-changes = 변경
git-commit-group-untracked = 버전 관리 안 됨
git-commit-group-stash = Stash
# $name: 그룹 이름, $count: 파일 수
git-commit-group-count = { $name } ({ $count })
git-commit-partial = 부분
git-commit-stage = 스테이징
git-commit-unstage = 스테이징 풀기
git-commit-stage-all = 모두 스테이징
git-commit-unstage-all = 모두 스테이징 풀기
git-commit-conflict-hint = 충돌은 머지 도구에서 푼다
git-commit-no-changes = 바뀐 파일이 없다
git-commit-no-repository = Git 저장소가 없다
git-commit-unsupported = 지원하지 않는 저장소
git-commit-no-project = 프로젝트 창에서만 쓸 수 있다

## 메시지 칸

git-commit-message = 커밋 메시지
git-commit-history = 메시지 기록
git-commit-history-empty = 기록이 없다
git-commit-amend = Amend
git-commit-signoff = Sign-off
git-commit-run = 커밋
git-commit-run-and-push = 커밋 후 푸시
git-commit-running = 커밋하는 중…
git-commit-nothing-staged = 스테이징된 변경이 없다
git-commit-empty-message = 커밋 메시지가 비었다
git-commit-template-unedited = 템플릿을 고치지 않았다 — 메시지를 쓴 뒤 커밋한다

## 커밋 결과 · 훅 출력

git-commit-output = 훅 출력
git-commit-output-failed = 커밋하지 않았다 — 훅 · git 의 출력
git-commit-output-close = 닫기
# $hash: 만든 커밋(짧은 해시)
git-commit-done = 커밋했다: { $hash }
# $previous: 고치기 전 커밋, $hash: 새 커밋(짧은 해시)
git-commit-amended = 고쳐 커밋했다: { $previous } → { $hash }
git-commit-push-unavailable = 푸시 다이얼로그가 아직 없다 — 커밋만 했다
# $message: 오류 원문
git-commit-failed = 실패: { $message }

## 변경 되돌리기(작업 트리 변경 버리기 — 되돌릴 수 없다)

git-commit-rollback-title = 변경 되돌리기
# $count: 파일 수
git-commit-rollback-body = 파일 { $count }개의 변경을 버린다. 스테이징된 것은 스테이징을 풀고 HEAD 로 돌린다. 되돌릴 수 없다.
git-commit-rollback-confirm = 되돌리기
# $count: 파일 수
git-commit-rollback-kept = 이 새 파일 { $count }개는 지우지 않고 디스크에 그대로 둔다:
# $path: 파일 경로
git-commit-rollback-kept-without-staged = { $path } — 디스크의 내용만 남고 스테이징한 내용은 잃는다
# $count: 서브모듈 수
git-commit-rollback-untouched = 서브모듈 { $count }개는 건드리지 않는다 — 가리키는 커밋과 그 안의 변경이 그대로 남는다:
git-commit-rollback-changed = 그사이 상태가 바뀌었다 — 새 목록을 확인한다
git-commit-rollback-unstable = 상태가 계속 바뀌어 되돌리지 않았다 — 잠시 뒤에 다시 한다
# $files: 줄마다 파일 하나
git-commit-rollback-untracked = 버전 관리 안 되는 파일은 지우지 않았다: { $files }
git-commit-rollback-none = 되돌릴 변경을 고르지 않았다
# $count: 더 있는 줄 수
git-commit-more = 그리고 { $count }개 더

## diff 미리보기의 hunk · 줄 체크

git-commit-hunk-stage = 이 묶음 스테이징
git-commit-hunk-unstage = 이 묶음 스테이징 풀기
git-commit-line-stage = 이 줄 스테이징
git-commit-line-unstage = 이 줄 스테이징 풀기
git-commit-hunk-revert = 이 묶음 되돌리기
git-commit-hunk-revert-title = 변경 묶음 되돌리기
git-commit-hunk-revert-body = 이 묶음의 작업 트리 변경을 버린다 — 아래의 + 줄은 파일에서 지워지고 - 줄이 돌아온다. 되돌릴 수 없다.
git-commit-hunk-changed = 그사이 파일이 바뀌었다 — diff 를 다시 읽었다

## Stash

git-commit-stash-create = Stash 만들기…
git-commit-stash-include-untracked = untracked 포함
git-commit-stash-name = Stash 메시지
git-commit-stash-hint = 비우면 git 의 기본 메시지
git-commit-stash-apply = 적용
git-commit-stash-pop = 꺼내기
git-commit-stash-drop = 버리기
git-commit-stash-drop-title = Stash 버리기
# $name: stash 이름(stash@{"{"}0{"}"} 메시지)
git-commit-stash-drop-body = { $name } 을(를) 버린다. 되돌릴 수 없다.
git-commit-stash-nothing = 담을 변경이 없다
git-commit-stash-conflicted = 적용했지만 충돌이 있다 — 충돌 그룹에서 푼다
git-commit-stash-changed = Stash 목록이 그사이 바뀌었다 — 다시 읽었다
git-commit-stash-empty = Stash 가 없다

## 액션(액션 찾기 · 키맵)

git-commit-action-open = 커밋…
git-commit-action-run = 커밋 실행
git-commit-action-run-and-push = 커밋 후 푸시
git-commit-action-rollback = 변경 되돌리기
