import type { GitObject, Ref, RefList, Resolution } from './wasm/git_remote_refs_wasm.js';

export { default as init, parseAdvertisement } from './wasm/git_remote_refs_wasm.js';
export type {
	GitObject,
	GitRequest,
	ObjectFormat,
	ObjectKind,
	Ref,
	RefList,
	Resolution,
	Signature,
	TreeEntry,
} from './wasm/git_remote_refs_wasm.js';

export interface RemoteOptions {
	fetch?: (url: string, init: RequestInit) => Response | Promise<Response>;
	signal?: AbortSignal;
}

export interface ListRefsOptions extends RemoteOptions {
	prefixes?: string[];
}

export interface Peeled {
	chain: GitObject[];
	target: GitObject;
}

export function listRefs(repository: string | URL, options?: ListRefsOptions): Promise<RefList>;

export function getRef(repository: string | URL, name: string, options?: RemoteOptions): Promise<Ref | undefined>;

export function resolve(repository: string | URL, name: string, options?: RemoteOptions): Promise<Resolution>;

export function getObject(
	repository: string | URL,
	oid: string,
	options?: RemoteOptions,
): Promise<GitObject>;

export function peel(repository: string | URL, oid: string, options?: RemoteOptions): Promise<Peeled>;
