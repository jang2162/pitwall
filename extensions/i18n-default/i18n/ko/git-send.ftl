### 커밋 보내기 · 가져오기(apps/desktop/src/git/send): 다른 머신 체크아웃으로 커밋을 보내고 가져오는 팝업 · 확인 · 알림(18-m7-plan M7-17).

action-git-send = 다른 체크아웃으로 보내기…
action-git-receive = 다른 체크아웃에서 가져오기…

## 대상을 고르는 팝업.

git-send-pick-send = 보낼 체크아웃
git-send-pick-receive = 가져올 체크아웃
git-send-pick-placeholder = 체크아웃 찾기
git-send-pick-empty = 이 프로젝트에 다른 머신의 체크아웃이 없다
git-send-main = 메인 체크아웃
git-send-last = 마지막 대상

## 확인.

# $branch: 브랜치, $from: 원본 머신, $to: 대상 머신
git-send-confirm-title = { $branch } 을(를) { $from } 에서 { $to } (으)로
git-send-from = 원본
git-send-to = 대상
git-send-loading = 원본 브랜치를 읽는 중…
git-send-not-branch = 원본 체크아웃이 브랜치에 있지 않다 — 보낼 브랜치가 없다
git-send-not-connected = 그 머신에 연결되지 않았다
git-send-uncommitted = 원본에 커밋하지 않은 변경이 있다 — 그 변경은 가지 않는다
# $runs: 실행 수, $terminals: 터미널 수
git-send-target-busy = 대상 체크아웃에서 실행 { $runs }개 · 터미널 { $terminals }개가 돌고 있다 — 막지 않는다
git-send-no-force = 대상 브랜치가 갈라졌으면 덮지 않고 멈춘다
git-send-go = 보내기
git-send-go-receive = 가져오기

## 진행 · 끝.

# $to: 대상 머신
git-send-progress = { $to } (으)로 보내는 중
git-send-cancel = 보내기 취소
# $branch: 브랜치, $to: 대상 머신
git-send-created = { $branch } 을(를) { $to } 에 만들고 체크아웃했다
# $branch: 브랜치, $to: 대상 머신, $range: 「이전 해시 → 지금 해시」
git-send-moved = { $to } 의 { $branch } 을(를) { $range } (으)로 올리고 체크아웃했다
# $branch: 브랜치, $to: 대상 머신
git-send-same = { $to } 의 { $branch } 은(는) 이미 그 커밋이다 — 체크아웃했다
git-send-uncommitted-left = 원본의 커밋하지 않은 변경은 가지 않았다
# $count: LFS 포인터로 남은 파일 수
git-send-lfs = 파일 { $count }개가 LFS 포인터로 남았다 — 그 머신에서 git lfs pull 이 필요하다
git-send-run-there = 거기서 실행
git-send-terminal-there = 거기서 터미널
# $branch: 브랜치, $to: 대상 머신
git-send-stopped = { $to } 의 { $branch } 을(를) 맞추지 못해 멈췄다 — 대상은 바뀌지 않았다
# $ref: 받은 커밋을 둔 전용 ref
git-send-incoming = 받은 커밋은 { $ref } 에 있다
git-send-reason = { $reason ->
    [diverged] 대상 브랜치에 원본에 없는 커밋이 있다(갈라짐)
    [branchMoved] 고른 뒤 대상 브랜치가 움직였다
    [incomingMoved] 받은 커밋을 둔 전용 ref 가 그사이 바뀌었다
    [inOtherWorktree] 그 브랜치를 대상 머신의 다른 워크트리가 쓰고 있다
   *[blocked] git 이 거절했다
}
git-send-failed = 커밋 보내기 실패
