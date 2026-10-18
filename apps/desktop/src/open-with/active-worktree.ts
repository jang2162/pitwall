// 프로젝트 창의 활성 워크트리(04 F2-5). 창이 받은 열기 요청(project-open.ts)의 `worktree`(`ActiveWorktree.top`)를 기억해
// 외부 도구가 `{project_root}` 를 그 워크트리 기준으로 열게 한다(openWith.run 의 `worktree`). 창마다 웹뷰가 따로라 모듈 상태로 둔다.
//
// 워크트리 없이 파일을 연 요청은 메인 체크아웃으로 돌아온 것으로 본다. 폴더만 다시 연 요청은 창을 앞으로 가져오기만 해서(빈 요청)
// 여기에 오지 않는다 — 그 경우 워크트리는 바뀌지 않는다.

import type { ProjectOpen } from "../project-open";

let active: string | undefined;

export function rememberActiveWorktree(open: ProjectOpen): void {
  if (open.worktree) active = open.worktree;
  else if (open.file) active = undefined;
}

export function activeWorktree(): string | undefined {
  return active;
}
