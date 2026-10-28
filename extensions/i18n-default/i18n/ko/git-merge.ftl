### 충돌 해결(apps/desktop/src/git/merge): 진행 중 작업 배너 · 충돌 목록 다이얼로그 · 3-way 머지 문서.

## 액션(액션 찾기에 보인다).

git-merge-action-resolve-conflicts = 충돌 해결…
git-merge-action-open = 머지 도구로 열기
git-merge-action-continue = 진행 중인 git 작업 계속
git-merge-action-skip = 진행 중인 git 작업의 이 단계 건너뛰기
git-merge-action-abort = 진행 중인 git 작업 중단

# 진행 중 작업의 이름. $kind: merge · rebase · cherryPick · revert · bisect · am
git-merge-kind = { $kind ->
        [merge] 머지
        [rebase] 리베이스
        [cherryPick] 체리픽
        [revert] 되돌림 커밋(revert)
        [bisect] bisect
       *[other] 패치 적용(am)
    }

## 진행 중 작업 배너(에디터 영역 위 · 커밋 툴 윈도우).

git-merge-banner-label = 진행 중인 git 작업
# $kind: git-merge-kind 의 글
git-merge-banner-title = { $kind } 진행 중
# $branch: 리베이스하는 브랜치, $onto: 대상 커밋(짧은 해시)
git-merge-banner-rebase = { $branch } → { $onto }
# $current · $total: 단계
git-merge-banner-step = { $current }/{ $total } 단계
# $count: 지금 단계 뒤에 남은 단계
git-merge-banner-remaining = 남은 단계 { $count }
# $count: 충돌 중인 파일 수
git-merge-banner-conflicts = { $count ->
        [0] 충돌 없음
       *[other] 충돌 파일 { $count }개
    }
git-merge-banner-resolve = 충돌 해결…
git-merge-banner-continue = 계속
git-merge-banner-skip = 건너뛰기
git-merge-banner-abort = 중단

## 중단 확인(결정 M2-D6 ① — 진행 중 작업 중단).

# $kind: git-merge-kind 의 글
git-merge-abort-title = { $kind }을(를) 중단할까?
git-merge-abort-body = 작업을 시작하기 전의 HEAD 와 작업 트리로 돌린다. 이 작업에서 풀어 둔 충돌도 사라진다.
git-merge-abort-confirm = 중단

## 결과 알림.

# $kind: git-merge-kind 의 글
git-merge-result-done = { $kind }을(를) 끝냈다
git-merge-result-stopped = 다음 단계에서 다시 충돌했다 — 충돌을 풀고 계속한다
git-merge-result-skipped = 이 단계를 건너뛰었다
git-merge-result-aborted = { $kind }을(를) 중단했다
# $action: 하려던 일(액션 이름)
git-merge-result-failed = { $action } 실패

## 충돌 목록 다이얼로그.

git-merge-conflicts-title = 충돌 파일
git-merge-conflicts-loading = 충돌을 읽는 중…
git-merge-conflicts-empty = 충돌이 없다
# $message: 오류 원문
git-merge-conflicts-failed = 충돌을 읽지 못했다: { $message }
git-merge-conflicts-ours = 내 것
git-merge-conflicts-theirs = 그쪽 것
git-merge-conflicts-merge = 병합…
git-merge-conflicts-close = 닫기
git-merge-conflicts-changed = 파일이 그사이 바뀌어 고르지 않았다 — 목록을 다시 읽었다
# 충돌의 갈래(index 단계). $kind: bothModified · bothAdded · addedByUs · addedByThem · deletedByUs · deletedByThem · bothDeleted
git-merge-conflict-kind = { $kind ->
        [bothModified] 양쪽에서 고침
        [bothAdded] 양쪽에서 추가
        [addedByUs] 우리 쪽에서만 추가
        [addedByThem] 그쪽에서만 추가
        [deletedByUs] 우리 쪽에서 지움
        [deletedByThem] 그쪽에서 지움
       *[other] 양쪽에서 지움
    }
# [병합…] 을 끈 까닭. $reason: binary · large · deleted · submodule · symlink · other
git-merge-pick-only = { $reason ->
        [binary] 바이너리 파일이라 한쪽만 고를 수 있다
        [large] 5 MiB 를 넘는 파일이라 한쪽만 고를 수 있다
        [deleted] 한쪽에서 지운 파일이라 한쪽만 고를 수 있다(지운 쪽을 고르면 지운다)
        [submodule] 서브모듈이라 한쪽만 고를 수 있다
        [symlink] 심볼릭 링크라 한쪽만 고를 수 있다
       *[other] 자동 병합 결과가 없어 한쪽만 고를 수 있다
    }

## 3-way 머지 문서(에디터 탭).

# $name: 파일 이름
git-merge-title = { $name } (머지)
git-merge-loading = 충돌을 읽는 중…
# $message: 오류 원문
git-merge-failed = 충돌을 읽지 못했다: { $message }
git-merge-gone = 이 파일은 이미 충돌이 아니다
git-merge-pane-ours = 내 것(ours)
git-merge-pane-result = 결과
git-merge-pane-theirs = 그쪽 것(theirs)
# $count: 아직 고르지 않은 충돌 수
git-merge-unresolved = { $count ->
        [0] 모든 충돌을 골랐다
       *[other] 남은 충돌 { $count }개
    }
git-merge-clean = 고를 충돌 구간이 없다 — 세 쪽을 보고 저장하면 해결된다
git-merge-apply-non-conflicting = 충돌 없는 변경 모두 적용
git-merge-compare-base = base 와 비교
git-merge-previous = 이전 충돌
git-merge-next = 다음 충돌
git-merge-save = 저장하고 해결
git-merge-apply-ours = 결과에 넣기(≫)
git-merge-apply-theirs = 결과에 넣기(≪)
git-merge-ignore = 무시(✕)
git-merge-pick-ours = 내 것으로 해결
git-merge-pick-theirs = 그쪽 것으로 해결
git-merge-side-missing = 이 쪽에는 파일이 없다(고르면 지운다)
# $count: 아직 고르지 않은 충돌 수
git-merge-save-unresolved-title = 충돌 { $count }개를 아직 고르지 않았다
git-merge-save-unresolved-body = 그 자리는 지금 결과 칸 그대로(비어 있으면 빈 채로) 저장되고 해결로 표시된다.
git-merge-save-unresolved-confirm = 그래도 저장
# $path: 저장소 기준 경로
git-merge-saved = { $path } 의 충돌을 풀었다
git-merge-save-conflict = 파일이 그사이 바뀌어 저장하지 않았다 — 다시 읽었다. 결과 칸은 그대로이니 다시 저장하면 덮는다
git-merge-all-resolved = 충돌을 모두 풀었다 — 진행 중 작업을 계속할 수 있다
