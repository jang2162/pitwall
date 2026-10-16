// 기본 테마를 확장 레지스트리(crates/pw-ext 의 `theme.get`)에서 받아 테마 스토어에 넣는다. 첫 그림을 번들에 묶은 같은
// 파일로 칠하고 나중에 바꾸는 이유는 docs/design/07-extensibility.md 「3. 테마」 「기본 테마」 행.

import type { RpcClient } from "@pitwall/rpc-client";

import type { ThemeStore } from "./theme-store";
import { parseTheme, type Theme } from "./tokens";

/** 기본 테마의 id. extensions/theme-default/extension.toml 의 `[contributes.themes.<id>]`. */
export const DEFAULT_THEME_ID = "pitwall-default";

export interface RegistryTheme {
  theme: Theme;
  /** 기여한 확장의 id. */
  extension: string;
}

export async function fetchRegistryTheme(
  client: Pick<RpcClient, "call">,
  id: string,
): Promise<RegistryTheme> {
  const file = await client.call("theme.get", { id });
  const theme = parseTheme(file.content);
  if (theme.id !== id) throw new Error(`theme.get(${id}) 이 다른 테마(${theme.id})를 줬다`);
  return { theme, extension: file.extension };
}

interface Log {
  info(message: string, source?: string): void;
  warn(message: string, source?: string): void;
}

/** 레지스트리의 테마로 바꾼다. 실패하면 경고만 남기고 지금 테마를 둔다. */
export async function applyRegistryTheme(
  store: ThemeStore,
  connect: () => Promise<{ client: Pick<RpcClient, "call">; close(): void }>,
  log: Log,
  id = DEFAULT_THEME_ID,
): Promise<void> {
  let conn: Awaited<ReturnType<typeof connect>> | undefined;
  try {
    conn = await connect();
    const { theme, extension } = await fetchRegistryTheme(conn.client, id);
    store.getState().setTheme(theme);
    log.info(`theme ${theme.id} applied from extension ${extension}`, "theme");
  } catch (error) {
    log.warn(`theme ${id} not loaded from the extension registry: ${String(error)}`, "theme");
  } finally {
    conn?.close();
  }
}
