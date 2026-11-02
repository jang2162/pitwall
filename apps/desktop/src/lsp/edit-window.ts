// 창 하나의 편집 문맥(15-m4-plan M4-12): 일괄 편집 적용기(workspace-edit.ts) · 이 창이 언어 서버에 동기화한 문서 표 · 서버 명령의
// 흐름(`lsp.command.execute` 와 그 처리 중에 서버가 거는 편집 — 결정 M4-D15) · 결과 알림. 창(edit-ui.tsx)이 하나 만들고, 에디터 쪽
// (LSP 기능 `edit` — edit-feature.ts, rename · 코드 액션)은 문서의 `services` 로 찾는다(`editWindowOf`) — 기능 모듈이 창의 것을
// import 하지 않는다.
//
// 서버가 거는 편집: Host 는 사람이 고른 명령이 그 서버에서 도는 동안에만 이 창에 이벤트 `lsp.applyEdit`(흐름 id 실림)를 보낸다
// (crates/pw-lsp/src/edit.rs). 창은 자기가 연 흐름의 것만 적용기에 넘기고(미리 확인 · 미리보기 · 되돌리기 그대로) 답한다
// (`lsp.applyEdit.answer`). 모르는 흐름의 이벤트는 적용하지도 답하지도 않는다 — Host 가 시한 뒤 `applied: false` 로 끝낸다.

import type { EventMessage, LspWorkspaceEdit } from "@pitwall/rpc-client";

import type { FsChangedEvent } from "../editor/documents";
import type { NotificationHandle, NotificationInput } from "../notifications/notifications";
import type { LspEditStrings } from "./edit-strings";
import type { LspDocument, LspServices } from "./lsp-sessions";
import { EditApplier, type EditApplierDeps, type EditOutcome } from "./workspace-edit";

/** 서버가 거는 편집의 토픽(crates/pw-lsp/src/edit.rs 의 `TOPIC_APPLY_EDIT`). */
export const LSP_APPLY_EDIT_TOPIC = "lsp.applyEdit";
/** 그 이벤트의 내용(crates/pw-lsp/src/edit.rs 의 `LspApplyEditEvent` — 이벤트 꼴은 RPC 생성물에 들지 않아 여기 같은 꼴을 둔다). */
export interface LspApplyEditEvent {
  flowId: string;
  requestId: number;
  server: string;
  label: string | null;
  edit: LspWorkspaceEdit;
}
/** `fs.changed` 토픽(crates/pw-fs/src/watch/mod.rs). */
const FS_CHANGED = "fs.changed";

export interface EditWindowDeps extends Omit<EditApplierDeps, "synced"> {
  services: LspServices;
  strings(): LspEditStrings;
  notify(input: NotificationInput): NotificationHandle;
  /** 설정 `[lsp] lightbulb`(기본 켬). */
  lightbulb(): boolean;
}

export interface EditWindow {
  readonly applier: EditApplier;
  strings(): LspEditStrings;
  notify(input: NotificationInput): NotificationHandle;
  lightbulb(): boolean;
  /** 이 창이 동기화한 문서(탭이 보이는 동안 — 기능 칸이 단다). 해제 함수를 돌려준다. */
  register(doc: LspDocument): () => void;
  /** 적용 결과를 알린다. */
  report(outcome: EditOutcome, failed?: (message: string) => string): void;
  /** 서버 명령을 돌린다 — 그동안 그 서버가 건 편집을 적용기에 넘긴다. */
  execute(doc: LspDocument, command: unknown): Promise<void>;
  /** 마지막 일괄 편집을 되돌리고 알린다. */
  undoLast(): Promise<void>;
  dispose(): void;
}

const windows = new WeakMap<LspServices, EditWindow>();

/** 문서의 창(`doc.services`)의 편집 문맥. 창이 붙지 않았으면(시험 · 파일 창) 없다. */
export function editWindowOf(services: LspServices): EditWindow | undefined {
  return windows.get(services);
}

const baseName = (path: string) => path.replace(/^.*[/\\]/, "");
const names = (paths: readonly string[]) => paths.map(baseName).join(", ");

