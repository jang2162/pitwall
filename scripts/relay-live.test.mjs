// scripts/relay-live.mjs 의 판정(하한 · 인자 · 컨테이너)을 도커 없이 본다.
// 실행: node --test scripts/relay-live.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";

import { FILTER, containerName, entry, parseArgs, relayFloor } from "./relay-live.mjs";

const floorText = readFileSync(resolve(import.meta.dirname, "relay-live.toml"), "utf8");
const liveRs = readFileSync(resolve(import.meta.dirname, "../apps/relay/tests/live.rs"), "utf8");

test("the floor is at least the scenarios M7-05 wrote and the file has them", () => {
  const floor = relayFloor(floorText);
  assert.ok(floor >= 5, `floor ${floor}`);
  const tests = [...liveRs.matchAll(/#\[test\]\s*\n\s*fn (live_relay_\w+)/g)].map((m) => m[1]);
  assert.ok(tests.includes(`${FILTER}_ready`));
  const counted = tests.filter((n) => n !== `${FILTER}_ready`);
  assert.ok(counted.length >= floor, `${counted.length} scenarios < floor ${floor}`);
  // 모든 시험이 필터에 걸리는 이름이다.
  for (const name of tests) assert.ok(name.startsWith(`${FILTER}_`), name);
  assert.throws(() => relayFloor("relay = 1\nother = 2"), /other than/);
  assert.throws(() => relayFloor("other = 2"), /no `relay/);
});

test("the live tests never turn off host key checking", () => {
  assert.doesNotMatch(liveRs, /StrictHostKeyChecking=(no|accept-new)/);
  assert.doesNotMatch(liveRs, /UserKnownHostsFile=\/dev\/null/);
  assert.match(liveRs, /StrictHostKeyChecking=yes/);
});

test("arguments, container name and entry", () => {
  assert.throws(() => parseArgs([]), /--bin/);
  const options = parseArgs(["--ci", "--bin", "/x/relay"]);
  // `--bin` 은 resolve 로 절대 경로가 된다 — Windows 에선 드라이브가 붙는다(relay-build 시험과 같은 꼴, CI run 45529596394).
  assert.deepEqual(options, { ci: true, bin: resolve("/x/relay") });
  assert.throws(() => parseArgs(["--bin", "x", "--what"]), /unknown argument/);
  const a = containerName("/w/a", 1);
  assert.match(a, /^pitwall-relay-live-[0-9a-f]{8}-1$/);
  assert.notEqual(a, containerName("/w/b", 1));
  assert.match(entry(), /apk add --no-cache openssh-server git/);
  assert.match(entry(), /PasswordAuthentication=no/);
});
