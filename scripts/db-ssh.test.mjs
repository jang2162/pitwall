// scripts/db-ssh.mjs 의 인자 읽기를 도커 · ssh-keygen 없이 본다.
// 실행: node --test scripts/db-ssh.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { resolve } from "node:path";
import { test } from "node:test";

import { DIR_ENV, URL_ENV, parseArgs } from "./db-ssh.mjs";

test("defaults: a free port, the default name and folder", () => {
  const options = parseArgs([]);
  assert.equal(options.port, 0);
  assert.equal(options.name, "pitwall-ssh");
  assert.match(options.out, /target[\\/]db-ssh$/);
});

test("a worktree picks its own name, port and folder", () => {
  const options = parseArgs(["--name", "pw-m311-ssh", "--port", "2222", "--out", "x/keys"]);
  assert.deepEqual(options, { name: "pw-m311-ssh", port: 2222, out: resolve("x/keys") });
});

test("bad ports and unknown arguments are refused", () => {
  assert.throws(() => parseArgs(["--port", "abc"]), /--port expects/);
  assert.throws(() => parseArgs(["pg"]), /unknown argument/);
});

test("the tests read the url and the key folder from these names", () => {
  assert.equal(URL_ENV, "PW_TEST_SSH_URL");
  assert.equal(DIR_ENV, "PW_TEST_SSH_DIR");
});
