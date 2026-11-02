// 언어 서버의 일괄 편집 적용기(15-m4-plan M4-12, 결정 M4-D14 — IntelliJ 식: 디스크에 바로 쓰고 「되돌리기」). rename · 코드 액션 ·
// 서버가 거는 편집(`workspace/applyEdit`)이 모두 이것을 지난다. 다른 행이 파일을 고치는 길을 따로 두지 않는다 — 포맷(M4-13)은
// 단일 문서 함수 `applyDocumentEdits` 를 쓴다(문서에만 넣고 디스크에 쓰지 않는다).
//
// 한 번의 적용은 네 단계다:
//   ① 미리 확인(쓰기 전, 전부) — 열린 문서 중 이 창이 언어 서버에 동기화한 것은 요청할 때의 버전 = 지금 버전 = 서버가 계산한 버전
//      (`TextDocumentEdit.version`)이어야 하고, 동기화하지 않은 열린 문서(뒤쪽 탭 — 서버는 디스크를 봤다)는 디스크와 같아야 하며
//      (미저장 편집이 있으면 그 편집과 겹치지 않을 때만 서버의 편집을 그 위로 옮긴다), 열지 않은 파일은 요청 뒤 `fs.changed` 가
//      오지 않았어야 한다(`begin` 부터 센다). 파일 연산은 `fs.stat` 으로 있음 · 없음과 루트(연결 범위)를 본다. 하나라도 어긋나면
//      **아무것도 쓰지 않는다**(`stale` · `refused`). 미리보기(바뀌는 파일이 둘 이상)를 거치면 확인 뒤 같은 확인을 한 번 더 한다.
//      **다른 창이 연 파일**(Host 가 `foreign` 표지를 단다 — 서버는 그 창의 미저장 텍스트를 봤다)이 끼면 처음부터 아무것도 하지
//      않는다(`stale` · `reason: "foreign"`) — 그 창의 버퍼 아래 디스크를 덮거나 어긋난 범위를 쓰지 않는다(결정 M4-D14 ①).
//   ② 쓰기 — 열린 문서부터: 마지막 확인 바로 뒤 `await` 없는 구간에서 한 문서씩 넣기 직전에 다시 보고(텍스트 · 버전) 문서에 한
//      트랜잭션(undo 한 단계)으로 넣는다 — 그사이 사람이 친 글이 끼어들 틈이 없다. 미저장 편집이 없던 문서는 문서 모델의 저장
//      (기대 `version` — `⌘S` 와 같은 규칙)으로 쓰고, **미저장 편집이 있는 열린 문서는 디스크를 덮지 않고 그 문서에만 넣는다**
//      (미저장으로 남는다). 그다음 파일 연산(`fs.create` · `fs.rename` · `fs.trash`, 덮어쓰기 옵션은 무시하고 충돌이면 멈춤), 마지막에
//      열지 않은 파일을 읽은 텍스트에 편집을 적용해 `fs.write`(기대 `version`)로. 확인과 쓰기 사이에 기대 값이 달라진 파일은 쓰지
//      않고 `notWritten` 에. 대상마다 따로 잡아 하나가 멈춰도 나머지는 가고, 파일 연산이 멈추면 열지 않은 파일은 쓰지 않는다.
//   ③ 되돌리기 기록 — 쓴 파일마다 원래 바이트(텍스트 + BOM — 줄바꿈은 텍스트에 그대로)와 쓴 뒤의 `version`, 열린 문서면 넣은 뒤의
//      텍스트, 파일 연산은 한 일과 지운 파일의 내용, 에디터에만 넣은 문서는 넣은 뒤의 텍스트. 창마다 마지막 하나. **어느 길로
//      멈춰도(`finally`) 그때까지 쓴 것을 기록한다** — 쓴 것은 늘 되돌릴 수 있고 결과에 실린다.
//   ④ 결과(`EditOutcome`) — 창이 알림 「N개 파일을 바꿨다 [되돌리기]」 로 보인다(edit-ui.tsx).
// 되돌리기(`undoLast`)는 기록 하나를 한 번에 거꾸로 돌린다. 그사이 사람이 고친 파일(쓴 뒤 `version` 이 달라짐 · 문서 텍스트가 넣은
// 뒤와 다름 · 미저장)은 건드리지 않고 `skipped` 에 둔다.
//
// 위치: 서버의 위치(줄 · UTF-16 글자)는 서버가 본 텍스트에서 오프셋으로 바꾼다. 줄바꿈은 LSP 규칙(`\r\n` · `\n` · `\r`)이다. 열린
// 문서는 문서 텍스트(`Text.toString()` — 웹뷰가 서버에 보낸 그 글이고, 그 글의 오프셋이 CodeMirror 위치다), 열지 않은 파일은
// 디스크 텍스트(BOM 을 뗀 것)다.
//
// 파일 범위: `fs.*` 의 접근 범위(워크트리)는 `begin` 때 잡는다(`EditFs.pin`) — 계획 · 미리보기 · 쓰기 사이에 다른 워크트리 탭으로
// 옮겨도 같은 범위다. `.git` 은 Host 가 URI 글자로 거절하고, 여기서 `fs.stat` 이 준 실제 경로로 다시 본다(저장소 안 심볼릭 링크로
// `.git` 에 닿는 경로).

import { diff } from "@codemirror/merge";
import { ChangeSet, EditorState, type Text } from "@codemirror/state";
import {
  ERROR_CODE,
  type FsContent,
  type FsRead,
  type FsStat,
  type FsTrashed,
  type FsWritten,
  type LspPosition,
  type LspTextEdit,
  type LspWorkspaceEdit,
} from "@pitwall/rpc-client";

