// 라이선스 · 출처 확인 다이얼로그(15-m4-plan M4-17 — 설치기의 `4001`, 결정 M4-D2 의 「처음 한 번」): 패키지 이름 · 설명 · 판 ·
// 라이선스(SPDX 목록 링크, 덮어쓰기 표의 문장) · 받을 곳(도메인 · 주소) · 무결성(고정 해시 · 체크섬 없음) · 받을 크기. 처음 포커스는
// [취소] 다 — 사람이 치던 Enter · Space 한 번으로 받기가 시작되지 않게(신뢰 확인 · 위험 작업 확인과 같은 규칙).
//
// 내용 중 registry 에서 온 글(설명 · 라이선스 · 주소 · 덮어쓰기 문장)은 미신뢰라 글로만 그린다(React 텍스트 — HTML 로 해석하지
// 않는다). 링크는 http(s) 만 만들고(`safeUrl`) 외부 브라우저로 연다. 설명 · 주소는 `tools.registry.get` 로 더 읽는다 — 읽지 못해도
// 확인 값(`4001` 의 data)만으로 그린다.

import type { ToolConfirmRequired, ToolPackageDetail } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import {
  formatBytes,
  licenseUrl,
  safeUrl,
  type ToolChangeKind,
  type ToolConfirmAsk,
  type ToolsCall,
} from "./install";
import { useToolsStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";

/** 받을 주소들(파일이면 그 주소, npm · pypi 면 레지스트리의 패키지 쪽). */
export function sourceUrls(detail: ToolPackageDetail | undefined): string[] {
  const source = detail?.resolved?.source;
  if (!source) return [];
  switch (source.kind) {
    case "files":
      return source.files.map((f) => f.url);
    case "npm":
      return [source.package, ...source.extra].map(
        (p) => `https://www.npmjs.com/package/${p.name}${p.version ? `/v/${p.version}` : ""}`,
      );
    case "pypi":
      return [source.package, ...source.extra].map(
        (p) => `https://pypi.org/project/${p.name}/${p.version ?? ""}`,
      );
  }
}

function ExternalLink({
  url,
  children,
  open,
}: {
  url: string | undefined;
  children: ReactNode;
  open(url: string): void;
}) {
  if (!url) return <span>{children}</span>;
  return (
    <a
      href={url}
      className="text-ui-accent underline"
      onClick={(e) => {
        e.preventDefault();
        open(url);
      }}
    >
      {children}
    </a>
  );
}

interface Pending {
  request: ToolConfirmRequired;
  kind: ToolChangeKind;
  resolve(ok: boolean): void;
}

export function ToolConfirmDialog({
  request,
  kind,
  detail,
  openUrl,
  onAnswer,
}: {
  request: ToolConfirmRequired;
  kind: ToolChangeKind;
  detail: ToolPackageDetail | undefined;
  openUrl(url: string): void;
  onAnswer(ok: boolean): void;
}) {
  const T = useToolsStrings().confirm;
  useKeymapPopup(true);
  const cancel = useRef<HTMLButtonElement>(null);
  const urls = sourceUrls(detail);
  const homepage = safeUrl(detail?.homepage);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="tool-confirm-dialog"
          data-kind={kind}
          onOpenAutoFocus={(e) => {
            // 처음 포커스는 [취소] — Radix 의 기본(첫 포커스 가능 요소 = 링크)을 쓰지 않는다.
            e.preventDefault();
            cancel.current?.focus();
          }}
          className="sheet-top fixed left-1/2 flex max-h-[80vh] w-[560px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 overflow-y-auto rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">
            {kind === "install"
              ? T.title(request.name, request.version)
              : T.updateTitle(request.name, request.version)}
          </Dialog.Title>
          <Dialog.Description className="text-ui-muted-foreground">
            {request.rejected ? T.changed : T.intro}
          </Dialog.Description>
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-[12px]">
            {detail?.summary.description ? (
              <>
                <dt className="text-ui-muted-foreground">{T.description}</dt>
                <dd data-testid="tool-confirm-description" className="whitespace-pre-wrap">
                  {detail.summary.description}
                </dd>
              </>
            ) : null}
            <dt className="text-ui-muted-foreground">{T.version}</dt>
            <dd data-testid="tool-confirm-version">{request.version}</dd>
            <dt className="text-ui-muted-foreground">{T.license}</dt>
            <dd data-testid="tool-confirm-license" className="flex flex-wrap gap-x-2">
              {request.licenses.map((id) => (
                <ExternalLink key={id} url={licenseUrl(id)} open={openUrl}>
                  {id}
                </ExternalLink>
              ))}
            </dd>
            {request.note ? (
              <>
                <dt className="text-ui-muted-foreground">{T.note}</dt>
                <dd data-testid="tool-confirm-note" className="whitespace-pre-wrap">
                  {request.note}
                </dd>
              </>
            ) : null}
            <dt className="text-ui-muted-foreground">{T.source}</dt>
            <dd data-testid="tool-confirm-source" className="flex min-w-0 flex-col">
              <span>{request.sourceDomains.join(" · ")}</span>
              {urls.map((url) => (
                <span key={url} className="truncate">
                  <ExternalLink url={safeUrl(url)} open={openUrl}>
                    {url}
                  </ExternalLink>
                </span>
              ))}
            </dd>
            <dt className="text-ui-muted-foreground">{T.integrity}</dt>
            <dd
              data-testid="tool-confirm-integrity"
              data-checksum={request.checksum}
              className={request.checksum === "none" ? "text-ui-accent" : undefined}
            >
              {request.checksum === "pinned" ? T.integrityPinned : T.integrityNone}
            </dd>
            <dt className="text-ui-muted-foreground">{T.size}</dt>
            <dd data-testid="tool-confirm-size">
              {request.size === null ? T.sizeUnknown : formatBytes(Number(request.size))}
            </dd>
            {homepage ? (
              <>
                <dt className="text-ui-muted-foreground">{T.homepage}</dt>
                <dd className="truncate">
                  <ExternalLink url={homepage} open={openUrl}>
                    {homepage}
                  </ExternalLink>
                </dd>
              </>
            ) : null}
          </dl>
          <div className="flex justify-end gap-2">
            <button
              ref={cancel}
              type="button"
              data-testid="tool-confirm-cancel"
              className={BUTTON}
              onClick={() => onAnswer(false)}
            >
              {T.cancel}
            </button>
            <button
              type="button"
              data-testid="tool-confirm-accept"
              className={`${BUTTON} border-ui-accent`}
              onClick={() => onAnswer(true)}
            >
              {kind === "install" ? T.install : T.update}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * 확인 다이얼로그를 띄우는 `ask` 와 창이 그릴 `element`. `call` 은 설명 · 주소를 더 읽는 데(`tools.registry.get`) 쓴다 — 없으면
 * 확인 값만 보인다.
 */
export function useToolConfirm(
  call: ToolsCall | undefined,
  openUrl: (url: string) => void,
): { ask: ToolConfirmAsk; element: ReactNode } {
  const [pending, setPending] = useState<Pending>();
  const [detail, setDetail] = useState<ToolPackageDetail>();
  const latest = useRef(call);
  latest.current = call;
  const ask = useCallback<ToolConfirmAsk>(
    (request, kind) =>
      new Promise<boolean>((resolve) => {
        setDetail(undefined);
        setPending((before) => {
          // 앞의 다이얼로그가 아직 열려 있으면 취소로 끝낸다(한 번에 하나).
          before?.resolve(false);
          return { request, kind, resolve };
        });
      }),
    [],
  );
  const name = pending?.request.name;
  useEffect(() => {
    if (name === undefined) return;
    let live = true;
    latest
      .current?.("tools.registry.get", { name })
      .then((d) => live && setDetail(d))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [name, pending]);
  const element = pending ? (
    <ToolConfirmDialog
      request={pending.request}
      kind={pending.kind}
      detail={detail?.summary.name === pending.request.name ? detail : undefined}
      openUrl={openUrl}
      onAnswer={(ok) => {
        setPending(undefined);
        pending.resolve(ok);
      }}
    />
  ) : null;
  return { ask, element };
}
