// 심볼로 이동(⌘⌥O) · 클래스로 이동(⌘O) · Search Everywhere 「심볼」 탭의 질의(15-m4-plan M4-16): 심볼 인덱스(M4-15
// `index.symbols.search`)와 언어 서버(`lsp.workspaceSymbol` — 서버가 준비된 언어만)를 함께 묻고 합친다.
//
// - 두 길은 따로 「요청 하나씩 · 마지막 것만 이어 보냄」(M1-19 의 파일 검색과 같은 규칙 — file-search.ts): 도는 동안 친 글자는 마지막
//   것만 남겼다가 답이 오면 보낸다. 느린 서버가 인덱스의 차례를 막지 않는다.
// - 합치기: 같은 (파일, 줄, 이름)은 하나로, 서버 결과를 앞에(서버의 정의 순서 · 서버가 준 순서), 그 뒤 인덱스 결과(점수순). 파일은
//   루트 기준 상대 경로로 맞춘다 — 루트가 심링크를 지나도 두 출처의 열쇠가 같다.
// - 200 ms 규칙: 인덱스 답이 와도 서버 답을 `LSP_GRACE_MS` 까지 기다렸다 함께 그린다(목록이 두 번 뒤바뀌지 않게). 그때까지 서버가
//   답하지 않으면 인덱스 결과를 먼저 그리고 서버 답이 오면 앞에 더한다. 지난 답에서 물을 서버가 없었으면(미신뢰 · 서버 없음 · 준비
//   전) 기다리지 않는다. 서버 쪽 실패는 조용히 넘긴다(인덱스 결과만 — 미신뢰면 서버가 없다, 결정 M4-D8).
// - 인덱스의 「바쁨」(`isBusyError` — `the symbol index is busy`)이면 결과를 두고 `BUSY_RETRY_MS` 뒤 마지막 질의를 다시 보낸다.
// - 결과에 `unindexed` 루트가 있으면 `index.status` 를 묻고, 그 루트가 `suspended`(파일 목록 작업이 취소됨 — M4-15 ⓘ)면
//   `suspended` 를 켠다 — 화면이 「파일 목록이 취소됨 — 다시 만들기」 를 보이고, 그 동작(`rebuild`)은 파일 목록을 다시 만들게 한 뒤
//   (`search.files` — `index.*` 요청은 목록을 다시 만들지 않는다) 마지막 질의를 다시 보낸다.
// - 상한(출처와 무관하게 여기 한 곳): 결과 `SYMBOL_LIMIT` 개, 이름 · 컨테이너 `NAME_LIMIT` 글자. 인덱스 심볼에는 깊이 · 수 상한이 없다
//   (M4-10 보고) — 요청의 `limit` 와 이 자르기가 소비자 쪽 상한이다.

import {
  isBusyError,
  type IndexStatus,
  type IndexSymbolKind,
  type IndexSymbolSearch,
  type IndexSymbolSearchParams,
  type LspWorkspaceSymbolParams,
  type LspWorkspaceSymbols,
} from "@pitwall/rpc-client";

import { kindOfLsp, lspKindsOf, TYPE_KINDS } from "./symbol-kinds";

/** 한 질의에서 보이는 결과 수(두 출처를 합친 뒤 — 파일 탭과 같다). 두 출처에도 이만큼 묻는다. */
export const SYMBOL_LIMIT = 50;
/** 이름 · 컨테이너를 그리는 상한(UTF-16 단위). Host 의 상한(256 바이트)과 같은 크기다 — 출처와 무관하게 여기서 한 번 더 자른다. */
export const NAME_LIMIT = 256;
/** 인덱스 답이 와도 서버 답을 기다리는 시간. */
export const LSP_GRACE_MS = 200;
/** 「바쁨」 이면 이만큼 뒤에 다시 보낸다(파일 검색과 같다). */
export const BUSY_RETRY_MS = 250;

/** 찾는 곳: 프로젝트와 창의 활성 워크트리 꼭대기. */
export interface SymbolScope {
  readonly projectId: string;
  readonly worktree: string | null;
}

