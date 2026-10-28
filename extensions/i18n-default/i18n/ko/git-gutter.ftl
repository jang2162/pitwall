### 에디터 거터의 git 변경 표시 · 인라인 diff 팝업 · blame(apps/desktop/src/git/gutter).

## 액션(액션 찾기에 보인다).

git-gutter-action-next-change = 다음 변경 위치
git-gutter-action-previous-change = 이전 변경 위치
git-gutter-action-toggle-blame = Git blame 주석 토글
git-gutter-action-blame-revision = 리비전의 파일을 blame 주석과 함께 열기

## 변경 표시(거터 막대 · 삼각형의 툴팁).

git-gutter-marker-added = HEAD 이후 더한 줄 — 눌러 변경 보기
git-gutter-marker-modified = HEAD 이후 바뀐 줄 — 눌러 변경 보기
git-gutter-marker-deleted = HEAD 이후 지운 줄 — 눌러 변경 보기

## 인라인 diff 팝업.

git-gutter-popup-label = 변경 미리보기
git-gutter-popup-revert = 되돌리기
git-gutter-popup-stage = 스테이징
git-gutter-popup-previous = 이전 변경
git-gutter-popup-next = 다음 변경
git-gutter-popup-close = 닫기
# 몇 번째 변경인지. $index: 1부터, $count: 전체
git-gutter-popup-position = { $index } / { $count }
git-gutter-stage-dirty = 저장하지 않은 편집이 있습니다 — 저장한 뒤 스테이징할 수 있습니다
git-gutter-stage-read-only = 이 문서는 고칠 수 없어 되돌리지 않습니다
git-gutter-staged-already = 이 변경은 이미 스테이징돼 있습니다
git-gutter-stage-disk-changed = 디스크의 파일이 이 문서와 달라 스테이징하지 않았습니다
git-gutter-stage-failed = 스테이징하지 못했습니다: { $message }

## 거터를 끈 까닭(에디터 위 막대).

# $driver: .gitattributes 의 filter 속성이 고른 드라이버 이름(lfs 등)
git-gutter-off-filter = Git 변경 표시를 껐습니다: 이 파일은 필터 드라이버 `{ $driver }` 를 거칩니다(드라이버는 자동으로 돌리지 않습니다)
git-gutter-off-dismiss = 닫기

## blame.

git-blame-uncommitted = 아직 커밋 안 됨
# 현재 줄 끝의 흐린 글씨. $author: 작성자, $when: 「3일 전」 같은 상대 시각, $subject: 커밋 제목
git-blame-inline = { $author }, { $when } · { $subject }
# 주석 칸 툴팁의 작성자 줄(해시 다음 줄, 제목은 그 아래). $date: 작성 시각
git-blame-tooltip-author = { $author } <{ $email }> · { $date }
git-blame-boundary = 경계 커밋(첫 커밋이거나 얕은 클론의 끝) — 이 줄이 이 커밋에서 생겼는지는 모를 수 있습니다
git-blame-click = 눌러 Git 로그에서 이 커밋 선택
git-blame-no-moves = 줄 이동 추적 안 함 — 파일 안에서 옮긴 줄은 옮긴 커밋으로 보입니다
git-blame-loading = blame 을 읽는 중…
git-blame-failed = blame 을 읽지 못했습니다: { $message }
git-blame-menu-previous = 이 커밋 이전으로 blame
git-blame-menu-close = blame 주석 닫기
git-blame-menu-history = 파일 히스토리
