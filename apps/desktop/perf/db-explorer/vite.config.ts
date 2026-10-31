// DB 탐색기 측정 하네스의 빌드 설정(db-explorer-bench.tsx 머리 주석). 앱의 설정 그대로 이 폴더의 index.html 만 만든다.

import { defineConfig, mergeConfig } from "vite";

import app from "../../vite.config";

export default defineConfig((env) =>
  mergeConfig(typeof app === "function" ? app(env) : app, {
    build: { rollupOptions: { input: new URL("./index.html", import.meta.url).pathname } },
  }),
);