import type { DocumentStore, FsChangedEvent, TextDocumentState } from "../editor/documents";
import type { LiveText } from "../editor/live-text";

// ---- 위치 ------------------------------------------------------------------------------------------------------------------

/** 줄마다 시작 오프셋(LSP 줄바꿈 규칙 — `\r\n` · `\n` · `\r`). */
export function lineStarts(text: string): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c === 10) starts.push(i + 1);
    else if (c === 13) {
      if (text.charCodeAt(i + 1) === 10) i++;
      starts.push(i + 1);
    }
  }
  return starts;
}

/** LSP 위치 → `text` 의 오프셋. 줄 밖의 글자는 줄 끝으로, 없는 줄은 텍스트 끝으로(LSP 규칙). */
export function lspOffset(text: string, starts: readonly number[], pos: LspPosition): number {
  if (pos.line >= starts.length) return text.length;
  const start = starts[pos.line]!;
  let end = text.length;
  if (pos.line + 1 < starts.length) {
    const next = starts[pos.line + 1]!;
    end = next - (text.slice(next - 2, next) === "\r\n" ? 2 : 1);
  }
  return start + Math.min(pos.character, end - start);
}

export interface TextChange {
  readonly from: number;
  readonly to: number;
  readonly insert: string;
}

/** 서버의 편집이 겹치거나 거꾸로다(LSP 규칙 위반). */
export class EditShapeError extends Error {}

/** 한 파일의 편집들(모두 같은 텍스트 기준) → 앞에서부터의 바꾸기. 같은 자리의 넣기는 서버가 준 차례다. */
export function textChanges(text: string, edits: readonly LspTextEdit[]): TextChange[] {
  const starts = lineStarts(text);
  const changes = edits.map((e, i) => ({
    from: lspOffset(text, starts, e.range.start),
    to: lspOffset(text, starts, e.range.end),
    insert: e.newText,
    i,
  }));
  changes.sort((a, b) => a.from - b.from || a.i - b.i);
  for (let k = 0; k < changes.length; k++) {
    const c = changes[k]!;
    if (c.from > c.to) throw new EditShapeError("a text edit ends before it starts");
    if (k > 0 && c.from < changes[k - 1]!.to)
      throw new EditShapeError("the language server sent overlapping text edits");
  }
  return changes.map(({ from, to, insert }) => ({ from, to, insert }));
}

/** 바꾸기들을 적용한 글. */
export function applyTextChanges(text: string, changes: readonly TextChange[]): string {
  let out = "";
  let at = 0;
  for (const c of changes) {
    out += text.slice(at, c.from) + c.insert;
    at = c.to;
  }
  return out + text.slice(at);
}

/** 열린 문서에 넣을 `ChangeSet`(문서 텍스트 기준). */
export function documentChanges(live: LiveText, edits: readonly LspTextEdit[]): ChangeSet {
  const text = live.text.toString();
  return ChangeSet.of(textChanges(text, edits), text.length, live.separator);
}

/**
 * 단일 문서 함수(포맷 M4-13 이 쓴다): 서버의 편집을 문서에 한 트랜잭션 — undo 한 단계 — 으로 넣는다. 붙은 뷰는 커서 · 선택을
 * 매핑한다. 디스크에 쓰지 않는다. 바뀐 뒤 텍스트를 돌려준다. 편집이 겹치면 `EditShapeError`.
 */
export function applyDocumentEdits(
  live: LiveText,
  edits: readonly LspTextEdit[],
  userEvent = "lsp.edit",
): Text {
  return live.applyChanges(documentChanges(live, edits), { userEvent, isolate: true });
}

// ---- 적용기 ----------------------------------------------------------------------------------------------------------------

/** 적용기가 쓰는 파일 RPC(창은 이 창의 Host 연결 · 그 워크트리로 묶어 넘긴다). */
export interface EditFs {
  stat(path: string): Promise<FsStat>;
  read(path: string): Promise<FsRead>;
  write(
    path: string,
    content: FsContent,
    bom: boolean,
    expectedVersion: string | null,
  ): Promise<FsWritten>;
  create(path: string): Promise<unknown>;
  rename(from: string, to: string): Promise<unknown>;
  trash(path: string): Promise<FsTrashed>;
  /** 지금의 접근 범위에 묶은 것(`begin` 이 부른다 — 그 뒤 활성 탭이 바뀌어도 범위가 그대로다). 없으면 이것 그대로. */
  pin?(): EditFs;
}

/** 경로의 어느 마디가 `.git` 이다(대소문자 무시, Windows 의 짧은 이름 `GIT~1` 도). */
export function insideGit(path: string): boolean {
  return path.split(/[/\\]/).some((part) => /^\.git$/i.test(part) || /^git~\d+$/i.test(part));
}

/** 적용기가 쓰는 창의 에디터(문서 모델 · 탭이 파일 조작을 따라가게). */
export interface EditWorkspace {
  readonly documents: Pick<DocumentStore, "get" | "save" | "changed">;
  moved(from: string, to: string): void;
  deleted(path: string): void;
}

/** 미리보기의 파일 하나. */
export interface PreviewFile {
  readonly path: string;
  /** 파일 연산(만들기 · 이름 바꾸기 · 지우기). 텍스트만 바뀌면 없다. */
  readonly op?: "create" | "rename" | "delete";
  /** 이름 바꾸기의 옛 경로. */
  readonly from?: string;
  readonly before: string;
  readonly after: string;
  /** 바뀌는 줄 수(더한 줄 + 뺀 줄). */
  readonly lines: number;
  /** 미저장 편집이 있는 열린 문서라 디스크에 쓰지 않고 에디터에만 넣는다. */
  readonly editorOnly: boolean;
}

