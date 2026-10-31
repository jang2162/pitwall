// 실 DB 시험의 TLS 서버를 띄운다(14-m3-plan.md M3-06 「TLS 시험 서버」). 시험 CA 와 서버 인증서를 그 자리에서 만들고, 그 인증서를 단
// 컨테이너를 띄운 뒤 시험이 읽을 환경 변수를 알린다.
//
//   node scripts/db-tls.mjs [--out <폴더>] [--name <이름 앞부분>] [--port <db>=<포트> …] [db …]     db: pg · redis(없으면 전부)
//
// CI 의 `db-live` 잡은 checkout 뒤 이것을 부른다 — 서비스 컨테이너는 checkout 전에 떠 저장소의 파일을 마운트할 수 없다. 환경 변수는
// `GITHUB_ENV` 가 있으면 거기에 더하고(다음 단계가 받는다), 늘 표준 출력에 `이름=값` 줄로 낸다(로컬은 그대로 export 한다).
// `mise run test-db` 는 도커가 있고 TLS URL 이 없으면 이 파일의 `startTls` 로 워크트리별 이름 · 빈 포트에 띄우고 끝나면 끈다
// (`scripts/db-live.mjs`).
//
// `--out`(기본 `target/db-tls/`)은 도커가 마운트할 수 있는 폴더여야 한다 — macOS Docker Desktop 은 `/private/tmp` 아래 폴더를 비워
// 보여 서버가 키를 못 찾고 곧바로 꺼졌다(실측). 인증서는 실행마다 새로 만든다(유효 30일) — 저장소에 키를 두지 않고, 만료로 시험이
// 깨지지 않는다. 서버 인증서의 이름은 `localhost` 와 `127.0.0.1` 이다. CA 는 모든 DB 가 함께 쓴다(`PW_TEST_TLS_CA`).
// 컨테이너는 `--rm` 이다 — 끝나면 `docker stop <이름>`. 같은 이름이 이미 떠 있으면 실패한다(다른 워크트리의 서버를 끄지 않는다 —
// 로컬은 `--name` · `--port` 로 가른다). 새 DB 는 TARGETS 에 한 항목을 더한다.

import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));

// CI 서비스와 같은 판(.github/workflows/ci.yml 의 db-live 잡). 올리면 둘을 함께 올린다.
const PG_IMAGE =
  "postgres:17.6@sha256:00bc86618629af00d2937fdc5a5d63db3ff8450acf52f0636ec813c7f4902929";
const REDIS_IMAGE =
  "redis:7.2.10@sha256:8cd70b529aec2af55a072917098c1ae6f1d9eb975ac1cf36928c7d5af7574f08";

/** CA 경로를 넘기는 변수(모든 DB 공용). */
export const CA_ENV = "PW_TEST_TLS_CA";

