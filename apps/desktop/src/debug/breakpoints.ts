// 창 하나의 브레이크포인트(17-m6-plan M6-18 ①). 저장의 주인은 Host 다(`$DATA/projects/<id>/breakpoints.json`, M6-17 — `debug.breakpoints.*`).
// 이 객체는 그 목록을 받아 두고(`saved`), 열린 문서마다 브레이크포인트를 **문서 위치**로 쥔다.
//
// - 웹뷰 문서가 정본이다(결정 M4-D10). 편집으로 줄이 밀리면 문서의 바뀜(`ChangeSet`)으로 위치를 옮긴다 — CodeMirror 의 위치 매핑
//   (`mapPos`)이다. 줄의 맨 앞 위치를 쥐고, 그 앞에 넣은 글은 브레이크포인트를 뒤로 민다(줄 앞에 Enter → 아래로 따라간다). 지워진
//   줄의 것은 지운 자리의 줄로 모이고 같은 줄에 둘이면 먼저 있던 것이 남는다.
// - 미저장 편집의 줄은 그 문서에만 있다. 문서가 디스크와 같을 때(미저장 없음 — 저장한 직후 · 되돌려 같아짐 · 처음 열림)에만 그 파일의
//   목록을 Host 에 보낸다(`debug.breakpoints.set` — 파일 하나 통째로). 그래서 「저장 때 Host 에」 가 된다.
// - Host 이벤트 `debug.breakpoints`(저장이 바뀌었거나 어댑터가 확인 · 옮김을 알렸다)는 `saved` 를 바꾸고, 그 문서에 미저장이 없을
//   때만 문서의 위치를 다시 만든다 — 미저장이 있으면 문서의 것이 다음 저장에서 이긴다.
// - 경로: Host 는 프로젝트 루트 기준 상대 경로(`/`)다. 문서는 실제 절대 경로다 — 그 문서를 연 워크트리의 루트를 빼서 잇는다
//   (`relative`, 루트 밖이면 브레이크포인트를 두지 않는다).

import type { ChangeSet, Text } from "@codemirror/state";
import type {
  Breakpoint,
  BreakpointState,
  DebugBreakpointsEvent,
  EventMessage,
  RpcClient,
} from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand/vanilla";

export const TOPIC_BREAKPOINTS = "debug.breakpoints";

export type BreakpointsClient = Pick<RpcClient, "call" | "subscribe">;

/** 문서 하나의 바뀜(documents.ts `DocumentChange` 와 같은 꼴). */
export type DocChange =
  | { kind: "edit"; changes: ChangeSet; before: Text; after: Text }
  | { kind: "reset"; text: Text };

/** 브레이크포인트가 보는 문서 하나. */
export interface BreakpointDoc {
  /** 바뀜을 듣는다(지금 텍스트면 곧바로 `reset`). 해제 함수. */
  onChange(listener: (change: DocChange) => void): () => void;
  /** 미저장 편집이 있다. */
  dirty(): boolean;
  /** 미저장 여부가 바뀌면 부른다. 해제 함수. */
  onDirty(listener: () => void): () => void;
}

/** 문서 안의 브레이크포인트 하나: 줄 맨 앞의 위치와 그 설정(`line` 은 보낼 때 채운다). */
export interface Placed {
  pos: number;
  bp: Breakpoint;
}

interface OpenDoc {
  rel: string;
  text: Text;
  items: Placed[];
  source: BreakpointDoc;
  holds: number;
  off: () => void;
}

export interface BreakpointsState {
  /** Host 에 저장된 목록(상대 경로 → 줄 차례). */
  saved: Readonly<Record<string, readonly Breakpoint[]>>;
  /** 열린 문서의 지금 목록(절대 경로 → 위치 차례). 문서가 바뀔 때마다 새 배열이다. */
  docs: Readonly<Record<string, { rel: string; items: readonly Placed[] }>>;
  /** 세션이 확인하지 못한 줄(`상대 경로:줄` → 까닭). 세션이 끝나면 비운다. */
  unverified: Readonly<Record<string, string>>;
  /** 저장 파일을 읽지 못했다(Host 의 까닭). */
  problem: string | null;
}

