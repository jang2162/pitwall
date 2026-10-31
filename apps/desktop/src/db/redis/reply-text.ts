// 명령 응답(`KvReply`)을 콘솔의 글 줄로 — redis-cli 와 같은 꼴: 배열은 `1) …` 로 번호를 달고 안쪽은 들여 쓴다, 맵은 `1# 키 => 값`,
// 글 값은 따옴표 안에(제어 문자는 이스케이프), 바이트는 `0x…`. 콘솔 응답 상한에서 끊긴 응답(`truncated`)은 끝에 그렇게 적는다.
// 값은 신뢰할 수 없는 글이라 화면은 글자 노드로만 그린다(console.tsx).

import type { DbValue, KvReply } from "@pitwall/rpc-client";

import { formatSize, valueText } from "../grid/values";
import type { DbRedisStrings } from "./strings";

/** 응답 하나에서 만드는 줄의 상한. 넘으면 「…」 한 줄로 끝낸다(응답 자체는 Host 가 원소 1만 · 4 MiB 로 묶었다). */
export const MAX_REPLY_LINES = 20_000;

/** 벌크 값 하나의 글. */
function bulkText(v: DbValue): string {
  if (v !== "null" && v.text !== undefined) return JSON.stringify(v.text);
  if (v !== "null" && v.bytes !== undefined) return v.bytes.hex;
  if (v !== "null" && v.truncated !== undefined)
    return `${JSON.stringify(v.truncated.prefix)}… (${formatSize(v.truncated.size)})`;
  return valueText(v) ?? "(nil)";
}

export function replyLines(reply: KvReply, T: DbRedisStrings["console"]["reply"]): string[] {
  const out: string[] = [];
  let full = false;
  const push = (line: string) => {
    if (out.length >= MAX_REPLY_LINES) {
      full = true;
      return;
    }
    out.push(line);
  };
  // 한 값의 줄들 — 첫 줄은 `head` 뒤에 붙이고 나머지는 `pad` 만큼 들인다.
  const render = (r: KvReply, head: string, pad: string) => {
    if (full) return;
    if (r === "nil") return push(head + T.nil);
    if (r.status !== undefined) return push(head + r.status);
    if (r.error !== undefined) return push(head + T.error(r.error));
    if (r.int !== undefined) return push(head + T.integer(valueText(r.int) ?? ""));
    if (r.double !== undefined) return push(head + T.double(r.double));
    if (r.bool !== undefined) return push(`${head}(${r.bool ? "true" : "false"})`);
    if (r.bulk !== undefined) return push(head + bulkText(r.bulk));
    if (r.array !== undefined) {
      if (r.array.length === 0) return push(head + T.empty);
      const width = String(r.array.length).length;
      r.array.forEach((item, i) => {
        const marker = `${String(i + 1).padStart(width)}) `;
        render(item, i === 0 ? head + marker : pad + marker, pad + " ".repeat(marker.length));
      });
      return;
    }
    if (r.map !== undefined) {
      if (r.map.length === 0) return push(head + T.empty);
      const width = String(r.map.length).length;
      r.map.forEach((pair, i) => {
        const marker = `${String(i + 1).padStart(width)}# `;
        // 맵의 키는 거의 늘 글 하나다 — 여러 줄이면 한 줄로 잇는다.
        const saved = out.length;
        render(pair.key, "", "");
        const keyLine = out.splice(saved).join(" ");
        const arrow = `${marker}${keyLine} => `;
        // 값의 둘째 줄부터는 값이 시작한 자리에 맞춘다.
        render(pair.value, (i === 0 ? head : pad) + arrow, pad + " ".repeat(arrow.length));
      });
      return;
    }
    if (r.truncated !== undefined) {
      render(r.truncated.reply, head, pad);
      return push(pad + T.truncated(r.truncated.elements, formatSize(r.truncated.bytes)));
    }
  };
  render(reply, "", "");
  if (full) out.push("…");
  return out;
}
