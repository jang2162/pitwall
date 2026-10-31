// Redis 키 문서의 문자열(14-m3-plan M3-18). 내용은 Fluent 리소스 extensions/i18n-default/i18n/*/db-redis.ftl 에 있다. 여기 적은 id
// 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import type { KvParseRefusal } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

/** 콘솔 명령 거절의 갈래(`PERMISSION_DENIED` 의 `data.reason = "blockedCommand"` 일 때 `data.block`, crates/pw-db/src/kv.rs). */
export type KvBlock = "notAllowed" | "blocking" | "scanCount";

export function dbRedisStrings(t: Translate) {
  return {
    title: (connection: string, database: string) => t("db-redis-title", { connection, database }),
    actions: {
      open: t("db-redis-action-open"),
      badArgs: t("db-redis-open-bad-args"),
    },
    bar: {
      connect: t("db-redis-connect"),
      disconnect: t("db-redis-disconnect"),
      connecting: t("db-redis-connecting"),
      notConnected: t("db-redis-not-connected"),
      database: t("db-redis-database"),
      pattern: t("db-redis-pattern"),
      patternPlaceholder: t("db-redis-pattern-placeholder"),
      separator: t("db-redis-separator"),
      rescan: t("db-redis-rescan"),
      stop: t("db-redis-stop"),
      connectFailed: (message: string) => t("db-redis-connect-failed", { message }),
    },
    tree: {
      label: t("db-redis-keys"),
      scanning: (count: number) => t("db-redis-scanning", { count }),
      scanned: (count: number, ms: number) => t("db-redis-scanned", { count, ms }),
      stopped: (count: number) => t("db-redis-scan-stopped", { count }),
      capped: (count: number) => t("db-redis-scan-capped", { count }),
      failed: (message: string) => t("db-redis-scan-failed", { message }),
      empty: t("db-redis-no-keys"),
      bytesKey: t("db-redis-bytes-key"),
    },
    type: {
      string: t("db-redis-type-string"),
      hash: t("db-redis-type-hash"),
      list: t("db-redis-type-list"),
      set: t("db-redis-type-set"),
      zset: t("db-redis-type-zset"),
      stream: t("db-redis-type-stream"),
      json: t("db-redis-type-json"),
    },
    viewer: {
      pickKey: t("db-redis-pick-key"),
      missing: t("db-redis-missing"),
      ttl: (ttl: string) => t("db-redis-ttl", { ttl }),
      noTtl: t("db-redis-no-ttl"),
      expired: t("db-redis-ttl-expired"),
      size: (size: string) => t("db-redis-size", { size }),
      total: (count: number) => t("db-redis-total", { count }),
      page: (page: number) => t("db-redis-page", { page }),
      prev: t("db-redis-prev-page"),
      next: t("db-redis-next-page"),
      reload: t("db-redis-reload"),
      view: {
        text: t("db-redis-view-text"),
        json: t("db-redis-view-json"),
        hex: t("db-redis-view-hex"),
      },
      unsupported: (type: string) => t("db-redis-unsupported", { type }),
      failed: (message: string) => t("db-redis-read-failed", { message }),
      loading: t("db-redis-loading"),
    },
    column: {
      field: t("db-redis-col-field"),
      value: t("db-redis-col-value"),
      member: t("db-redis-col-member"),
      score: t("db-redis-col-score"),
      index: t("db-redis-col-index"),
      id: t("db-redis-col-id"),
    },
    key: {
      open: t("db-redis-open-value"),
      editTtl: t("db-redis-edit-ttl"),
      clearTtl: t("db-redis-clear-ttl"),
      delete: t("db-redis-delete"),
      rename: t("db-redis-rename"),
      copyName: t("db-redis-copy-name"),
      ttlTitle: (key: string) => t("db-redis-ttl-title", { key }),
      ttlSeconds: t("db-redis-ttl-seconds"),
      ttlInvalid: t("db-redis-ttl-invalid"),
      renameTitle: (key: string) => t("db-redis-rename-title", { key }),
      renameTo: t("db-redis-rename-to"),
      renameSame: t("db-redis-rename-same"),
      apply: t("db-redis-apply"),
      cancel: t("db-redis-cancel"),
      overwriteNote: (target: string) => t("db-redis-overwrite-note", { target }),
      deleted: (key: string) => t("db-redis-deleted", { key }),
      renamed: (from: string, to: string) => t("db-redis-renamed", { from, to }),
      ttlChanged: (key: string) => t("db-redis-ttl-changed", { key }),
      ttlCleared: (key: string) => t("db-redis-ttl-cleared", { key }),
      gone: t("db-redis-key-gone"),
      failed: (message: string) => t("db-redis-action-failed", { message }),
      copied: t("db-redis-name-copied"),
    },
    console: {
      label: t("db-redis-console"),
      placeholder: t("db-redis-console-placeholder"),
      send: t("db-redis-console-send"),
      needsSession: t("db-redis-console-needs-session"),
      bytesArg: t("db-redis-console-bytes-arg"),
      parseFailed: (message: string) => t("db-redis-console-parse-failed", { message }),
      parseRefused: {
        tooLarge: t("db-redis-console-parse-too-large"),
        newline: t("db-redis-console-parse-newline"),
        nul: t("db-redis-console-parse-nul"),
        empty: t("db-redis-console-parse-empty"),
        unterminatedQuote: t("db-redis-console-parse-unterminated-quote"),
        closingQuote: t("db-redis-console-parse-closing-quote"),
      } satisfies Record<KvParseRefusal, string>,
      cancelled: t("db-redis-console-cancelled"),
      busy: t("db-redis-console-busy"),
      keyTooLarge: (message: string) => t("db-redis-console-key-too-large", { message }),
      failed: (message: string) => t("db-redis-console-failed", { message }),
      blocked: {
        notAllowed: (command: string) => t("db-redis-blocked-not-allowed", { command }),
        blocking: (command: string) => t("db-redis-blocked-blocking", { command }),
        scanCount: (command: string) => t("db-redis-blocked-scan-count", { command }),
      } satisfies Record<KvBlock, (command: string) => string>,
      reply: {
        nil: t("db-redis-reply-nil"),
        integer: (value: string) => t("db-redis-reply-integer", { value }),
        double: (value: string) => t("db-redis-reply-double", { value }),
        error: (message: string) => t("db-redis-reply-error", { message }),
        empty: t("db-redis-reply-empty"),
        truncated: (elements: number, size: string) =>
          t("db-redis-reply-truncated", { elements, size }),
      },
    },
  };
}

export type DbRedisStrings = ReturnType<typeof dbRedisStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useDbRedisStrings(): DbRedisStrings {
  const { l10n } = useLocalization();
  return useMemo(() => dbRedisStrings(translator(l10n)), [l10n]);
}
