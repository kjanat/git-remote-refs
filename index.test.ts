import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { before, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';
import type { RemoteOptions } from './index.js';
import { getObject, getRef, listRefs, parseAdvertisement, peel, resolve } from './index.js';

type Fetch = NonNullable<RemoteOptions['fetch']>;

const root = fileURLToPath(new URL('./target/js-fixtures', import.meta.url));
const environment = {
	...process.env,
	GIT_CONFIG_NOSYSTEM: '1',
	GIT_CONFIG_GLOBAL: '/dev/null',
	GIT_TERMINAL_PROMPT: '0',
};

before(() => {
	execFileSync(fileURLToPath(new URL('./tests/fixture.sh', import.meta.url)), [root], { stdio: 'inherit' });
});

function bytes(body: RequestInit['body']): Uint8Array {
	return body instanceof Uint8Array ? body : new Uint8Array();
}

function backend({ v2 = true }: { v2?: boolean } = {}): Fetch {
	return async (url, init) => {
		const { pathname, search } = new URL(url);
		const body = bytes(init.body);
		const headers = new Headers(init.headers);
		const contentType = headers.get('Content-Type');
		const protocol = headers.get('Git-Protocol');
		const result = spawnSync('git', ['http-backend'], {
			input: body,
			env: {
				...environment,
				GIT_PROJECT_ROOT: root,
				GIT_HTTP_EXPORT_ALL: '1',
				PATH_INFO: pathname,
				QUERY_STRING: search.slice(1),
				REQUEST_METHOD: init.method ?? 'GET',
				CONTENT_LENGTH: String(body.length),
				...(contentType === null ? {} : { CONTENT_TYPE: contentType }),
				...(v2 && protocol !== null ? { HTTP_GIT_PROTOCOL: protocol } : {}),
			},
		});
		const split = result.stdout.indexOf('\r\n\r\n');
		let status = 200;
		const responseHeaders = new Headers();
		for (const line of result.stdout.subarray(0, split).toString().split('\r\n')) {
			const separator = line.indexOf(': ');
			const name = line.slice(0, separator);
			const value = line.slice(separator + 2);
			if (name.toLowerCase() === 'status') status = Number(value.slice(0, 3));
			else responseHeaders.set(name, value);
		}
		return new Response(new Uint8Array(result.stdout.subarray(split + 4)), { status, headers: responseHeaders });
	};
}

const fetch = backend();
const repo = (name: string) => `http://fixtures/${name}.git`;
const rev = (name: string, revision: string) =>
	execFileSync('git', ['-C', `${root}/${name}.git`, 'rev-parse', revision], { env: environment }).toString().trim();

test('listRefs returns refs with peeled tags, symrefs and coverage', async () => {
	const list = await listRefs(repo('sha1'), { fetch });
	assert.equal(list.objectFormat, 'sha1');
	assert.equal(list.prefixes, null);
	assert.deepEqual(list.refs.get('HEAD'), { oid: rev('sha1', 'main'), symrefTarget: 'refs/heads/main' });
	assert.deepEqual(list.refs.get('refs/tags/annotated'), {
		oid: rev('sha1', 'annotated'),
		peeled: rev('sha1', 'main'),
	});
	const tags = await listRefs(repo('sha1'), { fetch, prefixes: ['refs/tags/v'] });
	assert.deepEqual([...tags.refs.keys()], ['refs/tags/v2']);
	assert.deepEqual(tags.prefixes, ['refs/tags/v']);
});

test('protocol v0 servers return the same listing', async () => {
	const v0 = backend({ v2: false });
	assert.deepEqual(await listRefs(repo('sha1'), { fetch: v0 }), await listRefs(repo('sha1'), { fetch }));
	const filtered = await listRefs(repo('sha1'), { fetch: v0, prefixes: ['refs/heads/'] });
	assert.deepEqual(
		[...filtered.refs.keys()],
		['refs/heads/heads/x', 'refs/heads/main', 'refs/heads/v2', 'refs/heads/x'],
	);
	await assert.rejects(getObject(repo('sha1'), rev('sha1', 'main'), { fetch: v0 }), { code: 'unsupported-feature' });
});

test('getRef looks up the exact name', async () => {
	assert.equal((await getRef(repo('sha1'), 'refs/heads/heads/x', { fetch }))?.oid, rev('sha1', 'refs/heads/heads/x'));
	assert.equal(await getRef(repo('sha1'), 'heads/x', { fetch }), undefined);
});

test('resolve follows Git order and reports other matches', async () => {
	assert.deepEqual(await resolve(repo('sha1'), 'v2', { fetch }), {
		kind: 'ref',
		name: 'refs/tags/v2',
		ref: { oid: rev('sha1', 'refs/tags/v2') },
		others: ['refs/heads/v2'],
	});
	const literal = await resolve(repo('sha1'), 'heads/x', { fetch });
	assert(literal.kind === 'ref');
	assert.equal(literal.name, 'refs/heads/x');
	assert.deepEqual(await resolve(repo('sha1'), rev('sha1', 'main'), { fetch }), {
		kind: 'object',
		oid: rev('sha1', 'main'),
		refs: [],
	});
	assert.deepEqual(await resolve(repo('sha1'), 'missing', { fetch }), { kind: 'not-found' });
	await assert.rejects(resolve(repo('sha1'), 'v2^{}', { fetch }), { code: 'invalid-ref-name' });
});

test('getObject returns verified commits', async () => {
	const commit = await getObject(repo('sha1'), rev('sha1', 'main').toUpperCase(), { fetch });
	assert(commit.kind === 'commit');
	assert.equal(commit.oid, rev('sha1', 'main'));
	assert.equal(commit.tree, rev('sha1', 'main^{tree}'));
	assert.deepEqual(commit.parents, [rev('sha1', 'main~1')]);
	assert.deepEqual(commit.author, {
		name: 'Ada Lovelace',
		email: 'ada@example.com',
		time: 1767315845,
		offsetMinutes: 120,
	});
	assert.equal(commit.message, 'second\n\nwith a body\n');
	const blob = await getObject(repo('sha1'), rev('sha1', 'main:file'), { fetch });
	assert.deepEqual(blob, { kind: 'blob', oid: rev('sha1', 'main:file'), data: new TextEncoder().encode('one\n') });
	const tree = await getObject(repo('sha256'), rev('sha256', 'main^{tree}'), { fetch });
	assert(tree.kind === 'tree');
	assert.deepEqual(
		tree.entries.map(entry => entry.name),
		['empty', 'file', 'numbers', 'numbers-changed'],
	);
});

test('non-UTF-8 text keeps its original bytes', async () => {
	const commit = await getObject(repo('latin1'), rev('latin1', 'main'), { fetch });
	assert(commit.kind === 'commit');
	assert.equal(commit.encoding, 'ISO-8859-1');
	assert.equal(commit.message, 'Caf�\n');
	assert.deepEqual(commit.messageBytes, new Uint8Array([0x43, 0x61, 0x66, 0xe9, 0x0a]));
	assert.deepEqual(commit.author.nameBytes, new Uint8Array([0xc9, 0x6d, 0x69, 0x6c, 0x65]));
	assert.equal(commit.committer.nameBytes, undefined);
	const tree = await getObject(repo('latin1'), commit.tree, { fetch });
	assert(tree.kind === 'tree');
	assert.deepEqual(tree.entries[0]?.nameBytes, new Uint8Array([0x63, 0x61, 0x66, 0xe9]));
});

test('peel follows nested tags to their target', async () => {
	let requests = 0;
	const counting: Fetch = async (url, init) => {
		requests += 1;
		return fetch(url, init);
	};
	const { chain, target } = await peel(repo('sha1'), rev('sha1', 'nested'), { fetch: counting });
	assert.equal(requests, 2);
	assert.deepEqual(
		chain.map(object => object.oid),
		[rev('sha1', 'nested'), rev('sha1', 'annotated'), rev('sha1', 'main')],
	);
	assert.equal(target.kind, 'commit');
	const tree = await peel(repo('sha1'), rev('sha1', 'tree-tag'), { fetch });
	assert.equal(tree.target.kind, 'tree');
	requests = 0;
	let served = false;
	const tagOnly: Fetch = async (url, init) => {
		if (served || init.method !== 'POST' || !new TextDecoder().decode(bytes(init.body)).includes('command=fetch')) {
			return counting(url, init);
		}
		served = true;
		requests += 1;
		const raw = execFileSync('git', ['-C', `${root}/sha1.git`, 'cat-file', 'tag', 'annotated'], { env: environment });
		const header = [0x80 | (4 << 4) | (raw.length & 15)];
		for (let rest = raw.length >> 4; rest > 0; rest >>= 7) header.push((rest & 0x7f) | (rest > 0x7f ? 0x80 : 0));
		const body = Buffer.concat([
			Buffer.from('PACK\0\0\0\x02\0\0\0\x01', 'latin1'),
			Buffer.from(header),
			deflateSync(raw),
		]);
		const pack = Buffer.concat([body, createHash('sha1').update(body).digest()]);
		const line = (data: Buffer) => Buffer.concat([Buffer.from((data.length + 4).toString(16).padStart(4, '0')), data]);
		const response = Buffer.concat([
			line(Buffer.from('packfile\n')),
			line(Buffer.concat([Buffer.from([1]), pack])),
			Buffer.from('0000'),
		]);
		return new Response(new Uint8Array(response), {
			headers: { 'content-type': 'application/x-git-upload-pack-result' },
		});
	};
	const split = await peel(repo('sha1'), rev('sha1', 'annotated'), { fetch: tagOnly });
	assert.deepEqual(
		split.chain.map(object => object.oid),
		[rev('sha1', 'annotated'), rev('sha1', 'main')],
	);
	assert.equal(requests, 3);
	const longest = await peel(repo('deep'), rev('deep', 't14'), { fetch });
	assert.equal(longest.chain.length, 16);
	await assert.rejects(peel(repo('deep'), rev('deep', 't16'), { fetch }), { code: 'limit-exceeded' });
});

test('requests carry the caller signal', async () => {
	const controller = new AbortController();
	const seen: Array<AbortSignal | null | undefined> = [];
	const watching: Fetch = async (url, init) => {
		seen.push(init.signal);
		return fetch(url, init);
	};
	await listRefs(repo('sha1'), { fetch: watching, signal: controller.signal });
	assert.deepEqual(seen, [controller.signal, controller.signal]);
	controller.abort();
	const aborting: Fetch = async (url, init) => {
		init.signal?.throwIfAborted();
		return fetch(url, init);
	};
	await assert.rejects(listRefs(repo('sha1'), { fetch: aborting, signal: controller.signal }), {
		name: 'AbortError',
	});
});

test('errors carry codes and statuses', async () => {
	await assert.rejects(listRefs(repo('missing'), { fetch }), { code: 'not-found', status: 404 });
	await assert.rejects(listRefs('ssh://example.com/repo', { fetch }), { code: 'invalid-url' });
	await assert.rejects(listRefs(repo('sha1'), { fetch, prefixes: ['a b'] }), { code: 'invalid-prefix' });
	await assert.rejects(getObject(repo('sha1'), 'abc', { fetch }), { code: 'invalid-object-id' });
	await assert.rejects(getObject(repo('sha1'), '1'.repeat(40), { fetch }), { code: 'remote' });
	const html: Fetch = async () => new Response('<html>', { headers: { 'content-type': 'text/html' } });
	await assert.rejects(listRefs(repo('sha1'), { fetch: html }), { code: 'not-smart-http' });
});

test('parseAdvertisement reads a saved v0 advertisement', async () => {
	const response = await backend({ v2: false })(`${repo('sha1')}/info/refs?service=git-upload-pack`, {
		method: 'GET',
		headers: {},
	});
	const list = parseAdvertisement(new Uint8Array(await response.arrayBuffer()));
	assert.equal(list.refs.get('refs/heads/main')?.oid, rev('sha1', 'main'));
});

test('the Wasm Remote refuses work before discovery', async () => {
	const { Remote } = await import('./wasm/git_remote_refs_wasm.js');
	const remote = new Remote(repo('sha1'));
	try {
		assert.throws(() => remote.refs(), { code: 'protocol' });
		assert.throws(() => remote.listRequest([]), { code: 'protocol' });
		assert.throws(() => remote.fetchRequest(['1'.repeat(40)]), { code: 'protocol' });
		assert.throws(() => remote.fetchRequest([]), { code: 'invalid-object-id' });
		assert.throws(() => remote.acceptFetch(200, null, new Uint8Array(), []), { code: 'invalid-object-id' });
	} finally {
		remote.free();
	}
});
