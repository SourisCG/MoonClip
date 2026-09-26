import { invoke } from "@tauri-apps/api/core";

/**
 * Thumbnail bytes over IPC: the webview cannot load `asset://` reliably, so
 * the backend streams the JPEG and we expose a revocable Blob URL.
 */
export async function thumbnailUrl(name: string): Promise<string> {
  const raw = await invoke<ArrayBuffer | number[]>("read_thumbnail", {
    thumbnailName: name,
  });
  const buffer = raw instanceof ArrayBuffer ? raw : new Uint8Array(raw).buffer;
  return URL.createObjectURL(new Blob([buffer], { type: "image/jpeg" }));
}