/** 줄 맨 앞 위치들로 바꾼다(줄 차례, 같은 줄은 처음 것). */
function place(text: Text, bps: readonly Breakpoint[]): Placed[] {
  const seen = new Set<number>();
  const out: Placed[] = [];
  for (const bp of [...bps].sort((a, b) => a.line - b.line)) {
    if (bp.line < 1 || bp.line > text.lines || seen.has(bp.line)) continue;
    seen.add(bp.line);
    out.push({ pos: text.line(bp.line).from, bp });
  }
  return out;
}

/** 편집 뒤 위치(CodeMirror 의 위치 매핑 — 머리 주석). */
export function mapPlaced(items: readonly Placed[], changes: ChangeSet, after: Text): Placed[] {
  const seen = new Set<number>();
  const out: Placed[] = [];
  for (const item of items) {
    const line = after.lineAt(changes.mapPos(item.pos, 1));
    if (seen.has(line.number)) continue;
    seen.add(line.number);
    out.push({ pos: line.from, bp: item.bp });
  }
  return out.sort((a, b) => a.pos - b.pos);
}

/** 문서 위치의 목록을 줄이 든 목록으로. */
export function linesOf(text: Text, items: readonly Placed[]): Breakpoint[] {
  return items.map(({ pos, bp }) => ({ ...bp, line: text.lineAt(pos).number }));
}

