// 언어 서버가 진행(`$/progress` — 색인 · 작업 공간 읽기 등)을 걸고 있는가 — rename(M4-12)이 까닭 없는 거절을 「아직 준비 중」 으로
// 알릴지 고른다. 상태는 M4-05 의 것(`lsp.status.get` — `lsp.status` 이벤트와 같은 꼴)을 그때 묻는다: 창의 `useLspStatus` 는 이벤트의
// 내용을 버리므로 진행을 읽으려면 그 공유 부품을 고쳐야 한다. 루트는 가리지 않는다(`LspInstanceStatus` 에 워크트리가 없고 `root` 는
// 실제 경로다 — 드물고 안전한 쪽).
//
// 포맷(M4-13)의 「준비 중」 은 이것을 쓰지 않는다 — 진행의 종류(제목)를 가려야 해서 Host 가 판정하고 답의 `notReady` 로 준다
// (crates/pw-lsp/src/format.rs 「진행 중의 답」). 알림 문구는 같다(`lsp-server-not-ready`).

import type { LspDocument } from "./lsp-sessions";

/** 서버 `server` 가 이 프로젝트에서 진행을 걸고 있는가. 묻지 못하면 아니다(알림은 지금 문구로). */
export async function indexing(doc: LspDocument, server: string): Promise<boolean> {
  try {
    const status = await doc.services.call("lsp.status.get", { projectId: doc.projectId });
    return status.instances.some((i) => i.server === server && i.progress !== null);
  } catch {
    return false;
  }
}
