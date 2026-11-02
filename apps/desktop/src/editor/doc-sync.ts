// 문서 동기화 공용 부품(15-m4-plan M4-07 ①): 열린 문서 하나를 Host 의 서비스(구문 분석 `syntax.*` · 언어 서버 `lsp.document.*`)에
// 문서 하나로 열고, 문서 모델의 바뀜(`DocumentStore.onChange`, 문서마다 한 번)을 버전을 붙여 차례로 보낸다. 서비스마다 다른 것은
// RPC 의 꼴뿐이라 `DocSyncProtocol` 로 받는다 — 구문 세션(syntax-sessions.ts)과 LSP 세션(src/lsp/lsp-sessions.ts)이 이 위에 선다.
//
// 버전: 문서를 (다시) 열 때 0 이고 바뀜마다 1씩 오른다. 웹뷰의 RPC 프레임은 순서가 보장되지 않으므로(crates/pw-rpc methods.rs
// 머리 주석) 열기의 응답을 받기 전에 생긴 편집은 모아 두었다가 응답 뒤에 차례로 보낸다.
//
// 어긋나면 다시 연다: 편집이 conflict · not found(서비스가 문서를 잃었다)로 끝나면 지금 텍스트로 새 문서를 연다(쓰는 쪽이 요청의
// 실패도 `failed` 로 넘긴다). 다시 열 때마다 세대(`generation`)가 바뀌고 앞 문서 id 로 늦게 도착한 답은 버린다.
//
// 그 밖의 실패(연결이 끊김 등)는 물러섰다가 지금 텍스트로 다시 연다 — 물러서는 시간은 실패가 이어질 때마다 두 배로 늘고
// `RETRY_MAX_MS` 에서 멈춘다. 쓰는 쪽이 답을 받으면(`answered`) 처음으로 돌아간다. `RETRY_NOTICE_AFTER` 번 이어 실패하면 로그에 한 번
// 남긴다. 물러서 있는 동안 기다리는 쪽(`settled`)은 다시 열 때까지 기다린다.
//
// 보내지 않는 문서(`off`): 기준(`protocol.skip` — 대용량 기준 limits.ts, 결정 M1-D9) 밖이거나 서비스가 끈 문서. 문서를 다시
// 읽으면(`reset`) 다시 본다.
//
// 닿음(`synced`): Host 가 어느 버전까지 받았는지 센다 — 편집의 답이 1 부터 빠짐없이 온 가장 큰 버전. Host 는 앞당겨 온 편집을 답한
// 뒤에 쌓아 두므로(차례가 오면 적용) 답 하나만으로는 그 버전이 적용됐다고 볼 수 없지만, 1 ~ N 이 모두 답했으면 N 까지 적용됐다. 요청은
// 그 버전이 닿은 뒤에 보낸다(LSP 요청 도우미) — 편집과 요청이 같은 틱에 나가면(`.` 을 친 직후의 완성) 요청이 편집보다 먼저 도착해
// 지난 버전으로 답하므로 버려진다(리뷰 H-1).
//
// 쥐기(`hold`): 쥔 동안의 바뀜은 보내지 않고 모았다가 마지막 쥔 쪽이 놓을 때 하나로 합쳐(버전 한 번) 보낸다 — 언어 서버에 한글 IME
// 조합 중의 글자를 보내지 않으려고 LSP 세션이 쓴다(조합이 끝난 뒤 한 번). 구문 세션은 쥐지 않는다.
//
// 마지막 쪽이 놓아도 닫기는 마이크로태스크 뒤로 미룬다(`DocSyncPool`). 문서를 디스크에서 다시 읽으면 세션이 먼저 새 텍스트로 열고,
// 그 뒤 화면이 뷰를 새로 만들며 같은 커밋에서 옛 뷰가 놓고 새 뷰가 잡는다 — 곧바로 닫으면 방금 연 문서를 닫고 같은 텍스트로 다시
// 연다.

import type { ChangeSet, Text } from "@codemirror/state";
import { ERROR_CODE, type TextChange } from "@pitwall/rpc-client";

