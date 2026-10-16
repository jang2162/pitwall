import type { Platform } from "@pitwall/keymap";

/** 키맵의 OS별 표기(10-ui §4.1)를 고르는 OS. */
export function keymapPlatform(userAgent: string): Platform {
  if (/Mac/.test(userAgent)) return "mac";
  if (/Windows/.test(userAgent)) return "win";
  return "linux";
}
