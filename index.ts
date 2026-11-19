import type {
	Fetched,
	GitObject,
	GitRequest,
	InitOutput,
	Ref,
	RefList,
	Resolution,
} from './wasm/git_remote_refs_wasm.js';
import initWasm, { candidates, parseAdvertisement, Remote } from './wasm/git_remote_refs_wasm.js';

export { parseAdvertisement };
export type {
	Fetched,
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

export type InitArgument = Parameters<typeof initWasm>[0];

type Reply = [status: number, contentType: string | null, body: Uint8Array];
type Exchange = (request: GitRequest) => Promise<Reply>;

const MAX_TAG_CHAIN = 16;

let initialization: Promise<InitOutput> | undefined;

async function loadWasm(input: InitArgument): Promise<InitOutput> {
	if (input !== undefined) return initWasm(input);
	const url = new URL('./wasm/git_remote_refs_wasm_bg.wasm', import.meta.url);
	if (url.protocol === 'file:' && globalThis.process?.versions?.node) {
		const { readFile } = await import('node:fs/promises');
		return initWasm({ module_or_path: await readFile(url) });
	}
	return initWasm({ module_or_path: url });
}

export function init(input?: InitArgument): Promise<InitOutput> {
	initialization ??= loadWasm(input).catch((error: unknown) => {
		initialization = undefined;
		throw error;
	});
	return initialization;
}

async function session<T>(
	repository: string | URL,
	options: RemoteOptions | undefined,
	work: (remote: Remote, exchange: Exchange) => Promise<T>,
): Promise<T> {
	const { fetch = globalThis.fetch, signal } = options ?? {};
	await init();
	const remote = new Remote(String(repository));
	const exchange: Exchange = async request => {
		const response = await fetch(request.url, {
			method: request.method,
			headers: request.headers,
			...(request.body && { body: new Uint8Array(request.body) }),
			...(signal && { signal }),
		});
		return [response.status, response.headers.get('content-type'), new Uint8Array(await response.arrayBuffer())];
	};
	try {
		remote.acceptDiscovery(...(await exchange(remote.discoveryRequest())));
		return await work(remote, exchange);
	} finally {
		remote.free();
	}
}

async function list(remote: Remote, exchange: Exchange, prefixes: string[]): Promise<RefList> {
	const request = remote.listRequest(prefixes);
	if (request !== undefined) remote.acceptList(...(await exchange(request)));
	return remote.refs();
}

async function fetchObjects(remote: Remote, exchange: Exchange, oids: string[]): Promise<Fetched> {
	return remote.acceptFetch(...(await exchange(remote.fetchRequest(oids))), oids);
}

export function listRefs(repository: string | URL, options?: ListRefsOptions): Promise<RefList> {
	return session(repository, options, (remote, exchange) => list(remote, exchange, options?.prefixes ?? []));
}

export function getRef(repository: string | URL, name: string, options?: RemoteOptions): Promise<Ref | undefined> {
	return session(
		repository,
		options,
		async (remote, exchange) => (await list(remote, exchange, [name])).refs.get(name),
	);
}

export function resolve(repository: string | URL, name: string, options?: RemoteOptions): Promise<Resolution> {
	return session(repository, options, async (remote, exchange) => {
		await list(remote, exchange, candidates(name));
		return remote.resolve(name);
	});
}

export function getObject(repository: string | URL, oid: string, options?: RemoteOptions): Promise<GitObject> {
	return session(repository, options, async (remote, exchange) => {
		const { wanted: [object] } = await fetchObjects(remote, exchange, [oid]);
		return object;
	});
}

export function peel(repository: string | URL, oid: string, options?: RemoteOptions): Promise<Peeled> {
	return session(repository, options, async (remote, exchange) => {
		let { wanted: [object], objects } = await fetchObjects(remote, exchange, [oid]);
		const chain = [object];
		while (object.kind === 'tag') {
			if (chain.length >= MAX_TAG_CHAIN) {
				throw Object.assign(new Error(`limit exceeded: tag chain longer than ${MAX_TAG_CHAIN} objects`), {
					code: 'limit-exceeded',
				});
			}
			const known = objects.get(object.target);
			if (known === undefined) {
				({ wanted: [object], objects } = await fetchObjects(remote, exchange, [object.target]));
			} else {
				object = known;
			}
			chain.push(object);
		}
		return { chain, target: object };
	});
}
