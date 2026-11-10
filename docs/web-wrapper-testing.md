# Testing the CADRS browser wrapper

This guide tests the generic `postMessage` API that lets a host page create or load a CADRS
document, then request native `.cadrs` and STEP bytes. The host page is responsible for persisting
those returned files on its own server.

## 1. Check and install build prerequisites

The repository pins Rust to `nightly-2026-10-21` in `rust-toolchain.toml`. Use that toolchain for
the browser build; do not substitute the system `stable` toolchain. From the repository root,
check whether the pin file is present and which toolchain Cargo will select:

```bash
git status --short -- rust-toolchain.toml
rustup show active-toolchain
```

If `rust-toolchain.toml` exists and is unchanged, it selects the pinned nightly and declares the
`wasm32-unknown-unknown` target. Rustup installs the declared toolchain/components/targets when
needed. If that file is missing or locally changed, explicitly install and select the repo's
pinned toolchain instead:

```bash
rustup toolchain install nightly-2026-10-21 --profile minimal \
  --component rustfmt --component clippy --target wasm32-unknown-unknown
rustc +nightly-2026-10-21 --version
rustup target list --installed --toolchain nightly-2026-10-21
```

For Ubuntu/Debian, the browser CI installs these system packages for the build scripts and the
`ring` C/assembly build:

```bash
sudo apt update
sudo apt install clang mold
clang --version
mold --version
```

The build script also requires `wasm-bindgen-cli` exactly matching the locked crate version. This
checkout's `Cargo.lock` pins `wasm-bindgen` to `0.2.128`. Check the installed CLI and install the
matching one if it is absent or different:

```bash
wasm-bindgen --version
cargo +nightly-2026-10-21 install --locked wasm-bindgen-cli --version 0.2.128
```

The main web build prerequisites are therefore: Rust nightly `nightly-2026-10-21`, target
`wasm32-unknown-unknown`, `clang`, `mold`, and `wasm-bindgen-cli 0.2.128`. A WebGPU-capable browser
is additionally needed to run the app after it builds. Linux desktop packages such as
`libwayland-dev` are not needed for this browser target.

## 2. Build the browser app and generate `target/web`

Run from the root of the CADRS checkout containing the wrapper changes. The first build may take
several minutes:

```bash
RUSTUP_TOOLCHAIN=nightly-2026-10-21 tools/web/build.sh
test -f target/web/index.html || { echo "ERROR: build did not create target/web/index.html"; exit 1; }
echo "Browser build output is ready"
```

`RUSTUP_TOOLCHAIN` makes the intended toolchain explicit if `rust-toolchain.toml` is missing or
changed in the local checkout. On success, the script builds the WASM app, runs `wasm-bindgen`, and
places `index.html`, `cadrs.js`, `cadrs_bg.wasm.gz`, `files.js`, `cadrs.png`, and `bundle.tar.gz`
under `target/web/`. The end of a successful run says `Built target/web. Try it:
python3 -m http.server -d target/web 8080`; the `test -f` check then confirms the entry page exists.

**A successful `cargo check` is not this build step.** It type-checks Rust code, but does not run
`wasm-bindgen`, copy `index.html`/`files.js`, create `bundle.tar.gz`, or populate `target/web/`.
Therefore, `cargo check ...` can finish successfully while `target/web/` is still empty. Run
`tools/web/build.sh` above before starting the HTTP server.

If you only want to type-check the browser code without generating the deployable web files, run:

```bash
cargo +nightly-2026-10-21 check -r --locked \
  --target wasm32-unknown-unknown -p cadrs --no-default-features -F gpu
```

Expected success ends with `Finished 'release' profile ...`. Warnings may still appear; they do
not mean the check failed if Cargo exits successfully. In particular, `cargo fix --lib
-p cadrs_core` is not a build prerequisite: it tries to edit code to address lints, and refuses
to run when the worktree is dirty. Do not run `cargo fix` for this test.

If Cargo reports compiler errors such as `E0505` or `E0597`, first confirm
`rustup show active-toolchain` reports `nightly-2026-10-21` (or use the explicit `cargo
+nightly-2026-10-21` command above). The repository's pinned nightly browser check passed in the
wrapper worktree. If the same errors remain with that exact toolchain, retain the complete
diagnostics and report them; do not try to silence them with `cargo fix`.