export interface EditApplierDeps {
  fs: EditFs;
  workspace: EditWorkspace;
  /** 이 창이 언어 서버에 동기화한 열린 문서: 경로 → 지금 버전(`LspDocument.version`). */
  synced(): ReadonlyMap<string, number>;
  /** 미리보기를 보이고 사람의 답을 기다린다(적용이면 `true`). */
  preview(files: readonly PreviewFile[], label: string | undefined): Promise<boolean>;
}

/** 적용 결과. */
export interface EditOutcome {
  readonly kind: "applied" | "nothing" | "cancelled" | "stale" | "refused" | "failed";
  /** `stale` · `refused` · `failed` 의 까닭(사람이 읽는 글 — 영어, 창이 문자열로 감싼다). */
  readonly message?: string;
  /**
   * 창이 따로 알리는 까닭: `foreign` — 다른 창이 연 파일이 끼었다(`stale`, `files` 가 그 파일), `ended` — 미리보기를 보는 사이 그
   * 편집을 건 명령이 끝났다(`cancelled` — 서버에는 이미 `applied: false` 가 갔다).
   */
  readonly reason?: "foreign" | "ended";
  /** `stale` · `refused` 가 걸린 파일. */
  readonly files?: readonly string[];
  /** 디스크에 쓴 파일(파일 연산 포함). */
  readonly written: readonly string[];
  /** 미저장 편집이 있어 에디터에만 넣은 문서. */
  readonly editorOnly: readonly string[];
  /** 확인과 쓰기 사이에 바뀌어 쓰지 못한 파일. */
  readonly notWritten: readonly { path: string; reason: string }[];
}

export interface UndoOutcome {
  readonly reverted: readonly string[];
  /** 그사이 사람이 고쳐 건드리지 않은 파일(「되돌리지 않은 파일」). */
  readonly skipped: readonly { path: string; reason: string }[];
}

/** 확인이 어긋났다 — 아무것도 쓰지 않는다. */
class Stop extends Error {
  constructor(
    readonly kind: "stale" | "refused",
    message: string,
    readonly files: readonly string[] = [],
    readonly reason?: "foreign",
  ) {
    super(message);
  }
}

const stopped = (e: Stop): EditOutcome => ({
  ...nothing(e.kind, e.message),
  ...(e.files.length > 0 && { files: e.files }),
  ...(e.reason && { reason: e.reason }),
});

/** 쓰는 사이 바뀐 열린 문서의 까닭. */
const CHANGED_WHILE_WRITING = "changed while the edit was being written";

const errorCode = (e: unknown) => {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "number" ? code : undefined;
};
const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** 편집할 수 있는 텍스트로 읽은 것(UTF-8 · 깨진 바이트 없음 — 원래 바이트로 되돌릴 수 있어야 한다). */
function editableText(read: FsRead): string | undefined {
  const { judgment, content } = read;
  if (judgment.binary || content.kind !== "text") return undefined;
  if (judgment.encoding !== "UTF-8" || judgment.lossy || !judgment.editable) return undefined;
  return content.text;
}

/** 열린 문서(텍스트)의 상태. */
function textDoc(workspace: EditWorkspace, path: string): TextDocumentState | undefined {
  const doc = workspace.documents.get(path);
  return doc?.status === "text" ? doc : undefined;
}

/** 바뀐 줄 수(미리보기). */
function changedLines(before: string, after: string): number {
  let lines = 0;
  for (const c of diff(before, after, { timeout: 200 })) {
    lines += before.slice(c.fromA, c.toA).split("\n").length - (c.fromA === c.toA ? 1 : 0);
    lines += after.slice(c.fromB, c.toB).split("\n").length - (c.fromB === c.toB ? 1 : 0);
  }
  return lines;
}

/** 편집 대상 파일 하나(계획). */
type Target =
  | {
      kind: "open";
      path: string;
      /** 계획할 때 문서가 있던 경로(이 편집이 이름을 바꾸면 `path` 와 다르다). */
      at: string;
      doc: TextDocumentState;
      /** 계획할 때의 문서 텍스트 — 쓰기 직전에 같아야 한다. */
      base: Text;
      /** 이 창이 언어 서버에 동기화한 문서면 그 버전. */
      synced: number | undefined;
      /** 넣을 바꾸기(문서 텍스트 기준) — 차례로 쌓인다. */
      changes: ChangeSet;
    }
  | {
      kind: "file";
      path: string;
      /** 처음 읽은 것(없으면 이 편집이 만든 파일). */
      read: FsRead | undefined;
      original: string;
      current: string;
      bom: boolean;
    };

type OpenTarget = Extract<Target, { kind: "open" }>;
type FileTarget = Extract<Target, { kind: "file" }>;

/** 파일 연산(계획). */
type Op =
  | { kind: "create"; path: string; withText: boolean }
  | { kind: "rename"; from: string; to: string }
  | { kind: "delete"; path: string; saved: { content: FsContent; bom: boolean } | undefined };

/** 계획: 대상(요청 경로 · 실제 경로가 같은 대상을 가리킬 수 있다)과 파일 연산. */
interface Plan {
  targets: Map<string, Target>;
  ops: Op[];
}

