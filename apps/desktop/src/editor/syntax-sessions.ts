// 구문 세션: 열린 문서 하나(와 그 언어)마다 Host 의 구문 분석 서비스(crates/pw-syntax, 12-m1-plan M1-10)에 문서 하나를 연다.
// 강조 기능(extensions/highlight.ts)의 뷰가 `acquire` 로 잡고, 마지막 뷰가 놓으면 닫는다. 같은 파일을 두 그룹이 보여도 세션은
// 하나다 — 편집은 문서 모델의 바뀜 구독(`DocumentStore.onChange`, 문서마다 한 번)에서 받아 `syntax.edit` 으로 보낸다.
//
// 버전: `syntax.open` 의 문서가 0 이고 바뀜마다 1씩 오른다. 강조 결과는 그것을 계산한 버전을 싣고 온다. 지금 버전보다 지난
// 결과는 버리지 않고 그 뒤의 바뀜(최근 `MAPPED_VERSIONS` 개를 들고 있다)으로 위치를 옮겨 준다(`exact: false`) — 버리기만 하면
// 왕복이 타자 간격보다 길 때 응답이 늘 지난 것이라 강조가 영영 따라오지 못한다(30초 타이핑 시험에서 응답의 90 % 가 1~3 버전
// 늦었다). 다만 지난 버전의 빈 결과는 옮기지 않고 다시 묻는다 — 서비스는 새 편집이 와서 하던 파싱을 그만두면 그 파싱의 버전으로
// 빈 구간을 답하므로(crates/pw-syntax service.rs `Reparse::Stopped`), 칠하면 화면 강조가 한 왕복 동안 통째로 지워진다. 옮긴 강조는 그사이 친 글자의 영향(따옴표 · 주석 기호)을 모르므로 뷰는 칠한 뒤 다시 묻는다. 웹뷰의 RPC 프레임은 순서가
// 보장되지 않으므로(methods.rs 머리 주석) 편집마다 버전을 붙이고, `syntax.open` 의 응답을 받기 전에 생긴 편집은 모아 두었다가
// 응답 뒤에 차례로 보낸다. 같은 까닭으로 강조 요청이 앞서 보낸 편집보다 먼저 도착해 지난 버전으로 답하는 것은 정상이다.
//
// 어긋나면 다시 연다: 편집이 conflict · not found(서버가 오래 안 쓴 문서를 닫았다 — `MAX_DOCUMENTS`)로 끝나거나, 강조가 not found
// 거나, 결과의 버전이 지금보다 앞서면 지금 텍스트로 새 문서 id 를 연다. 문서 id 를 새로 쓰는 것은 옛 id 로
// 늦게 도착한 편집이 새 문서에 들어가지 않게 하려는 것이다. 문서 id 는 창마다 다르다 — 서비스는 Host 하나에 하나라 여러 창이
// 같은 파일을 열 수 있다.
//
// 그 밖의 실패(연결이 끊김 등)는 물러섰다가 지금 텍스트로 다시 연다 — 물러서는 시간은 실패가 이어질 때마다 두 배로 늘고
// `RETRY_MAX_MS` 에서 멈춘다. 강조 응답을 받으면 처음으로 돌아간다. `RETRY_NOTICE_AFTER` 번 이어 실패하면 로그에 한 번 남긴다.
// 물러서 있는 동안 강조 요청은 다시 열 때까지 기다린다(뷰는 옛 강조를 그대로 둔다).
//
// 강조하지 않는 문서(`plain`): 그 언어의 문법이 없음, 대용량 기준(limits.ts, 결정 M1-D9) 밖, 서버가 끔(`off` — 크기 · 줄 길이 ·
// 시간 상한). 문서를 다시 읽으면(`reset`) 다시 본다.
//
// 마지막 뷰가 놓아도 닫기는 마이크로태스크 뒤로 미룬다. 문서를 디스크에서 다시 읽으면 세션이 먼저 새 텍스트로 열고, 그 뒤
// 화면이 뷰를 새로 만들며 같은 커밋에서 옛 뷰가 놓고 새 뷰가 잡는다 — 곧바로 닫으면 방금 연 문서를 닫고 같은 텍스트로 다시 연다.

