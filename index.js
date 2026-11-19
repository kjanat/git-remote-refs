import init, { parseAdvertisement } from './wasm/git_remote_refs.js';

export { init, parseAdvertisement };

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
	return parseAdvertisement(new Uint8Array(await response.arrayBuffer()));
}
