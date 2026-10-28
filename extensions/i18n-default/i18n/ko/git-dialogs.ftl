### Git 다이얼로그(apps/desktop/src/git/dialogs): 위험 작업 확인 · 푸시 · 풀 · fetch · 브랜치 이름 · [되돌리기] · 설정 창 「Git」.

action-git-push = 푸시…
action-git-pull = 프로젝트 업데이트…
action-git-fetch = 가져오기(Fetch)

## 함께 쓰는 것.

git-dialog-cancel = 취소
git-dialog-danger = Pitwall 에서 되돌릴 수 없다
git-dialog-no-project = Git 은 프로젝트 창에서만 쓴다
git-dialog-no-repository = 이 프로젝트는 Git 저장소에 있지 않다
# $what: 실패한 작업
git-dialog-failed = { $what } 실패

## reset · 되돌리기가 잃을 것(git.resetPreview).

# $count: 잃을 변경 수
git-lost-count = { $count ->
        [one] 이 변경을 잃는다:
       *[other] 이 변경 { $count }개를 잃는다:
    }
# $count: 보이지 않은 줄 수
git-lost-more = …그리고 { $count }개 더
git-lost-nothing = 잃는 것이 없다
git-lost-unverifiable = 잃을 것이 너무 많아 다 보일 수 없으므로 할 수 없다
git-lost-changed = 작업 트리가 바뀌었다 — 새 목록을 확인한다

## 브랜치 만들기 · 이름 바꾸기 · 지우기.

git-branch-name = 브랜치 이름
git-branch-create-title = 새 브랜치
# $from: 시작할 곳
git-branch-create-from = { $from } 에서
git-branch-create-checkout = 만든 뒤 체크아웃
git-branch-create = 만들기
# $name: 지금 브랜치 이름
git-branch-rename-title = { $name } 이름 바꾸기
git-branch-rename = 이름 바꾸기
git-branch-name-empty = 브랜치 이름을 넣는다
git-branch-name-invalid = Git 이 받지 않는 브랜치 이름이다
git-branch-name-taken = 같은 이름의 브랜치가 이미 있다
# $name: 브랜치
git-branch-force-title = 병합되지 않은 { $name } 을(를) 지울까?
# $name: 브랜치, $commit: 그 끝의 짧은 해시
git-branch-force-body = { $name }({ $commit })에 병합되지 않은 커밋이 있다. Git 이 지우기를 거절했다:
git-branch-force-confirm = 강제 삭제
# $name: 브랜치, $commit: 가리키던 짧은 해시
git-branch-deleted = { $name } 을(를) 지웠다({ $commit } 였다)

## ref 를 옮긴 작업의 [되돌리기].

git-undo = 되돌리기
# $ref: 브랜치, $from / $to: 짧은 해시(없었으면 ∅)
git-undo-moved = { $ref }: { $from } → { $to }
# $commit: 돌아갈 짧은 해시
git-undo-title = { $commit } 로 되돌리기
git-undo-confirm = 되돌리기
# $ref: 브랜치
git-undo-done = { $ref } 의 변경을 되돌렸다
# $name: 브랜치
git-undo-restored = { $name } 을(를) 되살렸다
# $name: 브랜치
git-undo-removed = { $name } 을(를) 지웠다
git-undo-failed = 되돌리기 실패
# $name: 브랜치, $commit: 지금 가리키는 짧은 해시(없으면 ∅)
git-undo-moved-since = { $name } 이(가) 그사이 { $commit } 로 움직였다 — 되돌리지 않는다

## reset 확인.

# $mode: soft · mixed · keep · hard, $commit: 짧은 해시
git-reset-title = { $commit } 로 reset --{ $mode }
git-reset-confirm = Reset

## 푸시 다이얼로그(⌘⇧K).

git-push-title = 푸시
git-push-loading = 보낼 곳을 셈하는 중…
# $branch: 로컬 브랜치, $remote: 원격 이름, $remoteRef: 원격의 ref
git-push-target = { $branch } → { $remote } : { $remoteRef }
# $source: 대상을 정한 것
git-push-source = { $source ->
        [input] 여기서 고른 대상
        [pushRefspec] 원격의 push 설정이 정한 대상
       *[pushDefault] push.default 가 정한 대상
    }
