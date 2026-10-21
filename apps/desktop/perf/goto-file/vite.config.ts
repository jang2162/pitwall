// 파일로 이동 측정 하네스의 빌드 설정(goto-file-bench.tsx 머리 주석). 앱의 설정에 하나만 더한다: 앱의 Host 연결
// (src/tauri-transport.ts)을 메모리 속 가짜 Host(fake-transport.ts)로 바꿔 끼운다.

import { fileURLToPath } from "node:url";

import { defineConfig, mergeConfig, type Plugin } from "vite";

import app from "../../vite.config";

const fake = fileURLToPath(new URL("./fake-transport.ts", import.meta.url));

function fakeTransport(): Plugin {
  return {
    name: "pitwall-fake-transport",
    enforce: "pre",
    async resolveId(source, importer, options) {
      if (!source.endsWith("tauri-transport")) return null;
      const resolved = await this.resolve(source, importer, { ...options, skipSelf: true });
      return resolved?.id.endsWith("/src/tauri-transport.ts") ? fake : null;
    },
  };
}

export default defineConfig((env) =>
  mergeConfig(typeof app === "function" ? app(env) : app, { plugins: [fakeTransport()] }),
);