/** 되돌리기 기록의 항목. */
type UndoEntry =
  | {
      kind: "file";
      path: string;
      original: { content: FsContent; bom: boolean };
      writtenVersion: string;
      /** 열린 문서에 넣고 저장했으면 그 문서의 넣은 뒤 텍스트와 되돌릴 바꾸기. */
      doc?: { after: Text; inverse: ChangeSet };
    }
  | { kind: "editor"; path: string; after: Text; inverse: ChangeSet }
  | { kind: "created"; path: string; writtenVersion: string | null }
  | { kind: "renamed"; from: string; to: string }
  | { kind: "deleted"; path: string; saved: { content: FsContent; bom: boolean } | undefined };

export interface UndoRecord {
  readonly label: string | undefined;
  readonly entries: readonly UndoEntry[];
  /** 그 편집을 쓴 파일 범위(되돌리기도 같은 범위로). */
  readonly fs: EditFs;
}

export interface ApplyOptions {
  label?: string;
  /** 아직 이 편집을 원하나 — 미리보기 뒤 · 쓰기 직전에 본다. 아니면 아무것도 쓰지 않고 `cancelled`(`reason: "ended"`). */
  wanted?: () => boolean;
}

/** 편집 흐름 하나(요청을 보내기 전에 `begin` 으로 연다). */
export interface EditFlow {
  /** 편집을 적용한다(미리 확인 → 미리보기 → 쓰기 → 기록). 던지지 않는다 — 뜻밖의 오류도 `failed` 결과다. */
  apply(edit: LspWorkspaceEdit, options?: ApplyOptions): Promise<EditOutcome>;
}

const nothing = (kind: EditOutcome["kind"], message?: string): EditOutcome => ({
  kind,
  ...(message !== undefined && { message }),
  written: [],
  editorOnly: [],
  notWritten: [],
});

export class EditApplier {
  /** `fs.changed` 를 받을 때마다 오른다. */
  private seq = 0;
  private readonly changedAt = new Map<string, number>();
  private rescanAt = -1;
  private record: UndoRecord | undefined;
  private readonly listeners = new Set<() => void>();
  /** 적용 · 되돌리기는 한 번에 하나(창마다). */
  private queue: Promise<unknown> = Promise.resolve();

  constructor(private readonly deps: EditApplierDeps) {}

  /** 창이 받은 `fs.changed`. */
  changed(event: FsChangedEvent) {
    this.seq += 1;
    if (event.rescan) this.rescanAt = this.seq;
    for (const path of event.paths) this.changedAt.set(path, this.seq);
  }

  /** 마지막 일괄 편집의 기록(되돌릴 것이 없으면 `undefined`). */
  get last(): UndoRecord | undefined {
    return this.record;
  }

