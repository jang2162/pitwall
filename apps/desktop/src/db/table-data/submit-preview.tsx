// 변경 제출 미리보기(14-m3-plan.md M3-17 · M3-12, 10-ui.md 「다이얼로그 · 팝업 목록」 의 DB 행): Host 가 만든 문장 글(`db.edit.preview`
// 의 리터럴 판 — 서버에는 같은 문장이 바인드 값으로 간다) · 문장마다 등급 · 적용 방식 · 연결 표지를 보이고, [적용] 이 같은
// `previewHash` 로 `db.edit.apply` 를 부른다. [적용] 이 결정 M3-D20 의 확인이다(프로덕션 연결의 「쓰기도 확인」 을 이 다이얼로그가
// 채운다 — M3-12 ⓐ).
//
// 틀은 M1 의 확인 시트와 같다(위쪽 시트 · 바깥 · Esc 는 [취소] · 떠 있는 동안 창의 키맵은 쉰다). 처음 포커스는 [취소] — 적용은
// 되돌릴 수 없어 Enter 한 번에 되지 않게(M2-20 위험 작업 확인과 같은 규칙). 적용하는 동안 두 단추를 끈다. 적용이 실패하면 다이얼로그를
// 닫지 않고 까닭을 보인다(변경은 그대로다).

import * as Dialog from "@radix-ui/react-dialog";
import type { DbEditPreview } from "@pitwall/rpc-client";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import { ConnectionMark, type DbConnectionMarkInput } from "../safety/connection-mark";
import { useDbSafetyStrings } from "../safety/strings";
import { useDbTableDataStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover disabled:opacity-50 disabled:hover:bg-transparent";

export function SubmitPreviewDialog({
  preview,
  connection,
  applying,
  failure,
  onApply,
  onCancel,
}: {
  preview: DbEditPreview;
  connection: DbConnectionMarkInput & { name: string };
  applying: boolean;
  /** 적용 실패의 문장(있으면). */
  failure: string | undefined;
  onApply(): void;
  onCancel(): void;
}) {
  const T = useDbTableDataStrings().preview;
  const S = useDbSafetyStrings();
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && !applying && onCancel()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="db-table-data-preview"
          className="sheet-top fixed left-1/2 flex max-h-[80vh] w-[720px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title}</Dialog.Title>
          <div className="flex items-center gap-2">
            <ConnectionMark {...connection} />
            <span className="text-ui-muted-foreground">
              {T.statements(preview.statements.length)}
            </span>
          </div>
          <Dialog.Description className="text-ui-muted-foreground">
            {preview.applies === "ownTransaction" ? T.own : T.open}
          </Dialog.Description>
          <ol
            data-testid="db-table-data-preview-statements"
            className="min-h-0 flex-1 overflow-auto rounded-sm border border-ui-border"
          >
            {preview.statements.map((s) => (
              <li
                key={s.index}
                data-statement={s.index}
                data-grade={s.grade}
                className="flex gap-2 border-b border-ui-border px-2 py-1 last:border-b-0"
              >
                <span className="shrink-0 text-[11px] text-ui-muted-foreground">
                  {S.grade[s.grade]}
                </span>
                <pre className="min-w-0 flex-1 font-mono text-[12px] whitespace-pre-wrap break-all">
                  {s.sql}
                </pre>
              </li>
            ))}
          </ol>
          {failure && (
            <p role="alert" data-testid="db-table-data-preview-failure" className="text-ui-accent">
              {failure}
            </p>
          )}
          <div className="flex items-center justify-end gap-2">
            {applying && <span className="text-ui-muted-foreground">{T.applying}</span>}
            <button
              type="button"
              autoFocus
              data-answer="cancel"
              className={BUTTON}
              disabled={applying}
              onClick={onCancel}
            >
              {T.cancel}
            </button>
            <button
              type="button"
              data-answer="apply"
              className={`${BUTTON} border-ui-accent`}
              disabled={applying}
              onClick={onApply}
            >
              {T.apply}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
