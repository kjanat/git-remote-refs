import initWasm, { candidates, parseAdvertisement, Remote } from './wasm/git_remote_refs_wasm.js';

export { parseAdvertisement };

const MAX_TAG_CHAIN = 16;

let initialization;

async function loadWasm(input) {
	if (input !== undefined) return initWasm(input);
	const url = new URL('./wasm/git_remote_refs_wasm_bg.wasm', import.meta.url);
	if (url.protocol === 'file:' && globalThis.process?.versions?.node) {
		const { readFile } = await import('node:fs/promises');
		return initWasm({ module_or_path: await readFile(url) });
	}
	return initWasm({ module_or_path: url });
}

export function init(input) {
	initialization ??= loadWasm(input).catch(error => {
		initialization = undefined;
		throw error;
	});
	return initialization;
}

async function session(repository, { fetch = globalThis.fetch, signal } = {}, work) {
	await init();
	const remote = new Remote(String(repository));
	const exchange = async request => {
		const response = await fetch(request.url, {
			method: request.method,
			headers: request.headers,
			body: request.body,
			signal,
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

async function list(remote, exchange, prefixes) {
	const request = remote.listRequest(prefixes);
	if (request !== undefined) remote.acceptList(...(await exchange(request)));
	return remote.refs();
}

async function objects(remote, exchange, oids) {
	return remote.acceptFetch(...(await exchange(remote.fetchRequest(oids))), oids);
}

export function listRefs(repository, { prefixes = [], ...options } = {}) {
	return session(repository, options, (remote, exchange) => list(remote, exchange, prefixes));
}

export function getRef(repository, name, options) {
	return session(
		repository,
		options,
		async (remote, exchange) => (await list(remote, exchange, [name])).refs.get(name),
	);
}

export function resolve(repository, name, options) {
	return session(repository, options, async (remote, exchange) => {
		await list(remote, exchange, candidates(name));
		return remote.resolve(name);
	});
}

export function getObject(repository, oid, options) {
	return session(
		repository,
		options,
		async (remote, exchange) => (await objects(remote, exchange, [oid])).get(oid.toLowerCase()),
	);
}

export function peel(repository, oid, options) {
	return session(repository, options, async (remote, exchange) => {
		const chain = [];
		let known = new Map();
		let next = oid.toLowerCase();
		for (;;) {
			if (!known.has(next)) known = await objects(remote, exchange, [next]);
			const object = known.get(next);
			chain.push(object);
			if (object.kind !== 'tag') return { chain, target: object };
			if (chain.length >= MAX_TAG_CHAIN) {
				throw Object.assign(new Error(`limit exceeded: tag chain longer than ${MAX_TAG_CHAIN} objects`), {
					code: 'limit-exceeded',
				});
			}
			next = object.target;
		}
	});
}
