// 웹뷰 로그를 모아 코어의 `log.write` 로 보낸다. 코어가 `$STATE/logs/ui.<날짜>.log` 에 쓴다(crates/pw-diag).
//
// 묶음 정책 — 로그가 쏟아져도 IPC 를 막지 않게 한다:
// - 항목을 대기열에 모아 `maxBatch` 개가 차거나 `flushIntervalMs` 가 지나면 한 번에 보낸다.
// - 보내는 중인 묶음은 하나뿐이다. 응답이 오기 전에 쌓인 것은 다음 묶음으로 간다.
// - 대기열이 `maxQueue` 를 넘으면 오래된 것부터 버리고, 버린 수를 다음 묶음의 `dropped` 로 알린다.
//   보내기가 실패한 묶음도 다시 보내지 않고 버린 수에 더한다 — 다시 보내다 같은 실패를 되풀이하지 않게.
// - 메시지는 `maxMessageLength` 글자에서 자른다. 코어도 따로 상한을 둔다(`pw_diag::methods`).
//
// 로그에 비밀(토큰 · 비밀번호)이나 사용자 데이터를 넣지 않는다 — 규칙은 crates/pw-diag/src/lib.rs 「로그에 비밀을 쓰지 않는다」 절.

import type { UiLogBatch, UiLogEntry, UiLogLevel } from "@pitwall/rpc-client";

export interface LoggerOptions {
  /** 묶음 하나를 코어로 보낸다. 실패하면 거부한다. */
  send: (batch: UiLogBatch) => Promise<unknown>;
  /** 묶음 하나의 최대 항목 수. 기본 50. 코어는 200 까지 받는다. */
  maxBatch?: number;
  /** 첫 항목이 쌓인 뒤 보내기까지 기다리는 시간. 기본 1000ms. */
  flushIntervalMs?: number;
  /** 보내지 못하고 쌓아 두는 최대 항목 수. 기본 500. */
  maxQueue?: number;
  /** 메시지 최대 글자 수. 기본 4000. */
  maxMessageLength?: number;
  /** 이보다 낮은 수준은 버린다. 기본 `info`. */
  minLevel?: UiLogLevel;
}

const LEVELS: Record<UiLogLevel, number> = { debug: 0, info: 1, warn: 2, error: 3 };

export class UiLogger {
  private readonly send: LoggerOptions["send"];
  private readonly maxBatch: number;
  private readonly flushIntervalMs: number;
  private readonly maxQueue: number;
  private readonly maxMessageLength: number;
  private readonly minLevel: number;
  private queue: UiLogEntry[] = [];
  private dropped = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private draining: Promise<void> | undefined;

  constructor(options: LoggerOptions) {
    this.send = options.send;
    this.maxBatch = options.maxBatch ?? 50;
    this.flushIntervalMs = options.flushIntervalMs ?? 1000;
    this.maxQueue = options.maxQueue ?? 500;
    this.maxMessageLength = options.maxMessageLength ?? 4000;
    this.minLevel = LEVELS[options.minLevel ?? "info"];
  }

  debug(message: string, source?: string): void {
    this.log("debug", message, source);
  }

  info(message: string, source?: string): void {
    this.log("info", message, source);
  }

  warn(message: string, source?: string): void {
    this.log("warn", message, source);
  }

  error(message: string, source?: string): void {
    this.log("error", message, source);
  }

  log(level: UiLogLevel, message: string, source?: string): void {
    if (LEVELS[level] < this.minLevel) return;
    const text =
      message.length > this.maxMessageLength
        ? `${message.slice(0, this.maxMessageLength)}…(truncated)`
        : message;
    this.queue.push({ level, message: text, source: source ?? null });
    if (this.queue.length > this.maxQueue) {
      const over = this.queue.length - this.maxQueue;
      this.queue.splice(0, over);
      this.dropped += over;
    }
    // 보내는 중이면 그 루프가 이어서 보낸다. 여기서 flush 를 부르면 항목마다 대기자가 하나씩 쌓인다.
    // 타이머는 루프가 끝나는 순간에 들어온 항목을 위한 안전망이다.
    if (this.queue.length >= this.maxBatch && !this.draining) {
      void this.flush();
    } else {
      this.timer ??= setTimeout(() => {
        this.timer = undefined;
        void this.flush();
      }, this.flushIntervalMs);
    }
  }

  /** 쌓인 것을 지금 보낸다. 보내는 중이면 그 루프의 Promise 를 돌려준다. 대기열이 비거나 보내기가 실패하면 풀린다. */
  flush(): Promise<void> {
    if (this.timer !== undefined) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    this.draining ??= this.drain().finally(() => {
      this.draining = undefined;
    });
    return this.draining;
  }

  private async drain(): Promise<void> {
    while (this.queue.length > 0 || this.dropped > 0) {
      const entries = this.queue.splice(0, this.maxBatch);
      const carried = this.dropped;
      this.dropped = 0;
      try {
        await this.send({ entries, dropped: carried });
      } catch {
        // 끊긴 연결에 같은 것을 되풀이해 보내지 않는다. 다음 묶음이 버린 수를 알린다.
        this.dropped += entries.length + carried;
        return;
      }
    }
  }
}

/**
 * 오류 값을 로그 한 줄로. Error 면 `이름: 메시지` 를 먼저 쓰고 스택을 붙인다.
 *
 * V8(WebView2 · Node)의 stack 은 `이름: 메시지` 로 시작하지만 JavaScriptCore(macOS · Linux 웹뷰)의 stack 은 프레임만
 * 있다. stack 만 쓰면 WebKit 에서 오류 메시지가 로그에서 빠진다.
 */
export function describeError(error: unknown): string {
  if (error instanceof Error) {
    const head = error.message ? `${error.name}: ${error.message}` : error.name;
    const stack = error.stack;
    if (!stack) return head;
    return stack.startsWith(head) ? stack : `${head}\n${stack}`;
  }
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error) ?? String(error);
  } catch {
    return String(error);
  }
}

/**
 * `error` 와 `unhandledrejection` 을 오류 로그로 남긴다. 기본 처리(콘솔 출력)는 막지 않는다. 떼는 함수를 돌려준다.
 */
export function captureGlobalErrors(
  target: Pick<EventTarget, "addEventListener" | "removeEventListener">,
  logger: Pick<UiLogger, "error">,
): () => void {
  const onError = (event: Event) => {
    const e = event as ErrorEvent;
    // 다른 출처 스크립트의 오류는 `error` 가 비어 있고 메시지만 온다.
    logger.error(e.error == null ? e.message : describeError(e.error), "window.error");
  };
  const onRejection = (event: Event) => {
    logger.error(describeError((event as PromiseRejectionEvent).reason), "unhandledrejection");
  };
  target.addEventListener("error", onError);
  target.addEventListener("unhandledrejection", onRejection);
  return () => {
    target.removeEventListener("error", onError);
    target.removeEventListener("unhandledrejection", onRejection);
  };
}
