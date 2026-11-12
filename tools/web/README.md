# Embedding the CADRS web app

For a step-by-step local build and wrapper test harness, see
[`docs/web-wrapper-testing.md`](../../docs/web-wrapper-testing.md).

The browser build optionally exposes a byte-oriented `postMessage` API for a host page. It is
disabled unless the CADRS page URL includes `cadrs-api-origins` with one or more exact origins,
comma-separated. For example, an iframe served from `https://cadrs.example/app/` can use:

```html
<iframe src="https://cadrs.example/app/?cadrs-api-origins=https%3A%2F%2Fshop.example"></iframe>
```

The host must send messages to the exact CADRS origin. CADRS accepts messages only from
`window.parent` and only when `event.origin` exactly matches an allowlisted origin. Requests use
this shape:

```js
{
  protocol: "cadrs.api",
  version: 1,
  requestId: "unique-host-request-id",
  op: "create-new",
  documentKey: "host-owned-document-id",
  name: "Bracket"
}
```

Every response echoes `requestId`, `op`, and `documentKey`, and contains `ok` plus either `result`
or `error`. Binary request values are `ArrayBuffer`s; binary export responses contain a `Uint8Array`
in `result.bytes`. Up to four requests may be outstanding. `documentKey` is opaque to CADRS and
must identify the currently open document for save/export operations.

Supported operations:

| Operation | Request fields | Successful result |
|---|---|---|
| `create-new` | `name` (optional) | `documentName` |
| `open-cadrs` | `name`, `bytes` | `documentName` |
| `import-step` | `name`, `bytes` | `documentName` |
| `save-cadrs` | — | `name`, `mimeType`, `bytes` |
| `export-step` | — | `name`, `mimeType`, `bytes` |

`create-new` starts a blank native CADRS document. `import-step` converts a STEP file into a new
CADRS document, retaining assembly structure when present. Save its `.cadrs` output and reopen it
with `open-cadrs` to continue editing with the CADRS feature history intact. STEP is an
interchange/export format and does not preserve that feature history.

For a host save, request both `save-cadrs` and `export-step` for the same `documentKey`, correlate
their responses by `requestId`, and store both outputs as one host-managed document version. The
`.cadrs` package is the editable source for future sessions; STEP is for visualization and
production. The API returns each output independently and does not provide an atomic paired
snapshot or server transaction, so the host must coordinate uploads/versioning and avoid edits
between the two exports if exact revision matching is required.

The host is responsible for authenticating the user, loading source bytes, uploading both outputs,
and maintaining document identity and version associations. CADRS does not access host URLs,
server storage, or credentials. Integrators should configure the allowlist on a page they control
and use their own authenticated upload flow. The allowlist is an origin check for message routing,
not authentication: it is supplied through the embed URL, so any page that can embed the public
CADRS build can opt in its own origin. Do not use it as the sole protection for sensitive data; the
host must authenticate and authorize its own users and endpoints. Origins must be canonical HTTPS
origins; HTTP is accepted only for `localhost`, `127.0.0.1`, and `[::1]` development hosts.