## 3. Serve CADRS and the host page

From the same repository root, confirm the generated entry page exists, then start the server:

```bash
test -f target/web/index.html || { echo "ERROR: run tools/web/build.sh first"; exit 1; }
python3 -m http.server -d target/web 8080
```

Open `http://localhost:8080/`. It should load CADRS, not a directory listing. The command
`python3 -m http.server -d target/web 8080` serves exactly the directory named by `-d`.
If that directory has no `index.html`, Python shows a directory listing at `/` and returns 404
for `/favicon.ico` (and other missing assets). That means the browser build did not populate that
directory, the build failed, or the server command is using a different checkout/output path.
If the preflight fails, stop: do not start the server. Re-run the full build step above from the
intended checkout and check the first build error. The browser needs WebGPU; use a supported
browser with WebGPU enabled.

In terminal 2, create an empty directory for the host harness and save the HTML from the next
section there as `host.html`. From that directory, run:

```bash
python3 -m http.server 5173
```

Open `http://localhost:5173/host.html`. The host origin is
`http://localhost:5173`; the CADRS origin is `http://localhost:8080`. They must be distinct
origins. The iframe URL opts in the host origin with the exact parameter
`cadrs-api-origins=http%3A%2F%2Flocalhost%3A5173`. For non-local deployments, use canonical HTTPS
origins. HTTP is allowed only for `localhost`, `127.0.0.1`, and `[::1]`.

## 4. Minimal test harness

Save this as `host.html` in the directory served on port 5173:

```html
<!doctype html>
<meta charset="utf-8">
<title>CADRS wrapper test</title>
<h1>CADRS wrapper test</h1>
<p id="status">Loading CADRS…</p>
<button id="new" disabled>Create new CADRS document</button>
<button id="step" disabled>Import STEP</button>
<button id="open" disabled>Open .cadrs</button>
<button id="save" disabled>Save .cadrs bytes</button>
<button id="export" disabled>Export STEP bytes</button>
<pre id="result"></pre>
<iframe id="cadrs" title="CADRS" style="width:100%;height:75vh;border:0"></iframe>

<script>
const CADRS_ORIGIN = "http://localhost:8080";
const frame = document.querySelector("#cadrs");
const status = document.querySelector("#status");
const output = document.querySelector("#result");
const buttons = [...document.querySelectorAll("button")];
const pending = new Map();
let documentKey = null;
let sequence = 0;

frame.src = `${CADRS_ORIGIN}/?cadrs-api-origins=${encodeURIComponent(location.origin)}`;

frame.addEventListener("load", () => {
  status.textContent = "CADRS page loaded. Choose an operation below.";
  buttons.forEach((button) => button.disabled = false);
});

window.addEventListener("message", (event) => {
  if (event.source !== frame.contentWindow || event.origin !== CADRS_ORIGIN) return;
  const message = event.data;
  if (message?.protocol !== "cadrs.api" || message.version !== 1) return;

  const waiter = pending.get(message.requestId);
  if (!waiter) return;
  clearTimeout(waiter.timer);
  pending.delete(message.requestId);
  if (message.ok) waiter.resolve(message);
  else waiter.reject(new Error(message.error || "CADRS API error"));
});

function request(op, key, fields = {}) {
  const requestId = `host-test-${++sequence}`;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(requestId);
      reject(new Error(`No response for ${op} after 5 minutes`));
    }, 5 * 60 * 1000);
    pending.set(requestId, { resolve, reject, timer });

    const message = {
      protocol: "cadrs.api",
      version: 1,
      requestId,
      op,
      documentKey: key,
      ...fields,
    };
    const transfer = fields.bytes instanceof ArrayBuffer ? [fields.bytes] : [];
    frame.contentWindow.postMessage(message, CADRS_ORIGIN, transfer);
  });
}

function newKey() {
  documentKey = `test-document-${crypto.randomUUID()}`;
  return documentKey;
}

function show(message) {
  output.textContent = JSON.stringify(message, (key, value) => {
    if (value instanceof Uint8Array) return `[Uint8Array: ${value.byteLength} bytes]`;
    return value;
  }, 2);
}

async function run(operation) {
  output.textContent = "Working…";
  try {
    const result = await operation();
    show(result);
  } catch (error) {
    output.textContent = `ERROR: ${error.message}`;
    console.error(error);
  }
}

function chooseFile(accept) {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.addEventListener("change", () => resolve(input.files?.[0] ?? null), { once: true });
    input.click();
  });
}

function download(file) {
  const blob = new Blob([file.bytes], { type: file.mimeType });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = file.name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

document.querySelector("#new").onclick = () => run(async () => {
  const key = newKey();
  return request("create-new", key, { name: "Wrapper test" });
});

document.querySelector("#step").onclick = () => run(async () => {
  const file = await chooseFile(".step,.stp,.p21");
  if (!file) return { cancelled: true };
  const key = newKey();
  return request("import-step", key, {
    name: file.name,
    bytes: await file.arrayBuffer(),
  });
});

document.querySelector("#open").onclick = () => run(async () => {
  const file = await chooseFile(".cadrs");
  if (!file) return { cancelled: true };
  const key = newKey();
  return request("open-cadrs", key, {
    name: file.name,
    bytes: await file.arrayBuffer(),
  });
});

document.querySelector("#save").onclick = () => run(async () => {
  if (!documentKey) throw new Error("Create or open a document first.");
  const response = await request("save-cadrs", documentKey);
  download(response.result);
  return {
    op: response.op,
    name: response.result.name,
    mimeType: response.result.mimeType,
    bytes: response.result.bytes.byteLength,
    downloaded: true,
  };
});

document.querySelector("#export").onclick = () => run(async () => {
  if (!documentKey) throw new Error("Create or open a document first.");
  const response = await request("export-step", documentKey);
  download(response.result);
  return {
    op: response.op,
    name: response.result.name,
    mimeType: response.result.mimeType,
    bytes: response.result.bytes.byteLength,
    downloaded: true,
  };
});
</script>
```