/** 결과 한 줄. */
export interface SymbolRow {
  /** 겹침 열쇠(파일 · 줄 · 이름). */
  readonly key: string;
  readonly name: string;
  readonly kind: IndexSymbolKind;
  readonly container?: string;
  /** 연 경로(루트 안이면 절대 경로, 루트 밖이면 서버가 준 경로 · URI). */
  readonly path: string;
  /** 루트 기준 상대 경로(`/`). 루트 밖이면 없다. */
  readonly relative?: string;
  /** 1 부터. */
  readonly line: number;
  /** 1 부터, UTF-16. */
  readonly column: number;
  /** 루트 밖 위치의 읽기 토큰(M4-08 의 토큰 길로만 연다). */
  readonly token?: string;
  /** 답한 서버(서버 결과만). */
  readonly server?: string;
  /** 외부 소스 탭의 언어(서버 결과만 — 그 서버에 이 창이 연 문서의 언어). */
  readonly language?: string;
  /** `name` 안의 일치 글자 위치(UTF-16). */
  readonly positions: readonly number[];
  readonly source: "lsp" | "index";
}

export interface SymbolSearchState {
  /** 결과(`rows`)가 가리키는 질의. 아직 받은 것이 없으면 `undefined`. */
  readonly query?: string;
  readonly types?: boolean;
  /** 결과를 찾은 곳 — 루트 안 결과는 이 워크트리로 연다. */
  readonly scope?: SymbolScope;
  readonly rows: readonly SymbolRow[];
  /** 마지막 질의의 답을 다 받지 못했다. */
  readonly pending: boolean;
  /** 인덱스 결과를 먼저 그렸고 서버 답을 기다린다. */
  readonly lspPending: boolean;
  /** 인덱스가 바쁘다(곧 다시 보낸다). */
  readonly busy: boolean;
  /** 인덱스 요청의 실패 문장(바쁨 포함). 다음 답이 오면 지운다. */
  readonly error?: string;
  /** 파일 목록 작업이 취소돼 인덱스가 비었다(다시 만들기를 보인다). */
  readonly suspended: boolean;
  /** 상한에 걸려 일부를 버렸다. */
  readonly truncated: boolean;
}

export interface SymbolSearchDeps {
  index(params: IndexSymbolSearchParams, signal: AbortSignal): Promise<IndexSymbolSearch>;
  /** 없으면 서버에 묻지 않는다(파일 창 · 시험). */
  lsp?(params: LspWorkspaceSymbolParams, signal: AbortSignal): Promise<LspWorkspaceSymbols>;
  status(scope: SymbolScope): Promise<IndexStatus>;
  /** 파일 목록을 다시 만들게 한다(`search.files`). */
  rebuild(scope: SymbolScope): Promise<unknown>;
}

interface Wanted {
  readonly text: string;
  readonly types: boolean;
  readonly scope: SymbolScope;
}

/** 한 질의의 두 답. */
interface Answers {
  readonly wanted: Wanted;
  index?: IndexSymbolSearch;
  indexError?: string;
  lsp?: LspWorkspaceSymbols;
  /** 서버 쪽이 끝났다(답 · 실패 · 물을 서버 없음). */
  lspDone: boolean;
  /** 기다림이 지났다. */
  graceOver: boolean;
  suspended: boolean;
}

const EMPTY: SymbolSearchState = {
  rows: [],
  pending: false,
  lspPending: false,
  busy: false,
  suspended: false,
  truncated: false,
};

function sameWanted(a: Wanted | undefined, b: Wanted): boolean {
  return (
    a !== undefined &&
    a.text === b.text &&
    a.types === b.types &&
    a.scope.projectId === b.scope.projectId &&
    a.scope.worktree === b.scope.worktree
  );
}

/** 「요청 하나씩 · 마지막 것만 이어 보냄」 의 한 길. */
class Lane<R> {
  private inFlight?: AbortController;
  private next?: Wanted;
  private retry?: ReturnType<typeof setTimeout>;
  private generation = 0;

  constructor(
    private readonly send: (wanted: Wanted, signal: AbortSignal) => Promise<R>,
    private readonly done: (
      wanted: Wanted,
      outcome: { ok: R } | { error: unknown },
    ) => "retry" | void,
  ) {}