export function createEditWindow(deps: EditWindowDeps): EditWindow {
  const docs = new Map<string, Set<LspDocument>>();
  const synced = () => {
    const out = new Map<string, number>();
    for (const [path, set] of docs) {
      const doc = set.values().next().value;
      if (doc) out.set(path, doc.version);
    }
    return out;
  };
  const applier = new EditApplier({ ...deps, synced });
  /** 이 창이 돌리는 명령의 흐름 id. */
  const flows = new Set<string>();
  let seq = 0;
  let lastNotice: NotificationHandle | undefined;
  applier.onRecord(() => {
    if (applier.last) return;
    lastNotice?.retire();
    lastNotice = undefined;
  });

  const report: EditWindow["report"] = (outcome, failed) => {
    const T = deps.strings();
    switch (outcome.kind) {
      case "nothing":
      case "cancelled":
        return;
      case "stale":
        deps.notify({ level: "warning", title: T.stale, body: outcome.message });
        return;
      case "refused":
        deps.notify({ level: "error", title: T.refused(outcome.message ?? "") });
        return;
      case "failed":
        if (outcome.written.length === 0 && outcome.editorOnly.length === 0) {
          deps.notify({ level: "error", title: (failed ?? T.failed)(outcome.message ?? "") });
          return;
        }
    }
    const body = [
      outcome.editorOnly.length > 0 && T.editorOnly(names(outcome.editorOnly)),
      outcome.notWritten.length > 0 && T.notWritten(names(outcome.notWritten.map((n) => n.path))),
      outcome.kind === "failed" && (failed ?? T.failed)(outcome.message ?? ""),
    ]
      .filter(Boolean)
      .join("\n");
    lastNotice?.retire();
    lastNotice = deps.notify({
      level: outcome.notWritten.length > 0 || outcome.kind === "failed" ? "warning" : "info",
      title: T.applied(outcome.written.length + outcome.editorOnly.length),
      ...(body !== "" && { body }),
      ...(applier.last && { action: { label: T.undo, run: () => void win.undoLast() } }),
    });
  };

  // 서버가 거는 편집 · 파일 바뀜(창이 사는 동안 하나씩).
  const unsubscribe = deps.services.subscribe(
    [LSP_APPLY_EDIT_TOPIC, FS_CHANGED],
    (event: EventMessage) => {
      if (event.topic === FS_CHANGED) {
        applier.changed(event.payload as unknown as FsChangedEvent);
        return;
      }
      const request = event.payload as unknown as LspApplyEditEvent;
      if (!flows.has(request.flowId)) return;
      void (async () => {
        let outcome: EditOutcome;
        try {
          outcome = await applier.begin().apply(request.edit, {
            ...(request.label != null && { label: request.label }),
          });
        } catch (e) {
          outcome = {
            kind: "failed",
            message: e instanceof Error ? e.message : String(e),
            written: [],
            editorOnly: [],
            notWritten: [],
          };
        }
        report(outcome);
        const applied = outcome.kind === "applied" || outcome.kind === "nothing";
        await deps.services
          .call("lsp.applyEdit.answer", {
            flowId: request.flowId,
            requestId: request.requestId,
            applied,
            failureReason: applied ? null : (outcome.message ?? outcome.kind),
          })
          .catch(() => {});
      })();
    },
  );

  const win: EditWindow = {
    applier,
    strings: deps.strings,
    notify: deps.notify,
    lightbulb: deps.lightbulb,
    register(doc) {
      let set = docs.get(doc.path);
      if (!set) docs.set(doc.path, (set = new Set()));
      set.add(doc);
      return () => {
        set.delete(doc);
        if (set.size === 0 && docs.get(doc.path) === set) docs.delete(doc.path);
      };
    },
    report,
    async execute(doc, command) {
      const flowId = `flow-${Date.now().toString(36)}-${++seq}`;
      flows.add(flowId);
      try {
        await doc.request(`command-${flowId}`, (at, signal) =>
          deps.services.call(
            "lsp.command.execute",
            { docId: at.docId, flowId, command: command as never },
            { signal },
          ),
        );
      } finally {
        flows.delete(flowId);
      }
    },
    async undoLast() {
      const T = deps.strings();
      if (!applier.last) {
        deps.notify({ level: "info", title: T.nothingToUndo });
        return;
      }
      const undone = await applier.undoLast();
      deps.notify({
        level: undone.skipped.length > 0 ? "warning" : "info",
        title: T.reverted(undone.reverted.length),
        ...(undone.skipped.length > 0 && {
          body: T.notReverted(names(undone.skipped.map((s) => s.path))),
        }),
      });
    },
    dispose() {
      unsubscribe();
      if (windows.get(deps.services) === win) windows.delete(deps.services);
    },
  };
  windows.set(deps.services, win);
  return win;
}