import type { DocumentChange, DocumentStore } from "./documents";

/** 실패 뒤 다시 열기까지 처음 물러서는 시간(ms). 실패가 이어지면 두 배씩 늘린다. */
export const RETRY_BASE_MS = 250;
/** 물러서는 시간의 상한(ms). */
export const RETRY_MAX_MS = 30_000;
/** 이만큼 이어 실패하면 로그에 한 번 남긴다. */
export const RETRY_NOTICE_AFTER = 4;

/** CodeMirror 의 바뀜을 서비스의 차례 적용 바꾸기로(`SyntaxEditParams.changes` · `LspDocumentChangeParams.changes` 의 규칙). */
export function toTextChanges(changes: ChangeSet): TextChange[] {
  const out: TextChange[] = [];
  changes.iterChanges((fromA, toA, fromB, _toB, inserted) =>
    out.push({ from: fromB, to: fromB + (toA - fromA), insert: inserted.toString() }),
  );
  return out;
}

export function errorCode(e: unknown): number | undefined {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "number" ? code : undefined;
}

/** 다시 열면 맞춰지는 오류. */
export const resyncs = (e: unknown) => {
  const code = errorCode(e);
  return code === ERROR_CODE.CONFLICT || code === ERROR_CODE.NOT_FOUND;
};

/** 서비스 하나의 문서 RPC. */
export interface DocSyncProtocol {
  /** 로그 이름(`syntax` · `lsp`). */
  readonly name: string;
  /** 이 텍스트는 보내지 않는다(기준 밖). 없으면 늘 보낸다. */
  skip?(text: Text): boolean;
  /** `text` 로 문서를 연다. `off` 면 서비스가 열지 않았다(이 문서는 보내지 않는다). */
  open(text: Text): Promise<{ docId: string; off: boolean }>;
  /** 편집 하나(`version` 은 이 편집 뒤의 버전). `off` 면 서비스가 그 문서를 껐다. */
  change(edit: DocEdit): Promise<{ off: boolean }>;
  /** 문서를 닫는다(답을 기다리지 않는다). */
  close(docId: string): void;
}

export interface DocEdit {
  docId: string;
  version: number;
  changes: TextChange[];
}

/** 쓰는 쪽에 알리는 것. */
export interface DocSyncHooks {
  /** 바뀜 하나를 받았다(버전이 오른 뒤 — 쥐는 동안 모은 것은 놓을 때 합쳐 한 번). */
  edited?(changes: ChangeSet): void;
  /** 새 문서를 열기 시작한다(버전이 0 이 된다). */
  opening?(): void;
  /** 문서가 열렸다(`open` 의 답을 받았다). */
  opened?(docId: string): void;
  /** 보내지 않는 문서가 됐다(기준 밖 · 서비스가 끔). */
  off?(): void;
}

type Phase =
  /** 아직 텍스트가 없거나(문서를 읽는 중) 잡은 쪽이 없다. */
  | { kind: "idle" }
  | {
      kind: "opening";
      generation: number;
      opened: Promise<void>;
      queued: Omit<DocEdit, "docId">[];
    }
  | { kind: "open"; generation: number; docId: string }
  /** 실패해서 물러서 있다. `timer` 가 끝나면 다시 열고 `reopened` 가 풀린다. */
  | {
      kind: "waiting";
      timer: ReturnType<typeof setTimeout>;
      reopened: Promise<void>;
      wake: () => void;
    }
  | { kind: "off" };

/** 기다린 끝의 상태. */
export type Settled =
  | { kind: "open"; docId: string }
  /** 보내지 않는 문서(다시 읽기 전까지). */
  | { kind: "off" }
  /** 닫혔거나 아직 텍스트가 없다. */
  | { kind: "closed" };

