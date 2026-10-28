### Git 로그 툴 윈도우(⌘9, apps/desktop/src/git/log): 브랜치 패널 · 필터 바 · 그래프 목록 · 커밋 상세 · 우클릭 메뉴 · 확인 대화상자.

git-log-list = 커밋 로그

## 브랜치 패널. 누르면 그 브랜치로 거른다(⌘ 를 누르면 여럿).

git-log-branches = 브랜치
git-log-branches-head = HEAD
git-log-branches-local = 로컬
git-log-branches-remote = 원격
git-log-branches-tags = 태그
git-log-branches-all = 모든 브랜치

## 필터 바. 텍스트 칸은 16진 7글자 이상이고 그 접두로 시작하는 커밋이 있으면 해시 접두로, 아니면 메시지로 찾는다.

git-log-filter-text = 텍스트/해시
git-log-filter-regex = 정규식
git-log-filter-match-case = 대소문자 구분
git-log-filter-branch = 브랜치
# $count: 고른 브랜치 수
git-log-filter-branch-count = 브랜치 { $count }개
git-log-filter-author = 작성자
git-log-filter-date = 날짜
git-log-filter-since = 부터
git-log-filter-until = 까지
git-log-filter-path = 경로
git-log-filter-path-hint = 저장소 기준 경로, 쉼표로 여럿(쉼표가 든 경로는 큰따옴표로)
git-log-filter-clear = 필터 지우기
git-log-filter-no-graph = 필터가 켜져 그래프 없이 맞는 커밋만 보인다

## 목록의 상태 줄.

git-log-loading = 불러오는 중…
# $count: 커밋 수
git-log-count = 커밋 { $count }개
# $count: 필터에 맞는 커밋 수
git-log-matched = 맞는 커밋 { $count }개
# $count: 해시 접두로 찾은 커밋 수
git-log-matched-hash = 해시 접두로 찾은 커밋 { $count }개
# $message: Host 의 오류 원문
git-log-failed = 로그를 읽지 못했다: { $message }
git-log-empty = 커밋이 없다
git-log-no-project = 프로젝트 창에서만 열 수 있다

## 오른쪽 커밋 상세.

git-log-detail-none = 커밋을 고르면 상세가 보인다
# $count: 바뀐 파일 수
git-log-detail-files = 변경 파일 ({ $count })
git-log-detail-files-loading = 변경 파일 읽는 중…
git-log-detail-parents = 부모
git-log-detail-root = 없음(뿌리 커밋)
git-log-detail-author = 작성자
git-log-detail-committer = 커미터
git-log-detail-contained-in = 포함한 브랜치 · 태그
git-log-detail-contained-loading = 찾는 중…
git-log-detail-contained-none = 없음
git-log-detail-generated = 생성
# $a, $b: 짧은 해시(옛 쪽, 새 쪽)
git-log-detail-compare = { $a } ↔ { $b } 비교
git-log-detail-open-compare = 비교 탭으로 열기
git-log-detail-open-commit = 커밋 상세 탭으로 열기
# $count: 고른 커밋 수
git-log-detail-many = 커밋 { $count }개를 골랐다
# $message: Host 의 오류 원문
git-log-detail-failed = 상세를 읽지 못했다: { $message }

## 우클릭 메뉴와 라벨 끌어 놓기 메뉴.

# $name: 브랜치 이름
git-log-menu-checkout-branch = { $name } 체크아웃
git-log-menu-checkout-revision = 이 커밋 체크아웃(분리된 HEAD)
git-log-menu-new-branch = 새 브랜치…
git-log-menu-new-tag = 새 태그…
# $count: 고른 커밋 수
git-log-menu-cherry-pick = { $count ->
        [1] Cherry-pick
       *[other] 커밋 { $count }개 Cherry-pick
    }
git-log-menu-revert = { $count ->
        [1] 커밋 되돌리기(Revert)
       *[other] 커밋 { $count }개 되돌리기(Revert)
    }
git-log-menu-reset = 이 커밋으로 Reset
# $mode: soft · mixed · hard · keep
git-log-menu-reset-mode = 이 커밋으로 Reset --{ $mode }
git-log-menu-copy-hash = 해시 복사
git-log-menu-show-commit = 커밋 상세 탭으로 열기
git-log-menu-compare = 두 커밋 비교 탭으로 열기
# $source: 끈 라벨, $target: 놓은 자리
git-log-menu-merge = { $source } 를 { $target } 에 merge
# $target: 지금 브랜치가 아닌 놓은 자리
git-log-menu-merge-needs-checkout = merge 하려면 먼저 { $target } 를 체크아웃한다
git-log-menu-rebase = { $source } 를 { $target } 위로 rebase
# $a, $b: 비교할 두 쪽
git-log-menu-compare-refs = { $a } 와 { $b } 비교

## 확인 대화상자.

git-log-dialog-new-tag = 새 태그
# $rev: 짧은 해시
git-log-dialog-from = { $rev } 에서
git-log-dialog-name-empty = 이름을 입력한다
git-log-dialog-name-invalid = 쓸 수 없는 이름이다

## 쓰기의 결과.

# $ref: 움직인 ref, $from · $to: 짧은 해시
git-log-result-moved = { $ref }: { $from } → { $to }
git-log-result-undo = 되돌리기
git-log-result-dismiss = 닫기
git-log-result-stopped = 충돌로 멈췄다
git-log-result-stopped-body = 충돌을 해결한 뒤 계속하거나 중단한다(커밋 툴 윈도우)
git-log-result-up-to-date = 바뀐 것이 없다
# $action: 메뉴 항목 이름
git-log-result-failed = { $action } 실패
git-log-result-copied = 해시를 복사했다

## 다른 화면이 로그에서 커밋을 고르게 할 때(액션 git.log.reveal — blame 주석 · 파일 히스토리).

git-log-reveal-action = Git 로그에서 커밋 선택
git-log-reveal-cleared = 지금 필터에 없는 커밋이라 필터를 풀었다
git-log-reveal-cleared-body = 브랜치 · 텍스트 · 작성자 · 날짜 · 경로 필터를 모두 지우고 그 커밋을 골랐다
git-log-reveal-missing = Git 로그에 없는 커밋이다
# $rev: 짧은 해시
git-log-reveal-missing-body = { $rev } 는 어느 브랜치 · 태그 · HEAD 에서도 닿지 않는다
# $message: Host 의 오류 원문
git-log-reveal-failed = 커밋을 찾지 못했다: { $message }
