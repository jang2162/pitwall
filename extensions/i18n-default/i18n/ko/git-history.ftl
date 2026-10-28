### 파일 히스토리(⌃⌥H, apps/desktop/src/git/history): 이름 바뀜을 따라가는 그 파일의 커밋 목록 + 고른 커밋에서의 그 파일 diff.

git-history-action-show = 파일 히스토리 보기
# 프로젝트 트리 · 에디터 탭의 우클릭 메뉴 항목
git-history-menu = 히스토리 보기
# 탭 이름. $name: 파일 이름
git-history-title = { $name } 히스토리
git-history-list = 파일 히스토리
git-history-loading = 불러오는 중…
git-history-empty = 이 파일을 바꾼 커밋이 없다
# $message: Host 의 오류 원문
git-history-failed = 히스토리를 읽지 못했다: { $message }
# $count: 커밋 수
git-history-count = 커밋 { $count }개
git-history-pick = 커밋을 고르면 그 커밋에서 이 파일의 diff 를 보인다

## 이름 바뀐 지점. 그 커밋 행에 옛 이름 → 새 이름을 단다.

# $from: 옛 경로, $to: 새 경로
git-history-renamed = 이름 바뀜 { $from } → { $to }
# $from: 원본 경로, $to: 새 경로
git-history-copied = 복사 { $from } → { $to }
# $path: 그 커밋에서의 경로(저장소 기준)
git-history-path-at = 이 커밋에서의 경로: { $path }

## 바뀜(스크린 리더가 글자 대신 읽는다).

git-history-change-added = 추가됨
git-history-change-modified = 수정됨
git-history-change-deleted = 삭제됨
git-history-change-renamed = 이름 바뀜
git-history-change-copied = 복사됨
git-history-change-type-changed = 종류 바뀜

## 커밋 행의 우클릭 메뉴.

git-history-menu-reveal = Git 로그에서 선택
git-history-menu-show-commit = 커밋 상세 열기
git-history-menu-open-diff = 이 diff 를 탭으로 열기
git-history-menu-open-revision = 이 리비전의 파일 열기
git-history-menu-copy-hash = 해시 복사
git-history-copied-hash = 해시를 복사했다

## 파일 히스토리를 열지 못할 때(알림).

git-history-no-file = 히스토리를 볼 파일이 없다
git-history-no-file-body = 에디터에서 파일을 열거나 프로젝트 트리에서 파일을 고른다
git-history-outside = 저장소 밖의 파일이다
git-history-folder = 폴더의 히스토리는 볼 수 없다 — 파일을 고른다
# $submodule: 서브모듈 자리(저장소 기준 경로)
git-history-submodule = { $submodule } 서브모듈 안의 파일이다 — 히스토리는 그 서브모듈 저장소에서 본다
# $repo: 그 저장소 폴더(트리 루트 기준)
git-history-nested = { $repo } 는 따로 된 저장소(서브모듈 · 중첩 저장소)다 — 그 안 파일의 히스토리는 그 저장소에서 본다
# $message: 오류 원문
git-history-open-failed = 파일 히스토리를 열지 못했다: { $message }