import type { ChangeSet, Text } from "@codemirror/state";
import {
  ERROR_CODE,
  type HighlightSpan,
  type SyntaxCloseParams,
  type SyntaxEditParams,
  type SyntaxEditResult,
  type SyntaxHighlightsParams,
  type SyntaxHighlightsResult,
  type SyntaxOpenParams,
  type SyntaxOpenResult,
  type TextChange,
  type TextRange,
} from "@pitwall/rpc-client";

import type { DocumentChange, DocumentStore } from "./documents";
import { overHighlightLimit } from "./limits";

/** 구문 분석 서비스의 문서 세션 RPC. 창은 Host 연결을 넘기고 시험은 가짜를 넘긴다. */
export interface SyntaxSource {
  open(params: SyntaxOpenParams): Promise<SyntaxOpenResult>;
  edit(params: SyntaxEditParams): Promise<SyntaxEditResult>;
  highlights(params: SyntaxHighlightsParams): Promise<SyntaxHighlightsResult>;
  close(params: SyntaxCloseParams): Promise<unknown>;
}

export type HighlightAnswer =
  /**
   * 지금 버전(`version`)의 위치로 된 구간. `exact` 가 거짓이면 지난 버전에서 계산해 위치만 옮긴 것이다 — 그사이 바뀐 곳의 강조는
   * 틀릴 수 있으니 다시 묻는다.
   */
  | { kind: "spans"; version: number; spans: HighlightSpan[]; exact: boolean }
  /** 그사이 바뀌었다 — 다시 묻는다. */
  | { kind: "stale" }
  /** 이 문서는 강조하지 않는다(다시 읽기 전까지). */
  | { kind: "plain" };

export interface SyntaxHandle {
  /** 지금 버전. 문서가 바뀔 때마다 오른다. */
  readonly version: number;
  /** 범위(UTF-16 위치)의 강조. 문서를 여는 중이면 열린 뒤에 묻는다. */
  highlights(range: TextRange): Promise<HighlightAnswer>;
  /** 놓는다. 마지막이면 서비스의 문서를 닫는다. */
  release(): void;
}

export interface SyntaxSessions {
  /** 문서 `path` 를 언어 `language`(Linguist 이름)로 강조하는 세션을 잡는다. */
  acquire(path: string, language: string): SyntaxHandle;
}

/** CodeMirror 의 바뀜을 서비스의 차례 적용 바꾸기로(`SyntaxEditParams.changes` 의 규칙). */
export function toTextChanges(changes: ChangeSet): TextChange[] {
  const out: TextChange[] = [];
  changes.iterChanges((fromA, toA, fromB, _toB, inserted) =>
    out.push({ from: fromB, to: fromB + (toA - fromA), insert: inserted.toString() }),
  );
  return out;
}

function errorCode(e: unknown): number | undefined {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "number" ? code : undefined;
}

/** 다시 열면 맞춰지는 오류. */
const resyncs = (e: unknown) => {
  const code = errorCode(e);
  return code === ERROR_CODE.CONFLICT || code === ERROR_CODE.NOT_FOUND;
};

type Phase =
  /** 아직 텍스트가 없다(문서를 읽는 중). */
  | { kind: "idle" }
  | { kind: "opening"; docId: string; opened: Promise<void>; queued: SyntaxEditParams[] }
  | { kind: "open"; docId: string }
  /** 실패해서 물러서 있다. `timer` 가 끝나면 다시 열고 `reopened` 가 풀린다. */
  | {
      kind: "waiting";
      timer: ReturnType<typeof setTimeout>;
      reopened: Promise<void>;
      wake: () => void;
    }
  | { kind: "plain" };

/** 지난 결과를 옮기려고 들고 있는 최근 바뀜의 수. 이보다 늦은 결과는 버린다. */
export const MAPPED_VERSIONS = 64;

/** 실패 뒤 다시 열기까지 처음 물러서는 시간(ms). 실패가 이어지면 두 배씩 늘린다. */
export const RETRY_BASE_MS = 250;
/** 물러서는 시간의 상한(ms). */
export const RETRY_MAX_MS = 30_000;
/** 이만큼 이어 실패하면 로그에 한 번 남긴다. */
export const RETRY_NOTICE_AFTER = 4;

