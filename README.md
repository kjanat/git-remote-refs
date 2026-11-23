# git-remote-refs

Read refs and objects from a Git repository over smart HTTP, the way `git ls-remote` and `git fetch` do, without cloning it. It works with GitHub, GitLab, Gitea, Forgejo and any self-hosted Git server, and needs no credentials for public repositories.

One Rust core ships four ways: an npm package, a Rust crate, a WebAssembly component and a Go module.

## JavaScript

```sh
npm install git-remote-refs
```

```js
import { getObject, resolve } from 'git-remote-refs';

const repo = 'https://github.com/denoland/setup-deno.git';
const v2 = await resolve(repo, 'v2');
// { kind: 'ref', name: 'refs/heads/v2', ref: { oid: '22d081ff…' }, others: [] }
const commit = await getObject(repo, v2.ref.oid);
// { kind: 'commit', oid, tree, parents, author, committer, message }
```

| Function                       | Returns                                                                                                                           |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `listRefs(repo, { prefixes })` | Every advertised ref with its `oid`, `peeled` tag target and `symrefTarget`, plus unborn symrefs such as `HEAD` of an empty repo. |
| `getRef(repo, name)`           | The ref with exactly this full name, or `undefined`.                                                                              |
| `resolve(repo, name)`          | The ref Git's `rev-parse` would pick for a short name, or the object for a full object ID.                                        |
| `getObject(repo, oid)`         | The commit, tag, tree or blob with this ID, checked against its hash.                                                             |
| `peel(repo, oid)`              | The chain of tags from `oid` to the first object that is not a tag.                                                               |
| `parseAdvertisement(bytes)`    | The refs in a saved `info/refs` response.                                                                                         |

`resolve` tries `name`, `refs/name`, `refs/tags/name`, `refs/heads/name`, `refs/remotes/name` and `refs/remotes/name/HEAD` in that order. It returns `{ kind: 'ref', name, ref, others }`, `{ kind: 'object', oid, refs }` or `{ kind: 'not-found' }`. Revision syntax such as `v2^{}` fails with `invalid-ref-name`.

Every function accepts `{ fetch, signal }` for a custom HTTP client and cancellation. Errors carry a `code`, such as `not-found`, `auth-required` or `remote`, and HTTP failures carry a `status`. GitHub answers `401` for a missing repository.

Names, emails and messages are decoded as UTF-8. Text that is not valid UTF-8 also comes with its original bytes in `nameBytes`, `emailBytes` or `messageBytes`.

In a browser, serve the package's `wasm/` directory next to `index.js`, and route requests through an origin that allows cross-origin reads.

## Rust

```sh
cargo add git-remote-refs
```

The crate is `no_std` with `alloc` and does no I/O. It builds each HTTP request and parses each response, and you send the requests with any client. Here `send` stands for that client and returns the status, content type and body:

```rust
use git_remote_refs::{Discovery, Remote, Response, parse_discovery, parse_ls_refs, resolve};

let remote = Remote::new("https://github.com/denoland/setup-deno.git")?;
let (status, content_type, body) = send(remote.discovery_request())?;
let discovery = parse_discovery(&Response { status, content_type: content_type.as_deref(), body: &body })?;
let refs = match discovery {
    Discovery::V0(refs) => refs,
    Discovery::V2(capabilities) => {
        let prefixes = ["refs/heads/v2", "refs/tags/v2"];
        let (status, content_type, body) = send(remote.ls_refs_request(&capabilities, &prefixes)?)?;
        let response = Response { status, content_type: content_type.as_deref(), body: &body };
        parse_ls_refs(&capabilities, &response, &prefixes)?
    }
};
let resolution = resolve(&refs, "v2")?;
```

`fetch_request` and `parse_fetch` download and verify objects, and `Limits` caps how many objects and bytes a response may hold.

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

resolution, err := client.Resolve(ctx, "https://github.com/denoland/setup-deno.git", "v2")
// &gitremoterefs.RefMatch{Name: "refs/heads/v2", Ref: gitremoterefs.Ref{OID: "22d081ff…"}, Others: []string{}}
```

The module is pure Go and needs no cgo. `Client` has `ListRefs`, `GetRef`, `Resolve`, `GetObject`, `Peel` and `ParseAdvertisement`, which behave like their JavaScript counterparts. It is safe for concurrent use. Names, emails and messages hold the raw bytes from the object. Protocol failures are `*gitremoterefs.Error` values with a `Code`, a `Status` for HTTP failures and a `Detail`.

## WebAssembly component

Each release attaches `git-remote-refs.component.wasm`, a component that exports the interface `kjanat:git-remote-refs/git` from [`bindings/component/wit/world.wit`](bindings/component/wit/world.wit). It imports nothing, so it runs in any component host. The component builds each request, the host sends it, and the host passes the response back.

## Protocol notes

Listing refs takes one request on protocol v0 servers and two on protocol v2 servers, and only v2 servers filter by prefix. Fetching objects needs protocol v2 with shallow fetch. Servers with partial clone filters send only the requested objects and the objects their tags point to. Other servers send the whole snapshot.

GitHub serves objects from a repository's whole fork network, so `getObject` can return a commit that exists only in a fork. To prove a commit belongs to a repository, find a branch or tag whose history reaches it.

## License

MIT