function sameList(a: readonly Breakpoint[], b: readonly Breakpoint[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((x, i) => {
    const y = b[i]!;
    return (
      x.line === y.line &&
      (x.enabled ?? true) === (y.enabled ?? true) &&
      (x.condition ?? null) === (y.condition ?? null) &&
      (x.logMessage ?? null) === (y.logMessage ?? null) &&
      (x.hitCondition ?? null) === (y.hitCondition ?? null)
    );
  });
}

/** 바꿀 수 있는 칸. */
export type BreakpointPatch = Partial<
  Pick<Breakpoint, "enabled" | "condition" | "logMessage" | "hitCondition">
>;

export class Breakpoints {
  readonly store: StoreApi<BreakpointsState> = createStore<BreakpointsState>(() => ({
    saved: {},
    docs: {},
    unverified: {},
    problem: null,
  }));
  private readonly open = new Map<string, OpenDoc>();
  private client: BreakpointsClient | undefined;
  private projectId: string | undefined;
  /** 문서의 위치가 편집 밖의 까닭(누름 · Host)으로 바뀌었다 — 뷰가 다시 읽는다. */
  private readonly moved = new Set<(path: string) => void>();
  private sends = new Map<string, Promise<void>>();

  /** Host 에 붙는다: 저장된 목록을 읽고 `debug.breakpoints` 를 듣는다. 해제 함수. */
  connect(client: BreakpointsClient, projectId: string): () => void {
    this.client = client;
    this.projectId = projectId;
    let disposed = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    void client
      .subscribe({ projectId, topics: [TOPIC_BREAKPOINTS] }, (e: EventMessage) => {
        if (!disposed && e.topic === TOPIC_BREAKPOINTS)
          this.received(e.payload as unknown as DebugBreakpointsEvent);
      })
      .then((s) => (disposed ? void s.unsubscribe() : (unsubscribe = s.unsubscribe)))
      .catch((e: unknown) => console.warn("[debug] could not listen for breakpoints", e));
    void client
      .call("debug.breakpoints.list", { projectId })
      .then((list) => {
        if (disposed) return;
        const saved: Record<string, Breakpoint[]> = {};
        for (const f of list.files) saved[f.path] = f.breakpoints;
        this.store.setState({ saved, problem: list.problem ?? null });
        for (const [path, doc] of this.open) if (!doc.source.dirty()) this.rebuild(path, doc);
      })
      .catch((e: unknown) => console.warn("[debug] could not read the breakpoints", e));
    return () => {
      disposed = true;
      void unsubscribe?.();
      if (this.client === client) this.client = undefined;
    };
  }

  /** 문서가 편집 밖의 까닭으로 바뀌면 부른다(뷰의 연결). 해제 함수. */
  onMoved(listener: (path: string) => void): () => void {
    this.moved.add(listener);
    return () => this.moved.delete(listener);
  }

  /**
   * 열린 문서를 붙인다(뷰마다 — 같은 문서는 하나를 나눈다). `rel` 은 프로젝트 루트 기준 상대 경로. 돌려준 함수로 놓는다 — 마지막이
   * 놓으면 문서의 위치를 버린다(미저장 편집의 줄도 문서와 함께 사라진다).
   */
  attach(path: string, rel: string, source: BreakpointDoc): () => void {
    let doc = this.open.get(path);
    if (doc) {
      doc.holds += 1;
    } else {
      const made: OpenDoc = {
        rel,
        text: undefined as unknown as Text,
        items: [],
        source,
        holds: 1,
        off: () => {},
      };
      this.open.set(path, made);
      const offChange = source.onChange((change) => this.changed(path, made, change));
      const offDirty = source.onDirty(() => this.sync(path, made));
      made.off = () => {
        offChange();
        offDirty();
      };
      doc = made;
    }
    const held = doc;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      held.holds -= 1;
      if (held.holds > 0 || this.open.get(path) !== held) return;
      held.off();
      this.open.delete(path);
      const docs = { ...this.store.getState().docs };
      delete docs[path];
      this.store.setState({ docs });
    };
  }

  /** 문서의 지금 위치들(뷰가 거터를 그린다). 붙지 않은 문서면 빈 목록. */
  placed(path: string): readonly Placed[] {
    return this.open.get(path)?.items ?? [];
  }

  /** 문서의 지금 줄 목록. */
  lines(path: string): Breakpoint[] {
    const doc = this.open.get(path);
    return doc?.text ? linesOf(doc.text, doc.items) : [];
  }

  /** 그 문서의 상대 경로. */
  relative(path: string): string | undefined {
    return this.open.get(path)?.rel;
  }

  /** 줄 `line`(1부터)에 브레이크포인트가 있으면 지우고, 없으면 둔다. */
  toggle(path: string, line: number): void {
    const doc = this.open.get(path);
    if (!doc?.text || line < 1 || line > doc.text.lines) return;
    const pos = doc.text.line(line).from;
    const at = doc.items.findIndex((i) => i.pos === pos);
    const items =
      at >= 0
        ? doc.items.filter((_, i) => i !== at)
        : [...doc.items, { pos, bp: { line, enabled: true } }].sort((a, b) => a.pos - b.pos);
    this.replace(path, doc, items);
  }

  /** 줄 `line` 의 브레이크포인트를 고친다(없으면 만들어 고친다). */
  edit(path: string, line: number, patch: BreakpointPatch): void {
    const doc = this.open.get(path);
    if (!doc?.text || line < 1 || line > doc.text.lines) return;
    const pos = doc.text.line(line).from;
    const blank = (s: string | null | undefined) => (s && s.trim() !== "" ? s : null);
    const fix = (bp: Breakpoint): Breakpoint => {
      const next = { ...bp, ...patch };
      return {
        ...next,
        condition: blank(next.condition),
        logMessage: blank(next.logMessage),
        hitCondition: blank(next.hitCondition),
      };
    };
    const at = doc.items.findIndex((i) => i.pos === pos);
    const items =
      at >= 0
        ? doc.items.map((i, k) => (k === at ? { pos, bp: fix(i.bp) } : i))
        : [...doc.items, { pos, bp: fix({ line, enabled: true }) }].sort((a, b) => a.pos - b.pos);
    this.replace(path, doc, items);
  }

  /** 줄 `line` 의 것을 지운다. */
  remove(path: string, line: number): void {
    const doc = this.open.get(path);
    if (!doc?.text) return;
    const pos = line >= 1 && line <= doc.text.lines ? doc.text.line(line).from : -1;
    this.replace(
      path,
      doc,
      doc.items.filter((i) => i.pos !== pos),
    );
  }

  /** 열지 않은 파일의 것(목록 팝업)을 지운다 — Host 에 곧바로. */
  async removeSaved(rel: string, line: number): Promise<void> {
    const { client, projectId } = this;
    if (!client || projectId === undefined) return;
    for (const [path, doc] of this.open)
      if (doc.rel === rel) {
        this.remove(path, line);
        return;
      }
    await client.call("debug.breakpoints.remove", { projectId, path: rel, line });
  }

  /** 세션이 끝났다 — 확인 표시를 걷는다. */
  sessionEnded(): void {
    this.store.setState({ unverified: {} });
  }

  private replace(path: string, doc: OpenDoc, items: Placed[]) {
    doc.items = items;
    this.publish(path, doc);
    for (const listener of this.moved) listener(path);
    this.sync(path, doc);
  }

  private publish(path: string, doc: OpenDoc) {
    this.store.setState((s) => ({
      docs: { ...s.docs, [path]: { rel: doc.rel, items: doc.items } },
    }));
  }

  private changed(path: string, doc: OpenDoc, change: DocChange) {
    if (change.kind === "reset") {
      const keep = doc.text && doc.source.dirty() ? linesOf(doc.text, doc.items) : undefined;
      doc.text = change.text;
      doc.items = place(change.text, keep ?? this.store.getState().saved[doc.rel] ?? []);
      this.publish(path, doc);
      for (const listener of this.moved) listener(path);
      this.sync(path, doc);
      return;
    }
    // 뷰는 같은 바뀜을 자기 필드에서 같은 매핑으로 옮겼다 — 여기서 알리지 않는다(뷰가 갱신 중일 수 있다).
    doc.text = change.after;
    doc.items = mapPlaced(doc.items, change.changes, change.after);
    this.publish(path, doc);
    this.sync(path, doc);
  }

  private rebuild(path: string, doc: OpenDoc) {
    if (!doc.text) return;
    doc.items = place(doc.text, this.store.getState().saved[doc.rel] ?? []);
    this.publish(path, doc);
    for (const listener of this.moved) listener(path);
  }

  /** 문서가 디스크와 같으면 그 파일의 목록을 Host 에 보낸다(머리 주석). */
  private sync(_path: string, doc: OpenDoc) {
    if (!doc.text || doc.source.dirty()) return;
    const lines = linesOf(doc.text, doc.items);
    const saved = this.store.getState().saved[doc.rel] ?? [];
    if (sameList(lines, saved)) return;
    const { client, projectId } = this;
    if (!client || projectId === undefined) return;
    // 같은 파일의 보내기는 차례로 — 늦게 끝난 앞의 답이 뒤의 것을 덮지 않게 받은 목록은 이벤트로 맞춘다.
    this.store.setState((s) => ({ saved: { ...s.saved, [doc.rel]: lines } }));
    const before = this.sends.get(doc.rel) ?? Promise.resolve();
    const sent = before
      .then(() =>
        client.call("debug.breakpoints.set", { projectId, path: doc.rel, breakpoints: lines }),
      )
      .then(() => undefined)
      .catch((e: unknown) => console.warn("[debug] could not store the breakpoints", doc.rel, e));
    this.sends.set(doc.rel, sent);
  }

  private received(event: DebugBreakpointsEvent) {
    this.store.setState((s) => {
      const saved = { ...s.saved };
      if (event.breakpoints.length === 0) delete saved[event.path];
      else saved[event.path] = event.breakpoints;
      if (event.runId === null || event.runId === undefined) return { saved };
      const unverified = { ...s.unverified };
      for (const key of Object.keys(unverified))
        if (key.startsWith(`${event.path}:`)) delete unverified[key];
      for (const st of event.states as BreakpointState[])
        if (!st.verified) unverified[`${event.path}:${st.line}`] = st.message ?? "";
      return { saved, unverified };
    });
    for (const [path, doc] of this.open)
      if (doc.rel === event.path && !doc.source.dirty()) this.rebuild(path, doc);
  }
}

/** 절대 경로를 루트 기준 상대 경로(`/`)로. 루트 밖이면 `undefined`. */
export function relative(root: string, path: string): string | undefined {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const r = norm(root);
  const p = norm(path);
  if (!p.startsWith(`${r}/`)) return undefined;
  return p.slice(r.length + 1);
}