/** 문서 하나의 동기화. 잡는 수는 `DocSyncPool` 이 센다. */
export class DocSync {
  holders = 0;
  /** 지금 버전. 서비스에 보낸(보낼) 마지막 편집의 버전이다 — 쥐고 있는 바뀜은 아직 들지 않는다. */
  version = 0;
  private phase: Phase = { kind: "idle" };
  /** 마지막으로 받은 텍스트(다시 열 때 보낸다). */
  private text: Text | undefined;
  /** 이어진 실패 수. 답을 받으면 0 이다. */
  private failures = 0;
  private generation = 0;
  /** Host 가 빠짐없이 받은 가장 큰 버전(머리 주석 「닿음」). 다시 열면 0. */
  private acked = 0;
  /** 답은 왔지만 그 앞 버전의 답이 아직인 버전. */
  private answeredEarly = new Set<number>();
  /** `synced` 를 기다리는 쪽. 버전이 닿거나 세대가 바뀌면 풀린다. */
  private waiters: { version: number; wake: () => void }[] = [];
  private holds = 0;
  /** 쥔 동안 모은 바뀜. */
  private pending: ChangeSet | undefined;
  private readonly unlisten: () => void;

  constructor(
    readonly path: string,
    documents: Pick<DocumentStore, "onChange">,
    private readonly protocol: DocSyncProtocol,
    private readonly hooks: DocSyncHooks = {},
  ) {
    this.unlisten = documents.onChange(path, (change) => this.changed(change));
  }

  /** 지금 상태(기다리지 않는다). */
  get state(): Phase["kind"] {
    return this.phase.kind;
  }

  /** 열린 문서 id. 여는 중 · 물러섬 · 끔이면 없다. */
  get docId(): string | undefined {
    return this.phase.kind === "open" ? this.phase.docId : undefined;
  }

  /** 쥔 바뀜이 있거나 쥐고 있다 — 서비스의 텍스트가 화면보다 뒤다. */
  get holding(): boolean {
    return this.holds > 0;
  }

  private changed(change: DocumentChange) {
    if (change.kind === "reset") {
      this.text = change.text;
      this.pending = undefined;
      this.open();
      return;
    }
    this.text = change.after;
    if (this.holds > 0) {
      this.pending = this.pending ? this.pending.compose(change.changes) : change.changes;
      return;
    }
    this.push(change.changes);
  }

  /** 바뀜 하나를 버전을 올려 보낸다. */
  private push(changes: ChangeSet) {
    this.version += 1;
    this.hooks.edited?.(changes);
    const phase = this.phase;
    if (phase.kind !== "opening" && phase.kind !== "open") return;
    const edit = { version: this.version, changes: toTextChanges(changes) };
    if (phase.kind === "opening") phase.queued.push(edit);
    else this.send({ docId: phase.docId, ...edit }, phase.generation);
  }

