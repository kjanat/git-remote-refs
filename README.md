# git-remote-refs

[![GitHub release](https://img.shields.io/github/v/release/kjanat/git-remote-refs?logo=github&logoColor=white&label=release&labelColor=181717&color=black)](https://github.com/kjanat/git-remote-refs/releases/latest)
[![Go module version](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fproxy.golang.org%2Fgithub.com%2Fkjanat%2Fgit-remote-refs%2Fgo%2F%40latest&query=%24.Version&label=go&logo=go&logoColor=white&labelColor=00ADD8&color=black)](https://pkg.go.dev/github.com/kjanat/git-remote-refs/go)
[![NPM](https://img.shields.io/npm/v/git-remote-refs?logo=npm&labelColor=CB3837&color=black)](https://npm.im/git-remote-refs)
[![Crates.io](https://img.shields.io/crates/v/git-remote-refs?logo=rust&labelColor=B7410E&color=black)](https://crates.io/crates/git-remote-refs)

Read refs and objects from a Git repository over smart HTTP, the way `git ls-remote` and `git fetch` do, without cloning it. It works with GitHub, GitLab, Gitea, Forgejo and any self-hosted Git server, and needs no credentials for public repositories.

One Rust core ships four ways: an npm package, a Rust crate, a WebAssembly component and a Go module.

## JavaScript

```sh
npm install git-remote-refs
```

```js
import { resolveObject } from 'git-remote-refs';

const repo = 'https://github.com/denoland/setup-deno.git';
const { resolution, object } = await resolveObject(repo, 'v2');
// resolution: { kind: 'ref', name: 'refs/heads/v2', ref: { oid: '22d081ff…' }, others: [] }
// object: { kind: 'commit', oid, tree, parents, author, committer, message }
```

| Function                       | Returns                                                                                                                           |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `listRefs(repo, { prefixes })` | Every advertised ref with its `oid`, `peeled` tag target and `symrefTarget`, plus unborn symrefs such as `HEAD` of an empty repo. |
| `getRef(repo, name)`           | The ref with exactly this full name, or `undefined`.                                                                              |
| `resolve(repo, name)`          | The ref Git's `rev-parse` would pick for a short name, or the object for a full ID in the repository’s object format.             |
| `resolveObject(repo, name)`    | The resolution and exact object, using one discovery request.                                                                     |
| `getObject(repo, oid)`         | The commit, tag, tree or blob with this ID, checked against its hash.                                                             |
| `peel(repo, oid)`              | The chain of tags from `oid` to the first object that is not a tag.                                                               |
| `parseAdvertisement(bytes)`    | The refs in a saved `info/refs` response.                                                                                         |

`resolveObject` sets `object` to `undefined` for a name with no match. A full object ID in the repository’s object format resolves to `kind: 'object'`. If the server lacks that object, `resolveObject` rejects with `remote`. A full ID in a different object format is looked up as a ref name. An annotated tag resolves to the tag object itself. Use `peel` to follow the tag to its target.

`resolve` tries `name`, `refs/name`, `refs/tags/name`, `refs/heads/name`, `refs/remotes/name` and `refs/remotes/name/HEAD` in that order. It returns `{ kind: 'ref', name, ref, others }`, `{ kind: 'object', oid, refs }` or `{ kind: 'not-found' }`. Revision syntax such as `v2^{}` fails with `invalid-ref-name`.

Remote calls accept `{ fetch, signal, maxResponseBytes }` for a custom HTTP client, cancellation, and a per-response byte limit. Zero or an omitted limit selects 264 MiB. Errors carry a `code`, such as `not-found`, `auth-required` or `remote`, and HTTP failures carry a `status`. GitHub answers `401` for a missing repository.

Names, emails and messages are decoded as UTF-8. Text that is not valid UTF-8 also comes with its original bytes in `nameBytes`, `emailBytes` or `messageBytes`.

In a browser, serve the package's `wasm/` directory next to `index.js`, and route requests through an origin that allows cross-origin reads.

## Rust

```sh
cargo add git-remote-refs
```

The crate is `no_std` with `alloc` and does no I/O. It builds each HTTP request and parses each response, and you send the requests with any client. Here `send` stands for that client and returns the status, content type and body.

```rust
use git_remote_refs::{Discovery, Remote, Response, candidates, parse_discovery, parse_ls_refs, resolve};

let remote = Remote::new("https://github.com/denoland/setup-deno.git")?;
let name = "v2";
let (status, content_type, body) = send(remote.discovery_request())?;
let discovery = parse_discovery(&Response { status, content_type: content_type.as_deref(), body: &body })?;
let refs = match discovery {
    Discovery::V0(refs) => refs,
    Discovery::V2(capabilities) => {
        let names = candidates(name);
        let prefixes: Vec<&str> = names.iter().map(String::as_str).collect();
        let (status, content_type, body) = send(remote.ls_refs_request(&capabilities, &prefixes)?)?;
        let response = Response { status, content_type: content_type.as_deref(), body: &body };
        parse_ls_refs(&capabilities, &response, &prefixes)?
    }
};
let resolution = resolve(&refs, name)?;
```

On protocol v2, `candidates` gives you every name `resolve` checks. Pass the same prefixes to `ls_refs_request` and `parse_ls_refs` so `resolve` knows which names the listing covered. If a candidate is absent and its name was outside the requested prefixes, `resolve` returns `Resolution::Incomplete`.

Use `Remote::fetch_request` to build an object request and `parse_fetch` to verify and unpack the server's response. `Limits` caps the object count and unpacked data size.

## Go

```sh
go get github.com/kjanat/git-remote-refs/go
```

```go
import gitremoterefs "github.com/kjanat/git-remote-refs/go"

client, err := gitremoterefs.New(ctx, http.DefaultClient)
if err != nil {
	return err
}
defer client.Close(ctx)

resolved, err := client.ResolveObject(ctx, "https://github.com/denoland/setup-deno.git", "v2")
if err != nil {
    return err
}
// resolved.Resolution is a *gitremoterefs.RefMatch for refs/heads/v2.
// resolved.Object is a *gitremoterefs.Commit.
```

The module is pure Go and needs no cgo. `Client` has `ListRefs`, `GetRef`, `Resolve`, `ResolveObject`, `GetObject`, `Peel` and `ParseAdvertisement`, which behave like their JavaScript counterparts. It is safe for concurrent use. Names, emails and messages hold the raw bytes from the object. Protocol failures are `*gitremoterefs.Error` values with a `Code`, a `Status` for HTTP failures and a `Detail`.

The per-response byte limit defaults to 264 MiB. Set `ClientOptions.MaxResponseBytes` with `NewWithOptions` to change it.

## WebAssembly component

Each release attaches `git-remote-refs.component.wasm`, a component that exports the interface `kjanat:git-remote-refs/git` from [`bindings/component/wit/world.wit`](bindings/component/wit/world.wit). It imports nothing, so it runs in any component host. The component builds each request, the host sends it, and the host passes the response back.

## Protocol notes

Listing refs takes one request on protocol v0 servers and two on protocol v2 servers, and only v2 servers filter by prefix. Fetching objects needs protocol v2 with shallow fetch. Servers with partial clone filters send only the requested objects and the objects their tags point to. Other servers send the whole snapshot.

GitHub serves objects from a repository's whole fork network, so `getObject` can return a commit that exists only in a fork. To prove a commit belongs to a repository, find a branch or tag whose history reaches it.

## License

MIT
