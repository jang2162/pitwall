// 터미널 탭 하나의 세션: Host 의 `terminal.create` 스트림 하나(crates/pw-run, 03-architecture.md 「RPC 설계」)와 화면의 xterm 하나를 잇는다.
//
// - 출력: 스트림 항목은 `started` → `output`(base64) … → `exited` 순서다. 바이트를 그대로 xterm 에 넘긴다 — 청크 경계가 UTF-8 문자
//   가운데여도 xterm 의 파서가 다음 청크와 이어 읽는다.
// - 흐름 제어: xterm 이 아직 그리지 않은 바이트가 `HIGH_WATER` 를 넘으면 `LOW_WATER` 아래로 내려갈 때까지 스트림에서 다음 항목을 꺼내지
//   않는다. 꺼내지 않으면 RPC 클라이언트가 credit 을 주지 않아 Host 가 PTY 읽기를 멈추고 셸이 쓰기에서 기다린다 — 빠른 출력
//   (`seq 1 200000`, `cat` 큰 파일)이 웹뷰의 메모리를 끝없이 늘리거나 한 번에 몰려 화면을 멈추지 않게.
// - 입력: `terminal.input` 은 unary 라 연달아 보내면 Host 에서 순서가 바뀔 수 있다. 하나가 가는 동안 온 입력은 모아 두었다가 그것이
//   끝나면 한 번에 보낸다 — 순서를 지키고 빠른 타이핑 · 붙여넣기는 묶인다. 셸이 뜨기 전(`started` 전)의 입력도 같이 모인다.
// - 크기: 바뀔 때마다 보내지 않고 마지막 크기만 보낸다(가는 중이면 끝난 뒤에).
// - 끝내기: `dispose` 가 스트림을 취소하면 Host 가 셸의 프로세스 트리를 끝낸다. 창이 닫혀 연결이 끊겨도 같다(M1-08).

import type { RpcClient, TerminalEvent } from "@pitwall/rpc-client";

/** 세션이 쓰는 xterm 의 부분. 시험은 가짜를 넣는다. */
export interface TerminalScreen {
  write(data: Uint8Array | string, callback?: () => void): void;
}

export type SessionState =
  | { kind: "starting" }
  | {
      kind: "running";
      terminalId: number;
      pid: number | null;
      cwd: string;
      /** Host 가 띄운 셸의 argv(드롭한 경로의 따옴표 규칙을 고른다). */
      shell: readonly string[];
    }
  | { kind: "exited"; code: number | null }
  | { kind: "failed"; message: string };

export interface SessionOptions {
  client: Pick<RpcClient, "call" | "stream">;
  projectId: string;
  /** 창의 활성 워크트리 꼭대기. Host 가 그 안의 프로젝트 루트에서 셸을 띄운다. */
  worktree?: string;
  cols: number;
  rows: number;
  screen: TerminalScreen;
  onState(state: SessionState): void;
  /** 끝났거나 실패했을 때 화면에 덧붙일 한 줄(UI 언어). */
  describeEnd(state: Extract<SessionState, { kind: "exited" | "failed" }>): string;
}

/** 그리지 않은 바이트가 이만큼 쌓이면 스트림에서 꺼내기를 멈춘다. */
export const HIGH_WATER = 512 * 1024;
/** 멈춘 뒤 이만큼 아래로 내려가면 다시 꺼낸다. */
export const LOW_WATER = 128 * 1024;

export function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export class TerminalSession {
  private state: SessionState = { kind: "starting" };
  private readonly controller = new AbortController();
  private pendingInput = "";
  private sendingInput = false;
  private wantedSize: { cols: number; rows: number } | undefined;
  private sendingSize = false;
  private unrendered = 0;
  private drained: (() => void) | undefined;
  /** 스트림을 다 읽으면 풀린다(시험 · 정리용). */
  readonly finished: Promise<void>;

  constructor(private readonly options: SessionOptions) {
    this.finished = this.run();
  }

  get current(): SessionState {
    return this.state;
  }

  /** 셸에 보낼 글자(키 입력 · 붙여넣기). 끝난 세션이면 버린다. */
  input(data: string): void {
    if (this.state.kind === "exited" || this.state.kind === "failed") return;
    this.pendingInput += data;
    this.flushInput();
  }

  resize(cols: number, rows: number): void {
    if (cols <= 0 || rows <= 0) return;
    this.wantedSize = { cols, rows };
    this.flushSize();
  }

  /** 스트림을 취소한다 — Host 가 셸과 그 프로세스를 끝낸다. */
  dispose(): void {
    this.controller.abort();
    this.drained?.();
  }

  private setState(state: SessionState) {
    this.state = state;
    this.options.onState(state);
  }

  private async run(): Promise<void> {
    const { client, projectId, worktree, cols, rows } = this.options;
    try {
      const stream = client.stream(
        "terminal.create",
        { projectId, worktree: worktree ?? null, cols, rows },
        { signal: this.controller.signal },
      );
      for await (const event of stream) {
        this.handle(event);
        if (this.unrendered > HIGH_WATER) await this.drain();
        if (this.controller.signal.aborted) return;
      }
      // 취소하지 않았는데 `exited` 없이 끝난 스트림(연결이 닫힘 등).
      if (!this.controller.signal.aborted && this.state.kind !== "exited") {
        this.end({ kind: "exited", code: null });
      }
    } catch (e) {
      if (this.controller.signal.aborted) return;
      this.end({ kind: "failed", message: e instanceof Error ? e.message : String(e) });
    }
  }

  private handle(event: TerminalEvent) {
    switch (event.kind) {
      case "started":
        this.setState({
          kind: "running",
          terminalId: event.terminalId,
          pid: event.pid,
          cwd: event.cwd,
          shell: event.shell,
        });
        this.flushInput();
        this.flushSize();
        return;
      case "output": {
        const bytes = decodeBase64(event.data);
        this.unrendered += bytes.length;
        this.options.screen.write(bytes, () => {
          this.unrendered -= bytes.length;
          if (this.unrendered < LOW_WATER) this.drained?.();
        });
        return;
      }
      case "exited":
        this.end({ kind: "exited", code: event.code });
    }
  }

  private drain(): Promise<void> {
    return new Promise((resolve) => {
      this.drained = () => {
        this.drained = undefined;
        resolve();
      };
      if (this.unrendered < LOW_WATER) this.drained();
    });
  }

  private end(state: Extract<SessionState, { kind: "exited" | "failed" }>) {
    this.pendingInput = "";
    this.options.screen.write(`\r\n\x1b[2m${this.options.describeEnd(state)}\x1b[0m\r\n`);
    this.setState(state);
  }

  private flushInput() {
    const { state } = this;
    if (this.sendingInput || state.kind !== "running" || this.pendingInput === "") return;
    const data = this.pendingInput;
    this.pendingInput = "";
    this.sendingInput = true;
    this.options.client
      .call("terminal.input", { terminalId: state.terminalId, data })
      .catch((e: unknown) => console.warn("[terminal] input not sent", e))
      .finally(() => {
        this.sendingInput = false;
        this.flushInput();
      });
  }

  private flushSize() {
    const { state, wantedSize } = this;
    if (this.sendingSize || state.kind !== "running" || !wantedSize) return;
    this.wantedSize = undefined;
    this.sendingSize = true;
    this.options.client
      .call("terminal.resize", { terminalId: state.terminalId, ...wantedSize })
      .catch((e: unknown) => console.warn("[terminal] resize not sent", e))
      .finally(() => {
        this.sendingSize = false;
        this.flushSize();
      });
  }
}
