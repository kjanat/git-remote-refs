import type { Advertisement } from "./wasm/git_remote_refs.js";

export { default as init, parseAdvertisement } from "./wasm/git_remote_refs.js";
export type { Advertisement } from "./wasm/git_remote_refs.js";

export interface ListRefsOptions {
  fetch?: (url: URL, options: RequestInit) => Response | Promise<Response>;
  signal?: AbortSignal;
}

export function listRefs(
  repository: string | URL,
  options?: ListRefsOptions,
): Promise<Advertisement>;
