// Redis 키 트리 측정 하네스의 빌드 설정(redis-keys-bench.tsx 머리 주석). 앱의 설정에 이 폴더의 index.html 만 만들고, 키 트리가 쓰는
// 창의 키맵(팝업 상태 알림)을 하네스의 빈 자리로 바꾼다 — 하네스는 키맵 공급자 없이 트리만 띄운다.

import { defineConfig, mergeConfig } from "vite";

import app from "../../vite.config";

export default defineConfig((env) =>
  mergeConfig(typeof app === "function" ? app(env) : app, {
    build: { rollupOptions: { input: new URL("./index.html", import.meta.url).pathname } },
    resolve: {
      alias: [
        {
          find: /^.*\/keymap\/keymap-provider$/,
          replacement: new URL("./keymap-stub.ts", import.meta.url).pathname,
        },
      ],
    },
  }),
);
