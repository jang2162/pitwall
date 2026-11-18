// 실행 하나의 출력 보기: Host 의 `run.attach { runId, since }` 스트림 하나(17-m6-plan M6-02 ⑤, 03-architecture.md 「4. RPC 설계」)를
// 콘솔의 xterm 에 잇는다. 실행은 연결이 아니라 Host 의 것이라(메인 판정 M6-P1) 이 스트림을 끊어도 실행은 돈다 — 창을 다시
// 불러오면 `since: 0` 으로 남은 출력을 처음부터 다시 받는다(M6-06 ⑤).
//
// - 항목: `attached { run, from, dropped }` → `output { offset, data }`(base64) … → `exited { exit }`. `dropped` 나 오프셋의 빈틈은
//   버퍼에서 버려진 앞이다 — 화면에 그렇다고 한 줄 적는다(조용히 잇지 않는다).
// - 흐름 제어 · 입력 묶기 · 크기는 터미널 세션(terminal-session.ts)과 같은 규칙이다: 그리지 않은 바이트가 `HIGH_WATER` 를 넘으면
//   `LOW_WATER` 아래로 내려갈 때까지 스트림에서 꺼내지 않는다(Host 가 credit 을 기다린다), `run.input` 은 하나가 가는 동안 온 것을
//   모아 순서대로, 크기는 마지막 것만.

import type { RpcClient, RunAttachEvent, RunExit, RunInfo } from "@pitwall/rpc-client";

import {
  decodeBase64,
  HIGH_WATER,
  LOW_WATER,
  type TerminalScreen,
} from "../terminal/terminal-session";

export interface RunStreamOptions {
  client: Pick<RpcClient, "call" | "stream">;
  runId: string;
  /** 이 오프셋부터 받는다(없으면 0 — 남은 처음부터). */
  since?: number;
  screen: TerminalScreen;
  /** 첫 항목. */
  onAttached?(run: RunInfo): void;
  /** 화면에 쓴 출력이 그려졌다(로그 레벨 강조 · 스크롤 고정). */
  onRendered?(): void;
  onExited?(exit: RunExit): void;
  /** 스트림이 오류로 끝났다(없는 실행 · 연결이 끊김). */
  onFailed?(message: string): void;
  /** 버려진 앞의 바이트 수를 알리는 줄(UI 언어). */
  droppedLine(bytes: number): string;
}

export class RunStream {
  private readonly controller = new AbortController();
  private pendingInput = "";
  private sendingInput = false;
  private wantedSize: { cols: number; rows: number } | undefined;
  private sendingSize = false;
  private unrendered = 0;
  private drained: (() => void) | undefined;
  private ended = false;
  /** 다음에 올 출력의 오프셋(받은 끝). */
  private offset: number;
  readonly finished: Promise<void>;

  constructor(private readonly options: RunStreamOptions) {
    this.offset = options.since ?? 0;
    this.finished = this.run();
  }

  get end(): number {
    return this.offset;
  }

  /** 실행에 보낼 글자(키 입력 · 붙여넣기 · xterm 의 보고 응답). 끝난 실행이면 버린다. */
  input(data: string): void {
    if (this.ended) return;
    this.pendingInput += data;
    this.flushInput();
  }

  resize(cols: number, rows: number): void {
    if (this.ended || cols <= 0 || rows <= 0) return;
    this.wantedSize = { cols, rows };
    this.flushSize();
  }

  /** 스트림만 끊는다 — 실행은 Host 에서 계속 돈다. */
  dispose(): void {
    this.ended = true;
    this.controller.abort();
    this.drained?.();
  }

  private async run(): Promise<void> {
    const { client, runId } = this.options;
    try {
      const stream = client.stream(
        "run.attach",
        { runId, since: this.offset },
        { signal: this.controller.signal },
      );
      for await (const event of stream) {
        this.handle(event);
        if (this.unrendered > HIGH_WATER) await this.drain();
        if (this.controller.signal.aborted) return;
      }
    } catch (e) {
      if (this.controller.signal.aborted) return;
      this.ended = true;
      this.options.onFailed?.(e instanceof Error ? e.message : String(e));
    }
  }

  private handle(event: RunAttachEvent) {
    switch (event.kind) {
      case "attached":
        if (event.dropped > 0) this.note(this.options.droppedLine(event.dropped));
        this.offset = event.from;
        this.options.onAttached?.(event.run);
        return;
      case "output": {
        if (event.offset > this.offset)
          this.note(this.options.droppedLine(event.offset - this.offset));
        const bytes = decodeBase64(event.data);
        this.offset = event.offset + bytes.length;
        this.unrendered += bytes.length;
        this.options.screen.write(bytes, () => {
          this.unrendered -= bytes.length;
          this.options.onRendered?.();
          if (this.unrendered < LOW_WATER) this.drained?.();
        });
        return;
      }
      case "exited":
        this.ended = true;
        this.pendingInput = "";
        // 남은 출력이 다 그려진 뒤에 알린다.
        this.options.screen.write("", () => this.options.onExited?.(event.exit));
    }
  }

  private note(text: string) {
    this.options.screen.write(`\x1b[2m${text}\x1b[0m\r\n`);
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

  private flushInput() {
    if (this.sendingInput || this.ended || this.pendingInput === "") return;
    const data = this.pendingInput;
    this.pendingInput = "";
    this.sendingInput = true;
    this.options.client
      .call("run.input", { runId: this.options.runId, data })
      .catch((e: unknown) => console.warn("[run] input not sent", e))
      .finally(() => {
        this.sendingInput = false;
        this.flushInput();
      });
  }

  private flushSize() {
    const { wantedSize } = this;
    if (this.sendingSize || this.ended || !wantedSize) return;
    this.wantedSize = undefined;
    this.sendingSize = true;
    this.options.client
      .call("run.resize", { runId: this.options.runId, ...wantedSize })
      .catch((e: unknown) => console.warn("[run] resize not sent", e))
      .finally(() => {
        this.sendingSize = false;
        this.flushSize();
      });
  }
}
