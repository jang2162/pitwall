// 웹뷰 로그를 코어의 UI 로그 파일로 보낸다. 로거 본체와 묶음 정책은 @pitwall/logger.

import { captureGlobalErrors, UiLogger } from "@pitwall/logger";
import { RpcClient } from "@pitwall/rpc-client";

import { connectHost } from "./tauri-transport";

// src-tauri/src/main.rs 의 LOCAL_HOST_ID. UI 로그는 프로젝트가 원격 Host 에 묶여 있어도 이 기기에 남긴다.
const LOCAL_HOST = { hostId: "local" };

/** 로거를 만들고 전역 오류 수집을 건다. 연결은 첫 묶음을 보낼 때 연다. */
export function installUiLog(target: Window): UiLogger {
  let client: Promise<RpcClient> | undefined;
  const connect = () =>
    (client ??= connectHost(LOCAL_HOST).then(
      (conn) => new RpcClient(conn.transport),
      (e: unknown) => {
        // 다음 묶음에서 다시 연다.
        client = undefined;
        throw e;
      },
    ));
  const logger = new UiLogger({
    send: async (batch) => (await connect()).call("log.write", batch),
    minLevel: import.meta.env.DEV ? "debug" : "info",
  });
  captureGlobalErrors(target, logger);
  return logger;
}
