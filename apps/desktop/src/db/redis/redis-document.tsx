// Redis 키 문서(14-m3-plan M3-18). 문서 틀(src/db/documents)만 먼저 들어온 자리 — 화면은 M3-18 의 다음 커밋이 채운다.

import type { DbDocumentProps } from "../documents/db-document-panel";

export function RedisDocument({ spec, panelId }: DbDocumentProps<"redis">) {
  return (
    <div data-redis-document={panelId} data-keymap-context="redisDocument" className="h-full p-2">
      {spec.connectionId} · DB {spec.database}
    </div>
  );
}