  want(wanted: Wanted) {
    this.next = wanted;
    this.pump();
  }

  stop() {
    this.generation++;
    clearTimeout(this.retry);
    this.retry = undefined;
    this.inFlight?.abort();
    this.inFlight = undefined;
    this.next = undefined;
  }

  private pump() {
    if (this.inFlight || this.retry || !this.next) return;
    const wanted = this.next;
    this.next = undefined;
    const generation = this.generation;
    const abort = new AbortController();
    this.inFlight = abort;
    const finish = (outcome: { ok: R } | { error: unknown }) => {
      if (generation !== this.generation) return;
      this.inFlight = undefined;
      if (this.done(wanted, outcome) === "retry") {
        // 그사이 새 질의가 왔으면 그것을, 아니면 이 질의를 다시.
        this.next ??= wanted;
        this.retry = setTimeout(() => {
          this.retry = undefined;
          this.pump();
        }, BUSY_RETRY_MS);
        return;
      }
      this.pump();
    };
    this.send(wanted, abort.signal).then(
      (ok) => finish({ ok }),
      (error: unknown) => finish({ error }),
    );
  }
}

export class SymbolSearch {
  private state: SymbolSearchState = EMPTY;
  private readonly listeners = new Set<() => void>();
  private current?: Answers;
  private grace?: ReturnType<typeof setTimeout>;
  /** 지난 서버 답에 물을 서버가 있었는가. 없었으면(미신뢰 · 서버 없음) 200 ms 를 기다리지 않는다. */
  private lspExpected = true;
  private disposed = false;
  private readonly indexLane: Lane<IndexSymbolSearch>;
  private readonly lspLane?: Lane<LspWorkspaceSymbols>;

  constructor(private readonly deps: SymbolSearchDeps) {
    this.indexLane = new Lane(
      (w, signal) =>
        deps.index(
          {
            projectId: w.scope.projectId,
            worktree: w.scope.worktree,
            query: w.text,
            kinds: w.types ? [...TYPE_KINDS] : null,
            limit: SYMBOL_LIMIT,
            offset: null,
          },
          signal,
        ),
      (w, outcome) => this.indexDone(w, outcome),
    );
    if (deps.lsp) {
      const lsp = deps.lsp;
      this.lspLane = new Lane(
        (w, signal) =>
          lsp(
            {
              projectId: w.scope.projectId,
              worktree: w.scope.worktree,
              query: w.text,
              kinds: w.types ? lspKindsOf(TYPE_KINDS) : null,
              limit: SYMBOL_LIMIT,
            },
            signal,
          ),
        (w, outcome) => this.lspDone(w, outcome),
      );
    }
  }

  get = (): SymbolSearchState => this.state;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** `text` 를 찾는다(`types` 면 타입 종류만 — 클래스로 이동). 받은 것이 이미 그 질의 · 범위의 것이면 다시 보내지 않는다. */
  request(text: string, types: boolean, scope: SymbolScope) {
    const wanted: Wanted = { text, types, scope };
    if (sameWanted(this.current?.wanted, wanted) && !this.state.error) return;
    this.current = {
      wanted,
      lspDone: !this.lspLane,
      graceOver: false,
      suspended: false,
    };
    clearTimeout(this.grace);
    this.grace = setTimeout(() => {
      if (this.current?.wanted !== wanted) return;
      this.current.graceOver = true;
      this.render();
    }, LSP_GRACE_MS);
    this.set({ pending: true });
    this.indexLane.want(wanted);
    this.lspLane?.want(wanted);
  }

  /** 기다리는 질의를 버리고 재시도를 멈추고 도는 요청을 취소한다. 받은 결과는 둔다. 팝업을 닫거나 · 탭을 떠나거나 · 질의가 비면. */
  stop() {
    this.indexLane.stop();
    this.lspLane?.stop();
    clearTimeout(this.grace);
    this.grace = undefined;
    this.current = undefined;
    if (this.state.pending || this.state.error !== undefined || this.state.lspPending)
      this.set({ pending: false, lspPending: false, busy: false, error: undefined });
  }

