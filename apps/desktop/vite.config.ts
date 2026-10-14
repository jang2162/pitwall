import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// 개발 서버 포트는 scripts/dev.mjs 가 워크트리마다 다르게 정해 넘긴다(docs/guideline/base/08-shared-resources.md).
// 기본값을 두지 않는다 — 고정 포트로 뜨면 다른 워크트리의 개발 서버와 부딪힌다.
// strictPort 로 둔다 — 포트가 차 있을 때 조용히 옆 포트로 옮겨 뜨면 Tauri 의 devUrl 과 어긋나
// 다른 워크트리의 서버를 자기 것인 줄 알고 보게 된다.
export default defineConfig(({ command, mode }) => {
  const port = Number(process.env.PITWALL_DEV_PORT);
  if (command === "serve" && mode !== "test" && !Number.isInteger(port)) {
    throw new Error("PITWALL_DEV_PORT 가 없다. 개발 서버는 `mise run dev` 로 띄운다.");
  }
  return {
    plugins: [react()],
    clearScreen: false,
    server: {
      host: "127.0.0.1",
      port,
      strictPort: true,
      watch: { ignored: ["**/src-tauri/**"] },
    },
  };
});
