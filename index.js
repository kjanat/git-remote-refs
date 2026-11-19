import initWasm, { parseAdvertisement } from './wasm/git_remote_refs.js';

export { parseAdvertisement };

let initialization;

async function loadWasm(input) {
	if (input !== undefined) return initWasm(input);
	const url = new URL('./wasm/git_remote_refs_bg.wasm', import.meta.url);
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

export async function listRefs(repository, { fetch = globalThis.fetch, signal } = {}) {
	const url = new URL(repository);
	if (!['https:', 'http:'].includes(url.protocol) || url.search || url.hash) {
		throw new TypeError('Expected an HTTP(S) Git repository URL without query or fragment');
	}
	url.pathname = url.pathname.replace(/\/+$/, '') + '/info/refs';
	url.searchParams.set('service', 'git-upload-pack');
	const type = 'application/x-git-upload-pack-advertisement';
	const response = await fetch(url, { headers: { Accept: type }, signal });
	if (!response.ok) throw new Error(`Git discovery returned HTTP ${response.status}`);
	if (response.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== type) {
		throw new Error('Expected a Git upload-pack advertisement');
	}
	await init();
	return parseAdvertisement(new Uint8Array(await response.arrayBuffer()));
}
