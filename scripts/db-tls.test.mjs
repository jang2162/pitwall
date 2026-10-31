// scripts/db-tls.mjs 의 인자 읽기를 도커 · openssl 없이 본다.
// 실행: node --test scripts/db-tls.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { resolve } from "node:path";
import { test } from "node:test";

import { CA_ENV, TARGETS, parseArgs, publish } from "./db-tls.mjs";

test("no db means every target, on its default port and name", () => {
  const options = parseArgs([]);
  assert.deepEqual(options.dbs, Object.keys(TARGETS));
  assert.deepEqual(options.ports, {});
  assert.equal(options.name, "pitwall-tls");
  assert.match(options.out, /target[\\/]db-tls$/);
});

test("a worktree picks its own name, port and folder", () => {
  const options = parseArgs([
    "--name",
    "pw-m306-tls",
    "--port",
    "pg=55437",
    "--out",
    "x/certs",
    "pg",
  ]);
  assert.deepEqual(options.dbs, ["pg"]);
  assert.deepEqual(options.ports, { pg: 55437 });
  assert.equal(options.name, "pw-m306-tls");
  assert.equal(options.out, resolve("x/certs"));
});

test("unknown dbs and bad ports are refused", () => {
  assert.throws(() => parseArgs(["oracle"]), /unknown argument/);
  assert.throws(() => parseArgs(["--port", "pg=abc"]), /--port expects/);
  assert.throws(() => parseArgs(["--port", "mssql=1433"]), /--port expects/);
});

test("the pg target hands the tests a localhost URL, and the CA is shared", () => {
  assert.equal(CA_ENV, "PW_TEST_TLS_CA");
  assert.equal(TARGETS.pg.urlEnv, "PW_TEST_PG_TLS_URL");
  assert.equal(TARGETS.pg.url(5443), "postgres://pitwall:pitwall@localhost:5443/pitwall");
  const args = TARGETS.pg.args({ out: "/certs" });
  assert.ok(args.includes("/certs:/pw-tls:ro"), args.join(" "));
  assert.ok(args.at(-1).includes("ssl=on"));
});

test("the redis target is tls-only on a pinned image and shares the CA", () => {
  assert.equal(TARGETS.redis.urlEnv, "PW_TEST_REDIS_TLS_URL");
  assert.equal(TARGETS.redis.url(6380), "rediss://localhost:6380/0");
  const args = TARGETS.redis.args({ out: "/certs" });
  assert.ok(args.includes("/certs:/pw-tls:ro"), args.join(" "));
  assert.ok(
    args.some((a) => /^redis:[\d.]+@sha256:[0-9a-f]{64}$/.test(a)),
    args.join(" "),
  );
  const command = args.at(-1);
  assert.match(command, /--port 0 --tls-port 6380 /);
  assert.match(command, /--tls-ca-cert-file \/tls\/ca\.pem/);
  assert.equal(TARGETS.redis.innerPort, 6380);
});

test("port 0 lets docker pick a free port", () => {
  assert.equal(publish(0, 5432), "127.0.0.1::5432");
  assert.equal(publish(5443, 5432), "127.0.0.1:5443:5432");
});