  /** `stop` 하고 받은 결과도 버린다(팝업을 새로 열 때 — 그사이 파일이 바뀌었을 수 있다). */
  forget() {
    this.stop();
    this.lspExpected = true;
    this.state = EMPTY;
  }

  dispose() {
    this.stop();
    this.disposed = true;
    this.listeners.clear();
  }

  /** 파일 목록을 다시 만들게 하고(취소된 목록) 마지막 질의를 다시 보낸다. */
  async rebuild() {
    const scope = this.current?.wanted.scope ?? this.state.scope;
    if (!scope) return;
    this.set({ suspended: false, pending: true });
    try {
      await this.deps.rebuild(scope);
    } catch (e) {
      this.set({ pending: false, error: e instanceof Error ? e.message : String(e) });
      return;
    }
    const wanted = this.current?.wanted;
    if (!wanted) return;
    this.current = undefined;
    this.request(wanted.text, wanted.types, wanted.scope);
  }

  private indexDone(
    wanted: Wanted,
    outcome: { ok: IndexSymbolSearch } | { error: unknown },
  ): "retry" | void {
    const answers = this.current;
    const ours = answers !== undefined && answers.wanted === wanted;
    if ("error" in outcome) {
      const busy = isBusyError(outcome.error);
      const message =
        outcome.error instanceof Error ? outcome.error.message : String(outcome.error);
      if (busy) {
        // 결과는 그대로 두고 조금 뒤 마지막 질의를 다시 보낸다.
        this.set({ busy: true, error: message, pending: true });
        return ours && !this.disposed ? "retry" : undefined;
      }
      if (ours) {
        answers.indexError = message;
        this.render();
      }
      return;
    }
    if (!ours) return;
    answers.index = outcome.ok;
    if (outcome.ok.unindexed.length > 0) void this.checkSuspended(answers, outcome.ok.unindexed);
    this.render();
  }

  private lspDone(wanted: Wanted, outcome: { ok: LspWorkspaceSymbols } | { error: unknown }): void {
    if ("ok" in outcome)
      this.lspExpected = outcome.ok.servers.length + outcome.ok.failed.length > 0;
    else this.lspExpected = false;
    const answers = this.current;
    if (answers?.wanted !== wanted) return;
    if ("ok" in outcome) answers.lsp = outcome.ok;
    answers.lspDone = true;
    this.render();
  }

  private async checkSuspended(answers: Answers, unindexed: readonly string[]) {
    let status: IndexStatus;
    try {
      status = await this.deps.status(answers.wanted.scope);
    } catch {
      return;
    }
    const suspended = status.roots.some(
      (r) => r.state === "suspended" && unindexed.includes(r.root),
    );
    if (!suspended || this.current !== answers) return;
    answers.suspended = true;
    this.render();
  }

  /** 지금 질의의 답으로 보일 것을 정한다(머리 주석 「200 ms 규칙」). */
  private render() {
    const a = this.current;
    if (!a) return;
    const indexIn = a.index !== undefined;
    const waitLsp = !a.lspDone && !a.graceOver && this.lspExpected;
    // 서버 결과는 인덱스 답 전에도 그린다(앞에 서므로 뒤에 인덱스가 붙어도 목록이 뒤바뀌지 않는다). 빈 서버 답만으로는 그리지
    // 않는다 — 지난 결과 · 「바쁨」 표시를 지우지 않게.
    const lspRows = (a.lsp?.symbols.length ?? 0) > 0;
    if (!(indexIn && !waitLsp) && !lspRows) {
      // 아직 그릴 것이 없다 — 지난 결과를 두고 상태만.
      this.set({
        lspPending: !a.lspDone,
        suspended: a.suspended,
        ...(a.indexError !== undefined && { error: a.indexError, busy: false, pending: false }),
      });
      return;
    }
    const merged = mergeSymbols(a.lsp, a.index, a.wanted.text, a.wanted.types);
    this.set({
      query: a.wanted.text,
      types: a.wanted.types,
      scope: a.wanted.scope,
      rows: merged.rows,
      truncated:
        merged.truncated ||
        (a.lsp?.truncated ?? false) ||
        (a.index !== undefined && a.index.matched > a.index.symbols.length),
      pending: !indexIn && a.indexError === undefined,
      lspPending: !a.lspDone,
      // 인덱스 답이 아직이면 「바쁨」 · 실패 문장을 그대로 둔다.
      busy: indexIn ? false : this.state.busy,
      error: indexIn ? undefined : (a.indexError ?? this.state.error),
      suspended: a.suspended,
    });
  }