  /** 기록이 바뀌면 부른다(되돌리기 액션 켜고 끄기). */
  onRecord(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private setRecord(record: UndoRecord | undefined) {
    this.record = record;
    for (const l of this.listeners) l();
  }

  /** 요청을 보내기 직전에 연다 — 그 뒤의 `fs.changed` 와 동기화한 문서의 버전을 기준으로 미리 확인한다. */
  begin(): EditFlow {
    const since = this.seq;
    const versions = new Map(this.deps.synced());
    const fs = this.deps.fs.pin?.() ?? this.deps.fs;
    return {
      apply: (edit, options = {}) =>
        this.serial(() => this.applyNow(edit, options, since, versions, fs)).catch(
          (e: unknown): EditOutcome => nothing("failed", errorText(e)),
        ),
    };
  }

  /** 마지막 일괄 편집을 한 번에 되돌린다. 기록이 없으면 빈 결과. */
  undoLast(): Promise<UndoOutcome> {
    return this.serial(async () => {
      const record = this.record;
      if (!record) return { reverted: [], skipped: [] };
      this.setRecord(undefined);
      return this.revert(record);
    });
  }

  private serial<T>(run: () => Promise<T>): Promise<T> {
    const next = this.queue.then(run, run);
    this.queue = next.catch(() => {});
    return next;
  }

  /** 열지 않은 파일이 `since` 뒤에 바뀌었다(감시가 다시 훑으라고 했으면 모두 그렇게 본다). */
  private touchedSince(path: string, since: number): boolean {
    return this.rescanAt > since || (this.changedAt.get(path) ?? -1) > since;
  }

  private async applyNow(
    edit: LspWorkspaceEdit,
    { label, wanted }: ApplyOptions,
    since: number,
    versions: ReadonlyMap<string, number>,
    fs: EditFs,
  ): Promise<EditOutcome> {
    if (edit.changes.length === 0) return nothing("nothing");
    // 다른 창이 연 파일 — 서버의 범위는 그 창의 미저장 텍스트 기준이다. 읽지도 쓰지도 않는다.
    const foreign = edit.changes.flatMap((c) => (c.kind === "text" && c.foreign ? [c.path] : []));
    if (foreign.length > 0)
      return stopped(
        new Stop(
          "stale",
          `${foreign.join(", ")} is open in another window of this project — save it there and try again`,
          foreign,
          "foreign",
        ),
      );
    let plan: Plan;
    try {
      plan = await this.plan(edit, versions, fs);
      this.verify(plan.targets, since, versions);
    } catch (e) {
      if (e instanceof Stop) return stopped(e);
      if (e instanceof EditShapeError) return nothing("refused", e.message);
      return nothing("failed", errorText(e));
    }
    const files = this.previewFiles(plan);
    if (files.length === 0) return nothing("nothing");
    if (files.length > 1) {
      if (!(await this.deps.preview(files, label))) return nothing("cancelled");
      try {
        this.verify(plan.targets, since, versions);
      } catch (e) {
        if (e instanceof Stop) return stopped(e);
        throw e;
      }
    }
    if (wanted && !wanted())
      return {
        ...nothing("cancelled", "the command that asked for this edit has ended"),
        reason: "ended",
      };
    // 여기서 `write` 의 열린 문서 넣기까지 `await` 가 없다(마지막 확인과 넣기 사이에 사람이 친 글이 끼지 않는다).
    return this.write(plan, label, fs);
  }

  /** 서버가 본 텍스트를 읽어 편집을 적용해 둔다(아직 쓰지 않는다). 파일 연산의 있음 · 없음 · 루트를 본다. */
  private async plan(
    edit: LspWorkspaceEdit,
    versions: ReadonlyMap<string, number>,
    fs: EditFs,
  ): Promise<Plan> {
    const { workspace } = this.deps;
    const targets = new Map<string, Target>();
    const ops: Op[] = [];
    /** 이 편집 안에서 지운 · 옮긴 경로(그 뒤에는 없다). */
    const gone = new Set<string>();

    const stat = async (path: string): Promise<FsStat | undefined> => {
      let s: FsStat;
      try {
        s = await fs.stat(path);
      } catch (e) {
        if (errorCode(e) === ERROR_CODE.NOT_FOUND) return undefined;
        throw new Stop("refused", `${path} cannot be changed here: ${errorText(e)}`, [path]);
      }
      // Host 는 URI 글자로 `.git` 을 막는다 — 저장소 안 심볼릭 링크로 닿는 실제 경로를 여기서 막는다.
      if (insideGit(s.path))
        throw new Stop("refused", `${path} leads inside .git (${s.path})`, [path]);
      return s;
    };
    const exists = async (path: string) =>
      targets.has(path) || (!gone.has(path) && (await stat(path)) !== undefined);
    const parentOf = (path: string) => path.replace(/[/\\][^/\\]*$/, "") || "/";
    const needParent = async (path: string) => {
      const parent = parentOf(path);
      if (targets.has(parent)) return;
      const s = await stat(parent);
      if (!s || (s.target ?? s.kind) !== "dir")
        throw new Stop("refused", `the folder of ${path} does not exist`);
    };

    const load = async (path: string): Promise<Target> => {
      const known = targets.get(path);
      if (known) return known;
      if (gone.has(path))
        throw new Stop("refused", `${path} is gone by an earlier part of the edit`);
      const s = await stat(path);
      if (!s) throw new Stop("stale", `${path} no longer exists`, [path]);
      const real = s.path;
      const doc = textDoc(workspace, real);
      let target: Target;
      if (doc) {
        if (doc.readOnly) throw new Stop("refused", `${real} is read-only`);
        if (doc.disk.kind !== "same") throw new Stop("stale", `${real} changed on disk`, [real]);
        target = {
          kind: "open",
          path: real,
          at: real,
          doc,
          base: doc.live.text,
          synced: versions.get(real),
          changes: ChangeSet.empty(doc.live.text.length),
        };
      } else {
        const read = await fs.read(real);
        const text = editableText(read);
        if (text === undefined)
          throw new Stop("refused", `${real} is not a UTF-8 text file the editor can change`);
        target = {
          kind: "file",
          path: real,
          read,
          original: text,
          current: text,
          bom: read.judgment.bom === "utf8",
        };
      }
      targets.set(real, target);
      if (real !== path) targets.set(path, target);
      return target;
    };

    for (const change of edit.changes) {
      switch (change.kind) {
        case "text": {
          const target = await load(change.path);
          if (target.kind === "file") {
            target.current = applyTextChanges(
              target.current,
              textChanges(target.current, change.edits),
            );
            break;
          }
          const { doc } = target;
          if (target.synced !== undefined) {
            // 서버가 이 창의 문서 텍스트를 봤다 — 그 버전이어야 한다.
            if (change.version !== null && change.version !== target.synced)
              throw new Stop("stale", `${target.path} changed after the language server read it`, [
                target.path,
              ]);
            const now = target.changes.apply(target.base).toString();
            const next = ChangeSet.of(
              textChanges(now, change.edits),
              now.length,
              doc.live.separator,
            );
            target.changes = target.changes.compose(next);
          } else if (!doc.live.dirty) {
            // 동기화하지 않은 깨끗한 문서 — 서버가 본 디스크와 문서 텍스트가 같다.
            const now = target.changes.apply(target.base).toString();
            const next = ChangeSet.of(
              textChanges(now, change.edits),
              now.length,
              doc.live.separator,
            );
            target.changes = target.changes.compose(next);
          } else {
            target.changes = target.changes.compose(await this.rebase(target, change.edits, fs));
          }
          break;
        }
        case "create": {
          if (await exists(change.path)) {
            if (change.ignoreIfExists) break;
            throw new Stop("refused", `${change.path} already exists`);
          }
          await needParent(change.path);
          targets.set(change.path, {
            kind: "file",
            path: change.path,
            read: undefined,
            original: "",
            current: "",
            bom: false,
          });
          gone.delete(change.path);
          ops.push({ kind: "create", path: change.path, withText: false });
          break;
        }
        case "rename": {
          if (!(await exists(change.from)))
            throw new Stop("stale", `${change.from} no longer exists`, [change.from]);
          // 파일이면 지금 내용을 잡아 둔다 — 같은 편집이 새 이름에 텍스트를 바꿀 수 있다.
          if (!targets.has(change.from)) {
            const s = await stat(change.from);
            if (s && (s.target ?? s.kind) === "file") await load(change.from);
          }
          if (await exists(change.to)) {
            if (change.ignoreIfExists) break;
            throw new Stop("refused", `${change.to} already exists`);
          }
          await needParent(change.to);
          const moved = targets.get(change.from);
          if (moved) {
            // 같은 대상을 가리키는 열쇠(요청 경로 · 실제 경로)를 모두 걷는다 — 하나라도 남으면 같은 문서에 두 번 넣는다.
            for (const [key, t] of targets) if (t === moved) targets.delete(key);
            targets.set(change.to, { ...moved, path: change.to });
          }
          gone.add(change.from);
          gone.delete(change.to);
          ops.push({ kind: "rename", from: change.from, to: change.to });
          break;
        }
        case "delete": {
          const s = targets.has(change.path) ? undefined : await stat(change.path);
          if (!s && !targets.has(change.path)) {
            if (change.ignoreIfNotExists) break;
            throw new Stop("stale", `${change.path} no longer exists`, [change.path]);
          }
          const dir = s && (s.target ?? s.kind) === "dir";
          if (dir && !change.recursive)
            throw new Stop("refused", `${change.path} is a folder and the edit is not recursive`);
          let saved: { content: FsContent; bom: boolean } | undefined;
          if (!dir && s) {
            const read = await fs.read(s.path);
            saved = { content: read.content, bom: read.judgment.bom === "utf8" };
          }
          targets.delete(change.path);
          gone.add(change.path);
          ops.push({ kind: "delete", path: change.path, saved });
          break;
        }
      }
    }
    // 만든 파일에 텍스트가 들면 만들기 대신 「없을 때만 쓰기」 한 번으로.
    for (const op of ops)
      if (op.kind === "create") {
        const t = targets.get(op.path);
        op.withText = t?.kind === "file" && t.current !== "";
      }
    return { targets, ops };
  }

  /** 서버가 디스크를 본 미저장 문서: 디스크 텍스트 기준의 편집을 미저장 편집 위로 옮긴다. 겹치면 멈춘다. */
  private async rebase(
    target: Extract<Target, { kind: "open" }>,
    edits: readonly LspTextEdit[],
    fs: EditFs,
  ): Promise<ChangeSet> {
    if (!target.changes.empty)
      throw new Stop("stale", `${target.path} is edited twice by the language server`, [
        target.path,
      ]);
    const read = await fs.read(target.path);
    const diskRaw = editableText(read);
    if (diskRaw === undefined || read.version !== target.doc.version)
      throw new Stop("stale", `${target.path} changed on disk`, [target.path]);
    const { live } = target.doc;
    // 문서와 같은 줄 구분으로 나눈 디스크 텍스트(줄 · 글자 위치는 디스크와 같다).
    const disk = EditorState.create({
      doc: diskRaw,
      extensions: live.separator === undefined ? [] : EditorState.lineSeparator.of(live.separator),
    }).doc.toString();
    const buffer = target.base.toString();
    const server = textChanges(disk, edits);
    const mine = diff(disk, buffer, { timeout: 200 });
    for (const s of server)
      for (const m of mine)
        if (s.from <= m.toA && m.fromA <= s.to)
          throw new Stop(
            "stale",
            `${target.path} has unsaved edits where the language server wants to change it`,
          );
    const serverSet = ChangeSet.of(server, disk.length, live.separator);
    const mineSet = ChangeSet.of(
      mine.map((c) => ({ from: c.fromA, to: c.toA, insert: buffer.slice(c.fromB, c.toB) })),
      disk.length,
      live.separator,
    );
    return serverSet.map(mineSet);
  }

  /** 미리 확인: 계획한 뒤(그리고 미리보기 뒤) 기준이 그대로인가. */
  private verify(
    targets: Map<string, Target>,
    since: number,
    versions: ReadonlyMap<string, number>,
  ) {
    const now = this.deps.synced();
    for (const target of new Set(targets.values())) {
      const moved =
        target.kind === "file"
          ? target.read !== undefined && this.touchedSince(target.path, since)
          : !this.openAsPlanned(target, now) || versions.get(target.at) !== target.synced;
      if (moved)
        throw new Stop("stale", `${target.path} changed after the language server read it`, [
          target.path,
        ]);
    }
  }

  /** 열린 문서가 계획한 때 그대로다: 같은 문서 · 같은 텍스트 · 동기화했으면 같은 버전, 아니면 여전히 동기화 안 함 · 디스크와 같음. */
  private openAsPlanned(target: OpenTarget, now: ReadonlyMap<string, number>): boolean {
    const doc = textDoc(this.deps.workspace, target.at);
    if (doc?.live !== target.doc.live || !doc.live.text.eq(target.base)) return false;
    if (target.synced !== undefined) return now.get(target.at) === target.synced;
    return !now.has(target.at) && doc.disk.kind === "same";
  }

  private previewFiles({ targets, ops }: { targets: Map<string, Target>; ops: Op[] }) {
    const files: PreviewFile[] = [];
    const seen = new Set<Target>();
    for (const [path, target] of targets) {
      if (seen.has(target) || path !== target.path) continue;
      seen.add(target);
      const before = target.kind === "file" ? target.original : target.base.toString();
      const after =
        target.kind === "file" ? target.current : target.changes.apply(target.base).toString();
      const op = ops.find((o) => (o.kind === "rename" ? o.to : o.path) === path);
      if (before === after && !op) continue;
      files.push({
        path,
        ...(op && { op: op.kind }),
        ...(op?.kind === "rename" && { from: op.from }),
        before,
        after,
        lines: changedLines(before, after),
        editorOnly: target.kind === "open" && target.doc.live.dirty,
      });
    }
    for (const op of ops)
      if (op.kind === "rename" && !files.some((f) => f.path === op.to))
        files.push({
          path: op.to,
          op: "rename",
          from: op.from,
          before: "",
          after: "",
          lines: 0,
          editorOnly: false,
        });
      else if (op.kind === "create" && !files.some((f) => f.path === op.path))
        files.push({
          path: op.path,
          op: "create",
          before: "",
          after: "",
          lines: 0,
          editorOnly: false,
        });
      else if (op.kind === "delete")
        files.push({
          path: op.path,
          op: "delete",
          before: op.saved?.content.kind === "text" ? op.saved.content.text : "",
          after: "",
          lines: 0,
          editorOnly: false,
        });
    return files;
  }

  private async write(plan: Plan, label: string | undefined, fs: EditFs): Promise<EditOutcome> {
    const { workspace } = this.deps;
    const written: string[] = [];
    const editorOnly: string[] = [];
    const notWritten: { path: string; reason: string }[] = [];
    const entries: UndoEntry[] = [];
    /** 멈춘 까닭(파일 연산 · 뜻밖의 오류). 있으면 그 뒤의 열지 않은 파일은 쓰지 않는다. */
    let failure: string | undefined;
    const opens: OpenTarget[] = [];
    const files: FileTarget[] = [];
    const seen = new Set<Target>();
    for (const [path, target] of plan.targets) {
      if (seen.has(target) || path !== target.path) continue;
      seen.add(target);
      if (target.kind === "open") opens.push(target);
      else files.push(target);
    }
    // 만든 파일에 텍스트가 들면 만들기 대신 「없을 때만 쓰기」 한 번으로(아래 열지 않은 파일 차례).
    const created = new Set(
      plan.ops.flatMap((op) => (op.kind === "create" && op.withText ? [op.path] : [])),
    );
    try {
      // ① 열린 문서 — 마지막 확인 바로 뒤의 동기 구간(`await` 없음). 넣기 직전에 한 문서씩 다시 본다.
      const now = this.deps.synced();
      const saving: Promise<void>[] = [];
      for (const target of opens) {
        if (target.changes.empty) continue;
        const { at } = target;
        try {
          if (!this.openAsPlanned(target, now)) {
            notWritten.push({ path: at, reason: CHANGED_WHILE_WRITING });
            continue;
          }
          const { live } = target.doc;
          const dirty = live.dirty;
          const before = live.text;
          const inverse = target.changes.invert(before);
          const after = live.applyChanges(target.changes, { userEvent: "lsp.edit", isolate: true });
          const entry: UndoEntry = { kind: "editor", path: at, after, inverse };
          entries.push(entry);
          if (dirty) {
            editorOnly.push(at);
            continue;
          }
          const original = {
            content: { kind: "text" as const, text: live.serialize(before) },
            bom: live.bom,
          };
          saving.push(
            workspace.documents.save(at).then(
              (saved) => {
                const doc = textDoc(workspace, at);
                if (saved.kind !== "saved" || !doc?.version) {
                  // 디스크가 그사이 바뀌었다 — 편집은 문서에 미저장으로 남고(배너), 되돌리기는 에디터 쪽만.
                  notWritten.push({
                    path: at,
                    reason:
                      saved.kind === "failed"
                        ? saved.message
                        : "changed on disk before it was written",
                  });
                  return;
                }
                written.push(at);
                // 저장이 넣은 텍스트 그대로를 썼을 때만 바이트로 되돌린다. 그사이 사람이 쳤으면 에디터 쪽 기록이 남는다 — 되돌리기는
                // 그 문서를 「그사이 고친 파일」 로 건너뛴다.
                if (doc.live === live && !live.dirty && live.text.eq(after))
                  entries[entries.indexOf(entry)] = {
                    kind: "file",
                    path: at,
                    original,
                    writtenVersion: doc.version,
                    doc: { after, inverse },
                  };
              },
              (e: unknown) => void notWritten.push({ path: at, reason: errorText(e) }),
            ),
          );
        } catch (e) {
          notWritten.push({ path: at, reason: errorText(e) });
        }
      }
      // 저장이 끝난 뒤에 파일 연산 — 이름 바꾸기가 저장 중인 경로를 옮기지 않게.
      await Promise.all(saving);

      // ② 파일 연산(차례대로). 멈추면 그 뒤 연산과 열지 않은 파일은 하지 않는다.
      for (const op of plan.ops) {
        if (op.kind === "create" && op.withText) continue;
        try {
          if (op.kind === "create") {
            await fs.create(op.path);
            const after = await fs.read(op.path).catch(() => undefined);
            entries.push({
              kind: "created",
              path: op.path,
              writtenVersion: after?.version ?? null,
            });
            written.push(op.path);
          } else if (op.kind === "rename") {
            await fs.rename(op.from, op.to);
            workspace.moved(op.from, op.to);
            entries.push({ kind: "renamed", from: op.from, to: op.to });
            written.push(op.to);
          } else {
            const trashed = await fs.trash(op.path);
            if (trashed.failed.length > 0) throw new Error(trashed.failed[0]!.message);
            workspace.deleted(op.path);
            entries.push({ kind: "deleted", path: op.path, saved: op.saved });
            written.push(op.path);
          }
        } catch (e) {
          failure = errorText(e);
          break;
        }
      }

      // ③ 열지 않은 파일.
      if (failure === undefined)
        for (const target of files) {
          const { path } = target;
          const isNew = created.has(path);
          if (target.current === target.original && !isNew) continue;
          try {
            // 이름을 바꾼 파일은 새 자리의 지금 `version` 으로 쓴다(내용은 읽은 것과 같아야 한다).
            let expected = isNew ? null : (target.read?.version ?? null);
            if (entries.some((e) => e.kind === "renamed" && e.to === path)) {
              const again = await fs.read(path);
              if (editableText(again) !== target.original) throw conflict();
              expected = again.version;
            }
            const result = await fs.write(
              path,
              { kind: "text", text: target.current },
              target.bom,
              expected,
            );
            entries.push(
              isNew
                ? { kind: "created", path, writtenVersion: result.version }
                : {
                    kind: "file",
                    path,
                    original: { content: { kind: "text", text: target.original }, bom: target.bom },
                    writtenVersion: result.version,
                  },
            );
            written.push(path);
          } catch (e) {
            notWritten.push({
              path,
              reason:
                errorCode(e) === ERROR_CODE.CONFLICT
                  ? "changed on disk before it was written"
                  : errorText(e),
            });
          }
        }
    } catch (e) {
      failure ??= errorText(e);
    } finally {
      // 어느 길로 멈춰도 쓴 것은 되돌릴 수 있게.
      if (entries.length > 0) this.setRecord({ label, entries, fs });
    }
    const changed = written.length + editorOnly.length;
    return {
      kind: failure === undefined && changed > 0 ? "applied" : "failed",
      ...(failure !== undefined
        ? { message: failure }
        : changed === 0 && { message: "no file could be written" }),
      written,
      editorOnly,
      notWritten,
    };
  }

  private async revert(record: UndoRecord): Promise<UndoOutcome> {
    const { workspace } = this.deps;
    const { fs } = record;
    const reverted: string[] = [];
    const skipped: { path: string; reason: string }[] = [];
    const touched = "changed after the edit";
    const undo = { userEvent: "lsp.edit.undo", isolate: true };
    const reload = (path: string) =>
      workspace.documents.changed({
        root: "",
        worktree: null,
        paths: [path],
        kinds: ["modified"],
        rescan: false,
      });
    for (const entry of [...record.entries].reverse()) {
      try {
        switch (entry.kind) {
          case "editor": {
            const doc = textDoc(workspace, entry.path);
            if (!doc || !doc.live.text.eq(entry.after)) {
              skipped.push({ path: entry.path, reason: touched });
              break;
            }
            doc.live.applyChanges(entry.inverse, undo);
            reverted.push(entry.path);
            break;
          }
          case "file": {
            const doc = textDoc(workspace, entry.path);
            if (doc?.live.dirty) {
              skipped.push({ path: entry.path, reason: touched });
              break;
            }
            if (doc && entry.doc && doc.live.text.eq(entry.doc.after)) {
              // 쓴 그때의 디스크 그대로일 때만(디스크가 바뀌어 배너가 뜬 문서는 건드리지 않는다).
              if (doc.version !== entry.writtenVersion || doc.disk.kind !== "same") {
                skipped.push({ path: entry.path, reason: touched });
                break;
              }
              const before = doc.live.applyChanges(entry.doc.inverse, undo);
              const saved = await workspace.documents.save(entry.path);
              if (saved.kind === "saved") {
                reverted.push(entry.path);
                break;
              }
              // 저장하지 못했다(그사이 디스크가 바뀜) — 문서도 넣은 뒤 텍스트로 돌려 「건드리지 않음」 이 참이게.
              if (doc.live.text.eq(before))
                doc.live.applyChanges(entry.doc.inverse.invert(entry.doc.after), undo);
              skipped.push({ path: entry.path, reason: touched });
              break;
            }
            await fs.write(
              entry.path,
              entry.original.content,
              entry.original.bom,
              entry.writtenVersion,
            );
            if (doc) await reload(entry.path);
            reverted.push(entry.path);
            break;
          }
          case "created": {
            const now = await fs.read(entry.path);
            if (entry.writtenVersion === null || now.version !== entry.writtenVersion) {
              skipped.push({ path: entry.path, reason: touched });
              break;
            }
            const trashed = await fs.trash(entry.path);
            if (trashed.failed.length > 0) {
              skipped.push({ path: entry.path, reason: trashed.failed[0]!.message });
              break;
            }
            workspace.deleted(entry.path);
            reverted.push(entry.path);
            break;
          }
          case "renamed": {
            const taken = await fs.stat(entry.from).then(
              () => true,
              (e: unknown) => errorCode(e) !== ERROR_CODE.NOT_FOUND,
            );
            if (taken) {
              skipped.push({ path: entry.to, reason: `${entry.from} is taken` });
              break;
            }
            await fs.rename(entry.to, entry.from);
            workspace.moved(entry.to, entry.from);
            reverted.push(entry.from);
            break;
          }
          case "deleted": {
            if (!entry.saved) {
              skipped.push({ path: entry.path, reason: "a folder — restore it from the Trash" });
              break;
            }
            await fs.write(entry.path, entry.saved.content, entry.saved.bom, null);
            reverted.push(entry.path);
            break;
          }
        }
      } catch (e) {
        const path = entry.kind === "renamed" ? entry.to : entry.path;
        skipped.push({
          path,
          reason: errorCode(e) === ERROR_CODE.CONFLICT ? touched : errorText(e),
        });
      }
    }
    return { reverted, skipped };
  }
}

function conflict() {
  return Object.assign(new Error("changed on disk"), { code: ERROR_CODE.CONFLICT });
}
