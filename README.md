# git-remote-refs

Read refs and objects from any Git smart HTTP server without credentials, the way `git ls-remote` and `git fetch` do. Works with GitHub, GitLab, Gitea, Forgejo and self-hosted Git.

```js
import { getObject, resolve } from 'git-remote-refs';

const repo = 'https://github.com/denoland/setup-deno.git';
const v2 = await resolve(repo, 'v2');
// { kind: 'ref', name: 'refs/heads/v2', ref: { oid: '22d081ff…' }, others: [] }
const commit = await getObject(repo, v2.ref.oid);
// { kind: 'commit', tree, parents, author, committer, message, … }
```

## JavaScript API

| Function                       | Result                                                                                                                                                                        |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `listRefs(repo, { prefixes })` | `RefList`: every advertised ref with `oid`, `peeled` and `symrefTarget`, unborn symrefs, and the `prefixes` the listing covers (`null` for all refs).                         |
| `getRef(repo, name)`           | The ref with exactly this full name, or `undefined`.                                                                                                                          |
| `resolve(repo, name)`          | Git's `rev-parse` lookup order: `name`, `refs/name`, `refs/tags/name`, `refs/heads/name`, `refs/remotes/name`, `refs/remotes/name/HEAD`. A full object ID resolves to itself. |
| `getObject(repo, oid)`         | A commit, tag, tree or blob whose bytes hash to `oid`.                                                                                                                        |
| `peel(repo, oid)`              | The tag chain from `oid` to its first non-tag object, up to 16 objects.                                                                                                       |
| `parseAdvertisement(bytes)`    | A `RefList` from saved `info/refs` bytes.                                                                                                                                     |

`resolve` returns `{ kind: 'ref', name, ref, others }`, where `others` lists the lower-priority refs with the same short name. It returns `{ kind: 'object', oid, refs }` for a full object ID, and `{ kind: 'not-found' }` otherwise. Revision syntax such as `v2^{}` fails with `invalid-ref-name`, and an abbreviated object ID is looked up as a ref name.

Every function takes `{ fetch, signal }` to supply an HTTP transport or cancel requests. Errors carry a `code` such as `not-found`, `auth-required`, `not-smart-http`, `unsupported-feature` or `remote`, and HTTP errors carry `status`. GitHub answers `401` for repositories that do not exist.

Text in names, emails and messages is decoded as UTF-8. When the bytes are not valid UTF-8, the object also carries the original bytes in `nameBytes`, `emailBytes` or `messageBytes`, and commits report their `encoding` header.

Listing refs takes one request on protocol v0 servers and two on protocol v2 servers. With `prefixes`, a v2 server sends only matching refs. Objects need protocol v2 with shallow fetch. Servers with partial clone filters send the requested objects and the objects their tags point to. Other servers send the whole snapshot.

`getObject` proves that the server has an object with that ID. GitHub serves objects from a repository's whole fork network, so `getObject` can return a commit that exists only in a fork. To show that a commit is part of the repository, find a branch or tag whose history reaches it.

In a browser, serve the generated `wasm/` directory alongside `index.js`, and route requests through an origin that permits cross-origin reads.

## Rust crate

The `git-remote-refs` crate is `no_std` with `alloc` and performs no I/O. `Remote` builds each `Request`, the caller sends it, and `parse_discovery`, `parse_ls_refs` and `parse_fetch` read the `Response`. `resolve` and `candidates` implement the lookup order over a `RefList`. `Limits` bounds object count and size.

## WebAssembly component

`bindings/component` exports the same `Remote` API as a WebAssembly component. Its interface is `kjanat:git-remote-refs/git` in `bindings/component/wit/world.wit`. The component imports nothing, so the host sends each request and passes the response back. Errors are the `error` variant, and maps are lists of name and value pairs.

## Development

`mise run build` compiles `bindings/wasm` into `wasm/`, then compiles `index.ts` with tsrs into `index.js` and `index.d.ts`. `mise run component:build` writes the component to `component/git-remote-refs.wasm`. `mise run test` runs the Rust, component and JavaScript suites against local repositories served by `git http-backend`.
