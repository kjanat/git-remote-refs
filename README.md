# git-remote-refs

Read refs from Git smart HTTP protocol v0. Public repositories that allow anonymous Git reads need one GET. The Rust function `parse_advertisement(&[u8])` accepts the full response body and preserves ref names as bytes. The JS bindings expose UTF-8 names as strings and throw on invalid UTF-8.

Build with the Rust target `wasm32-unknown-unknown`, wasm-bindgen-cli 0.2.128, and `npm run build`.

```js
import { init, listRefs, parseAdvertisement } from "./index.js";

// Node or Deno with node:fs support
import { readFile } from "node:fs/promises";
await init({ module_or_path: await readFile(new URL("./wasm/git_remote_refs_bg.wasm", import.meta.url)) });

const remote = await listRefs("https://github.com/denoland/setup-deno.git", { fetch });
console.log(remote.refs.get("refs/heads/main"));
console.log(remote.symrefs.get("HEAD"));

const saved = parseAdvertisement(await readFile("denoland-setup-deno-git-upload-pack.txt"));
console.log(saved.refs.size);
```

In a browser, serve the generated wasm directory alongside index.js and call `await init()`. The Git endpoint must permit cross-origin reads or be served through your own origin. Supply a fetch-compatible function to control transport or credentials. An optional `signal` is forwarded to fetch.
