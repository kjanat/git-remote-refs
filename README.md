# git-remote-refs

List a remote Git repository’s advertised refs by URL. Public repositories that allow anonymous Git reads need one smart-HTTP GET.

```js
import { listRefs } from 'git-remote-refs';

const { refs, symrefs } = await listRefs('https://github.com/denoland/setup-deno.git');
console.log(symrefs.get('HEAD')); // refs/heads/main
console.log(refs.get('refs/heads/main')); // object ID
```

`listRefs` loads the bundled Wasm automatically. Pass `{ fetch, signal }` as the second argument to supply your own HTTP transport or cancel a request. In a browser, serve the generated `wasm/` directory alongside `index.js`. The Git endpoint must permit cross-origin reads or be served through your own origin.

To build from source, install the Rust target `wasm32-unknown-unknown` and run `npm run build` with mise available.
