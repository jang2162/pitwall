// 에이전트용 표지 기억(16-m5-plan 결정 M5-D37 — 사용자 결정 2026-11-18, M5-17b): 연결 편집 다이얼로그의 「옵션」 탭에서 그 연결의
// 대상에 기억된 프로덕션 · 읽기 전용 표지를 보이고, 사람이 확인 다이얼로그를 지난 뒤에만 푼다(`db.connection.forgetMarks` — 그
// 프로젝트 창의 연결만 받는다, crates/pw-db/src/mark_memory.rs). 기억은 앱 실행을 넘어 남는다 — 에이전트가 설정 파일에서 표지를
// 지우고 앱이 다시 켜져도 그대로라, 푸는 길은 이것 하나다.
//
// - 기억이 없으면 아무것도 그리지 않는다(읽기 실패도 — 콘솔에만 남긴다).
// - 저장된 설정이 아직 표지를 달고 있으면(`configured`) 풀어도 곧바로 다시 기억된다는 것을 먼저 말한다.
// - 확인 다이얼로그의 처음 포커스는 [취소] 다(위험 작업 확인과 같은 규칙).

import * as Dialog from "@radix-ui/react-dialog";
import type { DbConnectionMarkMemory, DbMarks } from "@pitwall/rpc-client";
import { useEffect, useRef, useState } from "react";

import { BUTTON, type ConnectionDialogDeps, errorText, SHEET } from "./connection-dialog";
import { useDbConnectionStrings } from "./strings";

const HINT = "text-[11px] text-ui-muted-foreground";

/** 표지 문장의 갈래. 표지가 없으면 `undefined`. */
export function marksKind(marks: DbMarks): "production" | "readOnly" | "both" | undefined {
  if (marks.production && marks.readOnly) return "both";
  if (marks.production) return "production";
  if (marks.readOnly) return "readOnly";
  return undefined;
}

type Note = { kind: "forgotten" | "again" } | { kind: "failed"; message: string };

export function RememberedMarks({
  deps,
  connectionId,
  name,
}: {
  deps: ConnectionDialogDeps;
  connectionId: string;
  name: string;
}) {
  const S = useDbConnectionStrings();
  const T = S.marks;
  const [memory, setMemory] = useState<DbConnectionMarkMemory>();
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<Note>();
  const latest = useRef(deps);
  latest.current = deps;
  const params = () => ({
    projectId: latest.current.projectId,
    worktree: latest.current.worktree ?? null,
    connectionId,
  });

  useEffect(() => {
    let stale = false;
    latest.current.client.call("db.connection.marks", params()).then(
      (m) => {
        if (!stale) setMemory(m);
      },
      (e: unknown) => console.warn("[db] mark memory unavailable", connectionId, e),
    );
    return () => {
      stale = true;
    };
    // 처음 한 번 — 연결 id 는 다이얼로그 동안 바뀌지 않는다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const forget = async () => {
    setAsking(false);
    setBusy(true);
    try {
      const forgot = await latest.current.client.call("db.connection.forgetMarks", params());
      setNote({ kind: marksKind(forgot.configured) ? "again" : "forgotten" });
      setMemory(await latest.current.client.call("db.connection.marks", params()));
    } catch (e) {
      setNote({ kind: "failed", message: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  const remembered = memory ? marksKind(memory.remembered) : undefined;
  if (!remembered && !note) return null;
  return (
    <section
      data-testid="db-conn-marks"
      data-remembered={remembered ?? "none"}
      className="flex flex-col gap-1 border-t border-ui-border pt-2"
    >
      <span>{T.title}</span>
      {remembered && <p>{T.remembered(remembered)}</p>}
      {remembered && memory && marksKind(memory.configured) && (
        <p className={HINT} data-testid="db-conn-marks-configured">
          {T.configured}
        </p>
      )}
      {note && (
        <p
          role={note.kind === "failed" ? "alert" : "status"}
          data-testid="db-conn-marks-note"
          data-note={note.kind}
          className={note.kind === "failed" ? "break-all text-ui-accent" : HINT}
        >
          {note.kind === "failed"
            ? T.failed(note.message)
            : note.kind === "again"
              ? T.forgottenAgain
              : T.forgotten}
        </p>
      )}
      {remembered && (
        <div>
          <button
            type="button"
            className={BUTTON}
            disabled={busy}
            data-testid="db-conn-marks-forget"
            onClick={() => setAsking(true)}
          >
            {T.forget}
          </button>
        </div>
      )}
      {asking && (
        <Dialog.Root open onOpenChange={(open) => !open && setAsking(false)}>
          <Dialog.Portal>
            <Dialog.Overlay className="fixed inset-0 bg-black/20" />
            <Dialog.Content data-testid="db-conn-marks-confirm" className={SHEET}>
              <Dialog.Title className="text-sm font-semibold">{T.confirmTitle(name)}</Dialog.Title>
              <Dialog.Description className="text-ui-muted-foreground">
                {T.confirmBody}
              </Dialog.Description>
              <div className="flex justify-end gap-2">
                <button
                  type="button"
                  autoFocus
                  className={BUTTON}
                  data-testid="db-conn-marks-cancel"
                  onClick={() => setAsking(false)}
                >
                  {S.button.cancel}
                </button>
                <button
                  type="button"
                  className={`${BUTTON} border-ui-accent text-ui-accent`}
                  data-testid="db-conn-marks-confirm-forget"
                  onClick={() => void forget()}
                >
                  {T.confirm}
                </button>
              </div>
            </Dialog.Content>
          </Dialog.Portal>
        </Dialog.Root>
      )}
    </section>
  );
}