  /** 바뀜을 쥔다 — 돌려준 함수로 놓는다. 마지막이 놓으면 모은 바뀜을 하나로 보낸다. */
  hold(): () => void {
    this.holds += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      this.holds -= 1;
      if (this.holds > 0 || !this.pending) return;
      const pending = this.pending;
      this.pending = undefined;
      this.push(pending);
    };
  }

  private send(edit: DocEdit, generation: number) {
    this.protocol.change(edit).then(
      (result) => {
        if (!this.current(generation)) return;
        if (result.off) {
          this.goOff();
          return;
        }
        this.ack(edit.version);
      },
      (e: unknown) => {
        if (!this.current(generation)) return;
        if (resyncs(e)) this.open();
        else this.fail("edit", e);
      },
    );
  }

  /** 편집 `version` 의 답이 왔다. 1 부터 이어진 데까지 `acked` 를 올리고 기다리던 쪽을 깨운다. */
  private ack(version: number) {
    this.answeredEarly.add(version);
    while (this.answeredEarly.delete(this.acked + 1)) this.acked += 1;
    this.wake((w) => w.version <= this.acked);
  }

  private wake(which: (w: { version: number }) => boolean) {
    const ready = this.waiters.filter(which);
    if (ready.length === 0) return;
    this.waiters = this.waiters.filter((w) => !which(w));
    for (const w of ready) w.wake();
  }

  /**
   * Host 가 버전 `version` 까지 받을 때까지 기다린다. 그사이 다시 열거나 · 끄거나 · 물러서거나 · 닫으면 곧바로 풀린다 — 부른 쪽은
   * 풀린 뒤 문서 id · 버전을 다시 본다.
   */
  synced(version: number, signal?: AbortSignal): Promise<void> {
    if (this.phase.kind !== "open" || version <= this.acked || signal?.aborted)
      return Promise.resolve();
    return new Promise((resolve) => {
      // 부른 쪽이 그만두면(같은 종류 새 요청) 기다림을 빼고 곧바로 푼다 — Host 가 편집에 답하지 않는 동안 쌓이지 않게(재리뷰 N-3).
      const waiter = {
        version,
        wake: () => {
          signal?.removeEventListener("abort", abort);
          resolve();
        },
      };
      const abort = () => {
        this.waiters = this.waiters.filter((w) => w !== waiter);
        resolve();
      };
      signal?.addEventListener("abort", abort, { once: true });
      this.waiters.push(waiter);
    });
  }

  /** 지금 텍스트(바뀔 때마다 새 객체). 부른 쪽이 잰 위치가 아직 맞는지 동일성으로 본다(재리뷰 N-2). */
  get currentText(): Text | undefined {
    return this.text;
  }

  /** 세대가 바뀌었다 — 닿음을 처음으로 하고 기다리던 쪽을 모두 깨운다. */
  private resetAcks() {
    this.acked = 0;
    this.answeredEarly.clear();
    this.wake(() => true);
  }

  private goOff() {
    this.phase = { kind: "off" };
    this.resetAcks();
    this.hooks.off?.();
  }

  private current(generation: number) {
    const phase = this.phase;
    return (phase.kind === "opening" || phase.kind === "open") && phase.generation === generation;
  }

  /** `docId` 가 지금 연 문서다. */
  isCurrent(docId: string): boolean {
    return this.phase.kind === "open" && this.phase.docId === docId;
  }

  /** 지금 텍스트로 새 문서를 연다. 앞 문서는 닫는다. */
  open() {
    this.closeServerDoc();
    this.stopWaiting();
    this.resetAcks();
    // 새로 여는 텍스트(`this.text`)가 쥔 바뀜을 이미 담는다 — 남기면 놓을 때 같은 바뀜을 한 번 더 보낸다(리뷰 M-1).
    this.pending = undefined;
    const text = this.text;
    if (!text || this.holders === 0) {
      this.phase = { kind: "idle" };
      return;
    }
    this.version = 0;
    this.hooks.opening?.();
    if (this.protocol.skip?.(text)) {
      this.goOff();
      return;
    }
    const generation = ++this.generation;
    const queued: Omit<DocEdit, "docId">[] = [];
    const opened = this.protocol.open(text).then(
      ({ docId, off }) => {
        if (!this.current(generation)) {
          // 그사이 다시 열었거나 닫았다. 닫기가 열기보다 먼저 도착했을 수 있다.
          if (!off) this.protocol.close(docId);
          return;
        }
        if (off) {
          this.goOff();
          return;
        }
        this.phase = { kind: "open", generation, docId };
        this.hooks.opened?.(docId);
        for (const edit of queued) this.send({ docId, ...edit }, generation);
      },
      (e: unknown) => {
        if (this.current(generation)) this.fail("open", e);
      },
    );
    this.phase = { kind: "opening", generation, opened, queued };
  }

  /** 다시 열면 맞춰질 수도 있는 실패(연결이 끊김 등) — 앞 문서를 닫고 물러섰다가 지금 텍스트로 다시 연다. */
  fail(method: string, e: unknown) {
    this.closeServerDoc();
    this.failures += 1;
    const delay = Math.min(RETRY_BASE_MS * 2 ** (this.failures - 1), RETRY_MAX_MS);
    if (this.failures === RETRY_NOTICE_AFTER)
      console.warn(
        `[editor] ${this.protocol.name}.${method} keeps failing (${this.failures} times), retrying with backoff up to ${RETRY_MAX_MS} ms`,
        this.path,
        e,
      );
    let wake = () => {};
    const reopened = new Promise<void>((resolve) => (wake = resolve));
    const timer = setTimeout(() => this.open(), delay);
    this.phase = { kind: "waiting", timer, reopened, wake };
    this.resetAcks();
  }

  /** 요청이 `docId` 에서 실패했다: 다시 열면 맞춰지는 오류면 다시 열고, 그 밖은 물러선다. 지난 문서의 실패는 무시한다. */
  failed(docId: string, method: string, e: unknown) {
    if (!this.isCurrent(docId)) return;
    if (resyncs(e)) this.open();
    else this.fail(method, e);
  }

  /** 답을 받았다 — 물러서는 시간을 처음으로. */
  answered() {
    this.failures = 0;
  }

  /** 서비스가 이 문서를 껐다(답에 실렸다). 다시 읽을 때까지 보내지 않는다. */
  turnOff() {
    this.goOff();
  }

  /** 물러서 있었으면 그만두고 기다리던 요청을 깨운다. */
  private stopWaiting() {
    const phase = this.phase;
    if (phase.kind !== "waiting") return;
    clearTimeout(phase.timer);
    phase.wake();
    this.phase = { kind: "idle" };
  }

  private closeServerDoc() {
    const phase = this.phase;
    // 여는 중이면 답이 온 뒤 `open` 의 답 처리가 닫는다 — 그때는 세대가 바뀌었거나 닫혔다.
    if (phase.kind === "open") this.protocol.close(phase.docId);
  }

  /** 여는 중 · 물러섬이면 끝날 때까지 기다린다. */
  async settled(): Promise<Settled> {
    let phase = this.phase;
    while (phase.kind === "opening" || phase.kind === "waiting") {
      await (phase.kind === "opening" ? phase.opened : phase.reopened);
      phase = this.phase;
    }
    if (phase.kind === "open") return { kind: "open", docId: phase.docId };
    if (phase.kind === "off") return { kind: "off" };
    return { kind: "closed" };
  }

  /** 잡은 쪽이 생겼는데 텍스트를 받고도 아직 열지 않았다 — 처음 잡은 것이다. */
  start() {
    if (this.phase.kind === "idle" && this.text) this.open();
  }

  /** 끝낸다: 구독을 풀고 서비스의 문서를 닫는다. */
  dispose() {
    this.unlisten();
    this.closeServerDoc();
    this.stopWaiting();
    this.phase = { kind: "idle" };
    this.resetAcks();
  }
}