// DB → 기본 포트 · 컨테이너 안 포트 · 시험에 넘길 URL 변수와 값 · 컨테이너에 넣을 파일 · docker 인자.
export const TARGETS = {
  pg: {
    port: 5443,
    innerPort: 5432,
    urlEnv: "PW_TEST_PG_TLS_URL",
    url: (port) => `postgres://pitwall:pitwall@localhost:${port}/pitwall`,
    // 평문으로 붙으면 거절되는 사용자(`allow` 가 TLS 로 다시 붙는지 본다). 나머지는 평문 · TLS 둘 다 받는다.
    files: {
      "pg_hba.conf": [
        "local all all trust",
        "hostnossl all pitwall_tls_only all reject",
        "host all all all scram-sha-256",
        "",
      ].join("\n"),
      "init.sql": "CREATE ROLE pitwall_tls_only LOGIN PASSWORD 'pitwall';\n",
    },
    args: ({ out }) => [
      "-e",
      "POSTGRES_USER=pitwall",
      "-e",
      "POSTGRES_PASSWORD=pitwall",
      "-e",
      "POSTGRES_DB=pitwall",
      "-v",
      `${out}:/pw-tls:ro`,
      "--entrypoint",
      "sh",
      PG_IMAGE,
      "-c",
      // 서버 키는 postgres 사용자의 0600 이어야 한다 — 마운트한 파일의 소유자는 호스트 사용자라 복사해 맞춘다.
      [
        "install -o postgres -m 600 /pw-tls/server.key /var/lib/postgresql/server.key",
        "install -o postgres -m 644 /pw-tls/server.pem /var/lib/postgresql/server.pem",
        "install -m 644 /pw-tls/pg_hba.conf /var/lib/postgresql/pg_hba.conf",
        "cp /pw-tls/init.sql /docker-entrypoint-initdb.d/",
        "exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/var/lib/postgresql/server.pem " +
          "-c ssl_key_file=/var/lib/postgresql/server.key -c hba_file=/var/lib/postgresql/pg_hba.conf",
      ].join(" && "),
    ],
  },
  // TLS 만 받는 Redis(평문 포트를 닫는다 — 평문 시험은 CI 서비스의 PW_TEST_REDIS_URL). 클라이언트 인증서는 요구하지 않는다.
  redis: {
    port: 6380,
    innerPort: 6380,
    urlEnv: "PW_TEST_REDIS_TLS_URL",
    url: (port) => `rediss://localhost:${port}/0`,
    files: {},
    args: ({ out }) => [
      "-v",
      `${out}:/pw-tls:ro`,
      "--entrypoint",
      "sh",
      REDIS_IMAGE,
      "-c",
      // 마운트한 키는 호스트 사용자의 것이라 redis 사용자가 못 읽을 수 있다 — 복사해 맞춘다.
      [
        "install -d -o redis /tls",
        "install -o redis -m 600 /pw-tls/server.key /tls/server.key",
        "install -o redis -m 644 /pw-tls/server.pem /tls/server.pem",
        "install -o redis -m 644 /pw-tls/ca.pem /tls/ca.pem",
        "exec docker-entrypoint.sh redis-server --port 0 --tls-port 6380 --tls-cert-file /tls/server.pem " +
          "--tls-key-file /tls/server.key --tls-ca-cert-file /tls/ca.pem --tls-auth-clients no",
      ].join(" && "),
    ],
  },
};

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} failed:\n${result.stderr}`);
  return result.stdout;
}

/** 시험 CA 와 그 CA 가 서명한 서버 인증서(RSA 2048 · SHA-256 · serverAuth — macOS 신뢰 평가의 조건). */
export function makeCertificates(out) {
  mkdirSync(out, { recursive: true });
  const at = (f) => join(out, f);
  run("openssl", [
    "req",
    "-x509",
    "-newkey",
    "rsa:2048",
    "-nodes",
    "-sha256",
    "-days",
    "30",
    "-subj",
    "/CN=Pitwall test CA",
    "-addext",
    "basicConstraints=critical,CA:TRUE",
    "-addext",
    "keyUsage=critical,keyCertSign,cRLSign",
    "-keyout",
    at("ca.key"),
    "-out",
    at("ca.pem"),
  ]);
  run("openssl", [
    "req",
    "-newkey",
    "rsa:2048",
    "-nodes",
    "-subj",
    "/CN=localhost",
    "-keyout",
    at("server.key"),
    "-out",
    at("server.csr"),
  ]);
  writeFileSync(
    at("server.ext"),
    [
      "basicConstraints=critical,CA:FALSE",
      "keyUsage=critical,digitalSignature,keyEncipherment",
      "extendedKeyUsage=serverAuth",
      "subjectAltName=DNS:localhost,IP:127.0.0.1",
      "",
    ].join("\n"),
  );
  run("openssl", [
    "x509",
    "-req",
    "-sha256",
    "-days",
    "30",
    "-in",
    at("server.csr"),
    "-CA",
    at("ca.pem"),
    "-CAkey",
    at("ca.key"),
    "-CAcreateserial",
    "-extfile",
    at("server.ext"),
    "-out",
    at("server.pem"),
  ]);
}

/** docker `-p` 값. 포트가 0 이면 도커가 빈 포트를 고른다. */
export function publish(port, innerPort) {
  return port === 0 ? `127.0.0.1::${innerPort}` : `127.0.0.1:${port}:${innerPort}`;
}

/** 인증서가 `out` 에 있을 때 그 DB 의 TLS 서버를 띄운다. 포트 0 은 빈 포트. 띄운 이름 · 포트와 시험에 넘길 변수를 돌려준다. */
export function startTls(db, { name, port, out }) {
  const target = TARGETS[db];
  for (const [file, text] of Object.entries(target.files)) writeFileSync(join(out, file), text);
  run("docker", [
    "run",
    "-d",
    "--rm",
    "--name",
    name,
    "-p",
    publish(port, target.innerPort),
    ...target.args({ out }),
  ]);
  let bound = port;
  if (port === 0) {
    // `127.0.0.1:49153` 꼴의 줄.
    const line = run("docker", ["port", name, `${target.innerPort}/tcp`])
      .trim()
      .split("\n")[0];
    bound = Number(line.split(":").at(-1));
  }
  return {
    name,
    port: bound,
    env: { [target.urlEnv]: target.url(bound), [CA_ENV]: join(out, "ca.pem") },
  };
}

/** 그 이름의 컨테이너를 끈다(`--rm` 이라 지워진다). 없으면 아무것도 하지 않는다. */
export function stopTls(name) {
  spawnSync("docker", ["stop", name], { encoding: "utf8" });
}

export function parseArgs(argv) {
  const options = { out: join(root, "target", "db-tls"), name: "pitwall-tls", ports: {}, dbs: [] };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--out") options.out = resolve(argv[++i]);
    else if (arg === "--name") options.name = argv[++i];
    else if (arg === "--port") {
      const [db, port] = (argv[++i] ?? "").split("=");
      if (!TARGETS[db] || !/^\d+$/.test(port ?? ""))
        throw new Error(`--port expects <db>=<port>, got ${argv[i]}`);
      options.ports[db] = Number(port);
    } else if (TARGETS[arg]) options.dbs.push(arg);
    else throw new Error(`unknown argument ${arg} (dbs: ${Object.keys(TARGETS).join(", ")})`);
  }
  if (options.dbs.length === 0) options.dbs = Object.keys(TARGETS);
  return options;
}

function main(argv) {
  const options = parseArgs(argv);
  makeCertificates(options.out);
  const env = {};
  for (const db of options.dbs) {
    const started = startTls(db, {
      name: `${options.name}-${db}`,
      port: options.ports[db] ?? TARGETS[db].port,
      out: options.out,
    });
    console.error(`${db}: started ${started.name} on 127.0.0.1:${started.port}`);
    Object.assign(env, started.env);
  }
  const lines = Object.entries(env).map(([key, value]) => `${key}=${value}`);
  if (process.env.GITHUB_ENV) appendFileSync(process.env.GITHUB_ENV, `${lines.join("\n")}\n`);
  console.log(lines.join("\n"));
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`db-tls: ${e.message}`);
    process.exitCode = 2;
  }
}