## 5. Exercise each flow

1. Click **Create new CADRS document**. A success response contains `ok: true` and
   `result.documentName`. Create some geometry in the CADRS iframe before testing STEP export.
2. Click **Save .cadrs bytes**. The host receives `result.bytes` as a `Uint8Array` and downloads the
   `.cadrs` file. Open **DevTools → Network** to confirm the page itself does not upload it.
3. Click **Export STEP bytes**. If the active document/tab has parts and its rebuild is complete,
   the host receives STEP bytes and downloads the `.step` file. A blank document has no parts to
   export and should return an error.
4. Click **Import STEP**, select a `.step`, `.stp`, or `.p21` file, then wait for the import
   response. CADRS converts the STEP data into a new document; STEP does not contain CADRS feature
   history.
5. Save both outputs separately with **Save .cadrs bytes** and **Export STEP bytes**. To verify
   continued editing, click **Open .cadrs** and choose the saved native file. This reopens the
   CADRS document with its native edit history.

Responses echo `requestId`, `op`, and `documentKey`. Success has `ok: true` and `result`; errors
have `ok: false` and an `error` message. Export `result` contains `name`, `mimeType`, and `bytes`.
The harness displays the byte count and downloads the result. The request protocol is
`cadrs.api`, version `1`; request bytes must be an `ArrayBuffer`.

## What this test does not do

- The wrapper is implemented in the current worktree; it is not automatically present in the
  public CADRS site. Build and serve this checkout to test these changes.
- CADRS does **not** upload files to a server. The harness demonstrates bytes returned to the host
  by downloading them. A real host must authenticate its user and upload/store both `.cadrs` and
  STEP using its own backend, and maintain the document/version association.
- The browser app keeps its own CADRS files in IndexedDB when available. `/Uploads` is scratch
  space; without IndexedDB, CADRS files last only for that tab. This browser-side persistence is
  separate from the host server storage.
- The API supports at most four outstanding requests and 512 MiB per binary payload. Keep edits
  paused while requesting both exports if they must correspond to the same revision: they are
  independent operations and do not form an atomic snapshot.
- The origin allowlist is a message-routing check, not authentication. Protect host data with the
  host's own authorization and server-side checks.