  private set(change: Partial<SymbolSearchState>) {
    if (this.disposed) return;
    this.state = { ...this.state, ...change };
    for (const l of this.listeners) l();
  }
}

function clip(text: string): string {
  return text.length <= NAME_LIMIT ? text : text.slice(0, NAME_LIMIT);
}

/** 이름 안에서 질의 글자를 차례로 찾은 위치(대소문자 무시, 공백은 건너뜀). 차례로 들지 않으면 빈 목록. 서버 결과의 칠하기에만. */
export function matchPositions(name: string, query: string): number[] {
  const wanted = query.replace(/\s+/g, "").toLowerCase();
  const lower = name.toLowerCase();
  const out: number[] = [];
  let from = 0;
  for (const ch of wanted) {
    const at = lower.indexOf(ch, from);
    if (at < 0) return [];
    out.push(at);
    from = at + ch.length;
  }
  return out;
}

function keyOf(file: string, line: number, name: string): string {
  return `${file}\n${line}\n${name}`;
}

/**
 * 두 출처를 합친다: 서버 결과(서버의 정의 순서 · 서버가 준 순서)를 앞에, 그 뒤 인덱스 결과(점수순). 같은 (파일, 줄, 이름)은 하나 —
 * 앞의 것(서버)을 두고 열만 인덱스의 이름 자리로 고친다. `types` 면 타입 종류만 남긴다(출처가 거르지 않았어도). 결과 `SYMBOL_LIMIT` 개, 이름 · 컨테이너 `NAME_LIMIT`.
 */
export function mergeSymbols(
  lsp: LspWorkspaceSymbols | undefined,
  index: IndexSymbolSearch | undefined,
  query: string,
  types: boolean,
  limit = SYMBOL_LIMIT,
): { rows: SymbolRow[]; truncated: boolean } {
  const rows: SymbolRow[] = [];
  /** 열쇠 → `rows` 의 자리. */
  const seen = new Map<string, number>();
  let truncated = false;
  const push = (row: SymbolRow) => {
    const at = seen.get(row.key);
    if (at !== undefined) {
      // 같은 심볼: 앞의 것(서버)을 두되, 인덱스가 아는 이름 자리의 열을 쓴다 — 서버에 따라 선언 줄의 처음(typescript-language-
      // server 는 1열)을 주므로, 고르면 이름에 커서가 가게.
      const kept = rows[at]!;
      if (row.source === "index" && kept.source === "lsp" && kept.column !== row.column)
        rows[at] = { ...kept, column: row.column };
      return;
    }
    if (types && !TYPE_KINDS.includes(row.kind)) return;
    if (rows.length >= limit) {
      truncated = true;
      return;
    }
    seen.set(row.key, rows.length);
    rows.push(row);
  };
  for (const s of lsp?.symbols ?? []) {
    const name = clip(s.name);
    const file = s.relative ?? s.location.path;
    push({
      key: keyOf(file, s.location.line, name),
      name,
      kind: kindOfLsp(s.kind),
      ...(s.container && { container: clip(s.container) }),
      path: s.location.path,
      ...(s.relative !== null && { relative: s.relative }),
      line: s.location.line,
      column: s.location.column,
      ...(s.location.token && { token: s.location.token }),
      server: s.server,
      language: s.language,
      positions: matchPositions(name, query),
      source: "lsp",
    });
  }
  for (const m of index?.symbols ?? []) {
    const name = clip(m.name);
    const line = m.selectionRange.start.line + 1;
    push({
      key: keyOf(m.relative, line, name),
      name,
      kind: m.kind,
      ...(m.container && { container: clip(m.container) }),
      path: m.path,
      relative: m.relative,
      line,
      column: m.selectionRange.start.character + 1,
      positions: m.positions.filter((p) => p < name.length),
      source: "index",
    });
  }
  return { rows, truncated };
}