git-push-terminal = 터미널의 git push 는 다르게 한다:
# $value: push.default
git-push-terminal-default = push.default = { $value }
# $refspecs: 설정의 push refspec
git-push-terminal-configured = 설정의 push: { $refspecs }
# $sends: git 이 보낼 것
git-push-terminal-sends = git 이 보낼 것: { $sends }
git-push-terminal-nothing = git 은 아무것도 보내지 않는다
# $message: git 의 거절 이유
git-push-terminal-refuses = git 은 거절한다: { $message }
# $reason: git 이 push 를 거절할 이유
git-push-refused = { $reason ->
        [nothing] push.default 가 「nothing」 이라 git 은 아무 데도 보내지 않는다.
        [matching] push.default 가 「matching」 이다 — Pitwall 은 맞는 브랜치를 모두 보내는 push 를 하지 않는다.
        [noUpstream] 이 브랜치에 upstream 이 없다.
        [upstreamNameDiffers] upstream 브랜치의 이름이 다르다.
        [notUpstreamRemote] 이 원격은 브랜치의 upstream 원격이 아니다.
        [multipleUpstreams] 브랜치의 upstream 이 여럿이다.
        [notPushedByConfig] 원격의 push 설정이 이 브랜치를 보내지 않는다.
        [mirror] 원격이 mirror 다.
        [invalidConfig] git 이 받지 않는 push 설정이다.
       *[unqualifiedDestination] 설정의 대상이 짧은 이름이라 git 은 원격의 ref 에서 고른다.
    }
git-push-candidates = 보낼 곳을 고른다:
# $count: 보낼 커밋 수
git-push-commits = 보낼 커밋 { $count }개
git-push-commits-more = …나머지는 보이지 않는다
git-push-no-commits = 새 커밋이 없다 — ref 만 보낸다
git-push-set-upstream = upstream 으로 정하기
git-push-force = 강제 푸시(force-with-lease)
git-push-force-unavailable = 원격에 아직 이 브랜치가 없다 — 덮을 것이 없다
git-push-force-title = 강제 푸시할까?
# $remoteRef: 원격의 ref, $commit: 마지막으로 본 짧은 해시
git-push-force-body = 원격의 { $remoteRef }(마지막으로 본 곳 { $commit })를 덮는다. 원격에만 있는 커밋을 잃는다. 원격이 그 사이 움직였으면 git 이 거절하고 아무것도 바뀌지 않는다(--force-with-lease).
git-push-force-confirm = 강제 푸시
git-push-send = 푸시
git-push-sending = 푸시하는 중…
git-push-changed = 보인 뒤에 보낼 곳이 바뀌었다 — 새 대상을 확인한다
git-push-rejected = git 이 푸시를 거절했다:
git-push-pull = 풀…
# $branch, $remote, $remoteRef: 실제로 보낸 곳
git-push-done = { $branch } → { $remote } : { $remoteRef } 푸시함
# $refspec: 보낸 refspec
git-push-done-refspec = 보낸 것: { $refspec }

## 풀 다이얼로그(⌘T).

git-pull-title = 프로젝트 업데이트
git-pull-mode = 방법
git-pull-merge = Merge
git-pull-rebase = Rebase
git-pull-remote = 원격
git-pull-branch = 브랜치
# $name: upstream 브랜치
git-pull-upstream = upstream: { $name }
git-pull-no-upstream = 이 브랜치에 upstream 이 없다 — 원격 브랜치를 고른다
git-pull-send = 업데이트
git-pull-done = 업데이트했다
git-pull-stopped = 충돌로 멈췄다 — 풀고 나서 커밋 툴 윈도우에서 계속하거나 중단한다

## fetch.

git-fetch-done = 원격을 모두 가져왔다

## 설정 창 › Git(10-ui S7).

settings-git-title = Git
settings-git-path = 실행 파일
# $value: [git] path 설정
settings-git-configured = 설정에 적힌 값: { $value }
settings-git-not-configured = 로그인 셸의 PATH 에서 「git」 으로 찾았다
settings-git-version = 버전
# $minimum: 쓰기가 되는 가장 낮은 버전
settings-git-minimum = 쓰기에는 { $minimum } 이상이 필요하다
settings-git-writes = 쓰기
settings-git-writes-on = 켜짐
# $program: 찾은 이름
settings-git-not-found = 꺼짐 — { $program } 을(를) 찾지 못했다
# $version: 찾은 버전, $minimum: 쓰기가 되는 가장 낮은 버전
settings-git-too-old = 꺼짐 — { $version } 은(는) { $minimum } 보다 낮다
# $reason: git --version 을 읽지 못한 이유
settings-git-unreadable = 꺼짐 — 버전을 읽지 못했다: { $reason }
settings-git-shell-path = 로그인 셸의 PATH 에서 찾았다. 신뢰한 프로젝트의 런타임(mise)이 다른 git 을 앞에 둘 수 있다.
# $reason: 로그인 셸을 읽지 못한 이유
settings-git-process-path = 로그인 셸을 읽지 못해({ $reason }) 앱의 PATH 에서 찾았다.
settings-git-checking = 확인하는 중…
# $error: 오류
settings-git-failed = 확인하지 못했다: { $error }
settings-git-recheck = 다시 확인