/** 키(언어 · 경로)마다 `DocSync` 하나를 잡는 수로 나눠 쓴다. 마지막이 놓으면 마이크로태스크 뒤 끝낸다(머리 주석). */
export class DocSyncPool<S extends { sync: DocSync }> {
  private readonly entries = new Map<string, S>();

  constructor(private readonly create: (key: string) => S) {}

  get(key: string): S | undefined {
    return this.entries.get(key);
  }

  values(): IterableIterator<S> {
    return this.entries.values();
  }

  /** 잡는다. 돌려준 함수로 놓는다(두 번 불러도 한 번). */
  acquire(key: string): { entry: S; release: () => void } {
    let entry = this.entries.get(key);
    if (!entry) {
      entry = this.create(key);
      this.entries.set(key, entry);
    }
    const held = entry;
    held.sync.holders += 1;
    held.sync.start();
    let released = false;
    return {
      entry: held,
      release: () => {
        if (released) return;
        released = true;
        held.sync.holders -= 1;
        if (held.sync.holders > 0) return;
        // 같은 커밋에서 다시 잡으면 그대로 쓴다(머리 주석).
        queueMicrotask(() => {
          if (held.sync.holders > 0 || this.entries.get(key) !== held) return;
          held.sync.dispose();
          this.entries.delete(key);
        });
      },
    };
  }
}
