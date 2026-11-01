// tools-registry-bump.mjs 의 리다이렉트 검사(리뷰 F6): 홉마다 https · 횟수 상한. 네트워크 없이 가짜 fetch 로 돈다.
import assert from "node:assert/strict";
import { test } from "node:test";

import { MAX_REDIRECTS, followHttps } from "./tools-registry-bump.mjs";

/** 주소 → 답(`{ status, location }`)의 가짜 fetch. 받은 주소를 남긴다. */
function fakeFetch(routes) {
  const seen = [];
  const impl = async (url, init) => {
    assert.equal(init.redirect, "manual");
    seen.push(url);
    const r = routes[url] ?? { status: 404 };
    return {
      status: r.status,
      ok: r.status >= 200 && r.status < 300,
      headers: new Headers(r.location ? { location: r.location } : {}),
    };
  };
  return { impl, seen };
}

test("follows https redirects, including relative ones", async () => {
  const { impl, seen } = fakeFetch({
    "https://a.example/x": { status: 302, location: "https://b.example/y" },
    "https://b.example/y": { status: 301, location: "/z" },
    "https://b.example/z": { status: 200 },
  });
  const res = await followHttps("https://a.example/x", {}, impl);
  assert.equal(res.status, 200);
  assert.deepEqual(seen, ["https://a.example/x", "https://b.example/y", "https://b.example/z"]);
});

test("refuses an http hop in the middle even if it leads back to https", async () => {
  const { impl, seen } = fakeFetch({
    "https://a.example/x": { status: 302, location: "http://mitm.example/y" },
    "http://mitm.example/y": { status: 302, location: "https://b.example/z" },
    "https://b.example/z": { status: 200 },
  });
  await assert.rejects(followHttps("https://a.example/x", {}, impl), /non-https/);
  assert.deepEqual(seen, ["https://a.example/x"], "nothing is sent over http");
});

test("refuses a non-https start and too many redirects", async () => {
  const { impl } = fakeFetch({});
  await assert.rejects(followHttps("http://a.example/x", {}, impl), /non-https/);
  const loop = fakeFetch({ "https://a.example/loop": { status: 302, location: "/loop" } });
  await assert.rejects(followHttps("https://a.example/loop", {}, loop.impl), /redirects/);
  assert.equal(loop.seen.length, MAX_REDIRECTS + 1);
});