/** 지난 버전의 구간을 `mapping` 뒤의 위치로. 바뀐 경계에 끼운 글자는 구간에 넣지 않는다. 빈 구간은 버린다. */
function mapSpans(spans: readonly HighlightSpan[], mapping: ChangeSet): HighlightSpan[] {
  const out: HighlightSpan[] = [];
  for (const span of spans) {
    const from = mapping.mapPos(span.from, 1);
    const to = mapping.mapPos(span.to, -1);
    if (from < to) out.push({ ...span, from, to });
  }
  return out;
}

let windowSeq = 0;

export function createSyntaxSessions(
  source: SyntaxSource,
  documents: Pick<DocumentStore, "onChange">,
): SyntaxSessions {
  // 창마다 다른 앞부분. 같은 Host 에 붙은 다른 창의 문서 id 와 겹치지 않게 한다.
  const prefix = `w${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}${windowSeq++}`;
  let docSeq = 0;
  const sessions = new Map<string, Session>();

  class Session {
    holders = 0;
    version = 0;
    phase: Phase = { kind: "idle" };
    /** 마지막으로 받은 텍스트(다시 열 때 보낸다). */
    text: Text | undefined;
    /** 최근 바뀜. 끝이 지금 버전으로 온 바뀜이다(`recent[recent.length - k]` 는 버전 `version - k` 에서 온 것). */
    recent: ChangeSet[] = [];
    /** 이어진 실패 수. 강조 응답을 받으면 0 이다. */
    failures = 0;
    readonly unlisten: () => void;

    constructor(
      readonly key: string,
      readonly path: string,
      readonly language: string,
    ) {
      this.unlisten = documents.onChange(path, (change) => this.changed(change));
    }

    changed(change: DocumentChange) {
      if (change.kind === "reset") {
        this.text = change.text;
        this.open();
        return;
      }
      this.text = change.after;
      this.version += 1;
      this.recent.push(change.changes);
      if (this.recent.length > MAPPED_VERSIONS) this.recent.shift();
      const phase = this.phase;
      if (phase.kind !== "opening" && phase.kind !== "open") return;
      const edit = {
        docId: phase.docId,
        version: this.version,
        changes: toTextChanges(change.changes),
      };
      if (phase.kind === "opening") phase.queued.push(edit);
      else this.send(edit);
    }

    send(edit: SyntaxEditParams) {
      source.edit(edit).then(
        (result) => {
          if (result.off && this.current(edit.docId)) this.phase = { kind: "plain" };
        },
        (e: unknown) => {
          if (!this.current(edit.docId)) return;
          if (resyncs(e)) this.open();
          else this.fail("edit", e);
        },
      );
    }

    /** `docId` 가 지금 연(여는) 문서다. */
    current(docId: string) {
      const phase = this.phase;
      return (phase.kind === "opening" || phase.kind === "open") && phase.docId === docId;
    }

    /** 지금 텍스트로 새 문서 id 를 연다. 앞 문서는 닫는다. */
    open() {
      this.closeServerDoc();
      this.stopWaiting();
      const text = this.text;
      if (!text || this.holders === 0) {
        this.phase = { kind: "idle" };
        return;
      }
      this.version = 0;
      this.recent = [];
      if (overHighlightLimit(text)) {
        this.phase = { kind: "plain" };
        return;
      }
      const docId = `${prefix}-${docSeq++}`;
      const queued: SyntaxEditParams[] = [];
      const opened = source.open({ docId, language: this.language, text: text.toString() }).then(
        (result) => {
          if (!this.current(docId)) {
            // 그사이 다시 열었거나 닫았다. 닫기가 열기보다 먼저 도착했을 수 있다.
            if (result.grammar !== null) void source.close({ docId }).catch(() => {});
            return;
          }
          if (result.grammar === null) {
            this.phase = { kind: "plain" };
            return;
          }
          this.phase = { kind: "open", docId };
          for (const edit of queued) this.send(edit);
        },
        (e: unknown) => {
          if (this.current(docId)) this.fail("open", e);
        },
      );
      this.phase = { kind: "opening", docId, opened, queued };
    }

    /** 다시 열면 맞춰질 수도 있는 실패(연결이 끊김 등) — 앞 문서를 닫고 물러섰다가 지금 텍스트로 다시 연다. */
    fail(method: string, e: unknown) {
      this.closeServerDoc();
      this.failures += 1;
      const delay = Math.min(RETRY_BASE_MS * 2 ** (this.failures - 1), RETRY_MAX_MS);
      if (this.failures === RETRY_NOTICE_AFTER)
        console.warn(
          `[editor] syntax.${method} keeps failing (${this.failures} times), retrying with backoff up to ${RETRY_MAX_MS} ms`,
          this.path,
          e,
        );
      let wake = () => {};
      const reopened = new Promise<void>((resolve) => (wake = resolve));
      const timer = setTimeout(() => this.open(), delay);
      this.phase = { kind: "waiting", timer, reopened, wake };
    }

    /** 물러서 있었으면 그만두고 기다리던 요청을 깨운다. */
    stopWaiting() {
      const phase = this.phase;
      if (phase.kind !== "waiting") return;
      clearTimeout(phase.timer);
      phase.wake();
      this.phase = { kind: "idle" };
    }

    closeServerDoc() {
      const phase = this.phase;
      if (phase.kind === "opening" || phase.kind === "open")
        void source.close({ docId: phase.docId }).catch(() => {});
    }

    async highlights(range: TextRange): Promise<HighlightAnswer> {
      let phase = this.phase;
      while (phase.kind === "opening" || phase.kind === "waiting") {
        await (phase.kind === "opening" ? phase.opened : phase.reopened);
        phase = this.phase;
      }
      if (phase.kind === "plain") return { kind: "plain" };
      if (phase.kind !== "open") return { kind: "stale" };
      const { docId } = phase;
      let result: SyntaxHighlightsResult;
      try {
        result = await source.highlights({ docId, range });
      } catch (e) {
        if (!this.current(docId)) return { kind: "stale" };
        if (resyncs(e)) this.open();
        else this.fail("highlights", e);
        return { kind: "stale" };
      }
      if (!this.current(docId))
        return this.phase.kind === "plain" ? { kind: "plain" } : { kind: "stale" };
      this.failures = 0;
      if (result.off) {
        this.phase = { kind: "plain" };
        return { kind: "plain" };
      }
      const lag = this.version - result.version;
      if (lag === 0)
        return { kind: "spans", version: this.version, spans: result.spans, exact: true };
      // 지난 버전의 빈 결과는 새 편집이 그만두게 한 파싱일 수 있다 — 칠하지 않고(옛 강조를 두고) 다시 묻는다.
      if (lag > 0 && result.spans.length === 0) return { kind: "stale" };
      if (lag > 0 && lag <= this.recent.length) {
        const mapping = this.recent.slice(-lag).reduce((a, b) => a.compose(b));
        return {
          kind: "spans",
          version: this.version,
          spans: mapSpans(result.spans, mapping),
          exact: false,
        };
      }
      // 서버가 받은 편집이 이쪽보다 많다 — 어긋났다.
      if (lag < 0) this.open();
      return { kind: "stale" };
    }

    release() {
      this.holders -= 1;
      if (this.holders > 0) return;
      // 같은 커밋에서 다시 잡으면 그대로 쓴다(머리 주석).
      queueMicrotask(() => {
        if (this.holders > 0 || sessions.get(this.key) !== this) return;
        this.unlisten();
        this.closeServerDoc();
        this.stopWaiting();
        this.phase = { kind: "idle" };
        sessions.delete(this.key);
      });
    }
  }

  return {
    acquire(path, language) {
      const key = `${language}\u0000${path}`;
      let session = sessions.get(key);
      if (!session) {
        session = new Session(key, path, language);
        sessions.set(key, session);
      }
      session.holders += 1;
      // 텍스트를 받았는데(구독하자마자 온 `reset`) 아직 열지 않았다 — 처음 잡은 것이다.
      if (session.phase.kind === "idle" && session.text) session.open();
      const held = session;
      let released = false;
      return {
        get version() {
          return held.version;
        },
        highlights: (range) => held.highlights(range),
        release() {
          if (released) return;
          released = true;
          held.release();
        },
      };
    },
  };
}
