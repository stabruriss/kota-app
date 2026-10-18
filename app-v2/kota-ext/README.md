# Kota Local Pi extension and fixtures

Kota pins local Pi 1.3.0. Pi supplies its extension API and TypeBox through the host module mapping; no build or dependency installation is needed to load the TypeScript entry point. Standalone tests require Node >= 22.19.

Load explicitly with `-ne -e /absolute/path/to/kota-ext`. Do not use `pi install`, write user Pi settings, or change an existing agent's launch configuration. The package manifest loads only `index.ts`; test extensions never load in production.

## Runtime surface

`index.ts` registers `/kota-inject`, the private `/kota-auth` controller command, the three compaction hooks, the Auto String router (below), five Kota tools (`kota_agent_bus`, `kota_bbs`, `kota_ember`, `kota_bartender`, `kota_my_soul`), and Pi's native capabilities (below). The first four tools' labels, descriptions and wrapper errors exactly match approved `TOOLS-FINAL.md` (SHA-256 `7e422261e0029a9ff15a613c58505ad1a97e3a1dde2c2844b50246bbaa40b7b5`); `kota_my_soul` runs the `kota-my-soul` sidecar with the description from `TEXTS-FOR-REVIEW.md` §E (kota-my-soul); schemas have no field descriptions. Injection and tool transport follow the shared `CONTRACT-INJECT.md`, including BBS `agents` and the Bartender timeout exception.

The bridge validates the complete envelope, preserves its original content, adds `details.kotaId`, and calls `sendMessage` exactly once with `triggerTurn:true` / `deliverAs:"steer"`. It creates no success ACK, queue ledger, providers, system-prompt changes, UI, or settings mutations. Malformed JSON uses `kotaId=unknown`; controlled error payloads contain only fixed codes and a safely encoded ID. Asynchronous `sendMessage` errors remain Pi's responsibility.

Local Stop uses `/kota-hold-pause {"requestId":"..."}` → native `abort` → native `clear_queue` → `/kota-hold-resume {"requestId":"..."}`. Pause synchronously fences both flush hooks and idle injection. Resume requires idle, flushes the retained batch once in its original order, and does not start an empty turn. Both commands report `{schema:1,requestId,action,kotaIds}` through the private `kota.queue.v1:` UI notify codec; these receipts are not custom messages, model input, session entries, or room history. Rust requires both that receipt and the handled RPC ACK before advancing. Inputs already handed to Pi before pause may be cleared; native consumption always wins. Rust-held/new inputs wait until resume finishes. `test/stop-rpc.test.mjs` exercises six scenarios with the pinned bundled Pi, isolated directories and a loopback-only sandbox, including all-mode batching and compaction-abort consumption.


## Native Local codemode and MCP

Ordinary owners compose the public native codemode (`mode: 'on'`), tool_search and MCP extension factories. Codemode is active even without servers; ordinary tools remain available. MCP exposure and model-facing descriptions come from Pi itself. Auth/query helpers return before registering any of these capabilities.

All Local sessions use the launcher's owned `PI_CODING_AGENT_DIR`. Pi natively loads its shared `mcp.json` and `mcp-auth.json` when a session starts. Running connections are not reloaded when another agent or the UI changes that configuration. Only ordinary Local PATH prepends the pinned Pi directory, so agent bash can use native `pi mcp` management without a system installation.

`/kota-mcp {"schema":1,"requestId":"..."}` is a read-only, human-only status query. It calls the native `/mcp` command's synchronous public completion callback and emits a bounded `kota.mcp.v1:` notify receipt. It never calls `/mcp`'s handler, reconnects a server, or writes a message. Missing/changed callbacks and malformed formats produce unknown status; Rust consumes these receipts before workbench/Violet projection. Only fixed health/reason enums reach the frontend. This callback is not a stable Pi status API: `test/mcp-rpc.test.mjs` pins its actual bundled-runtime format and checks native codemode execution, MCP tool invocation, session/configuration separation, helper exclusion, private receipts and CLI management under an untouched model-auth lock. All servers, authorization and model responses are synthetic and sandboxed to loopback.

## Shadow clones

`kota_shadow_clone` is an ordinary-owner-only thin RPC tool with create, ask, status, stop and destroy. Its description and parameter copy follow the user's `PROMPTS-v3` verbatim. A private `kota.clone.v1:` native input request reaches Rust; the response returns immediately with clone IDs. Rust owns the independent processes, seven-running limit, per-run 90-minute default (zero means unlimited), native sessions under the parent's `under-table/clone-<id>/`, and the shared workbench projection.

Clones use the parent's current physical model and thinking, `-nc -ns -ne --offline`, and no Kota extension, MCP or codemode. Read-only clones have only native read, grep, find and ls tools. Offline suppresses startup's background OAuth refresh; a later expired-token request retains Pi's native refresh behavior. Finished processes exit while records remain; ask reopens the exact clone session. Human parent Stop and parent process exit stop all copies; the parent's natural completion does not. Destroy removes only that clone and its records, without retracting reports already queued.

Final reports are bounded to 20 KiB and enter the existing parent injection/ledger path as `kota.shadow_clone`. They are available to the parent model but hidden from Violet's room projection. The private control codec is neither a custom message nor a session/ledger entry. Names and generated portraits belong only to the UI.

## Auto String (M3)

`auto-route.ts` registers one virtual model, `kota/auto`, the `--kota-auto-route=<group JSON>` launch flag and `/kota-auto-route <group JSON>` update command. String gives each Local agent/Hero its own 2–6-model group, intents, default model and threshold; there are no named routes or presets. Rust validates and confirms changed physical configurations through Pi. Internal names (`autoRoute`, `auto-route`, `kota/auto`) remain unchanged.

The account's editable `heroes/system-puppeteer/puppeteer-prompts.json` contains all four Jev templates. Rust seeds/resets it through the existing system-prompt template manifest. `system_prompt_read` returns its path and effective content, plus an optional Puppeteer-only fallback warning; the compatible `puppeteer_jev_prompts` IPC still returns exactly four questions-only keys. The extension derives the same account path from its owned `providers/kota-local` root and is strictly read-only. Every new classification reads one validated snapshot; tools, physical fallback and Retry do not reread. Any invalid/unreadable section falls back to the whole bundled file without overwriting user edits. Instructions are nonblank, NUL-free and at most 512 Unicode characters; the JSON must be a regular UTF-8 file at most 1 MiB. `defaultIntents` is metadata, not a global Group setting. Editing the file changes the classifier's actual instructions/criteria, not the physical model's delivery envelope.

Each new Kota message (custom injection appears as a Pi `continuation`) classifies fresh user-role text after the latest reply, excluding compaction/branch summaries. First classification omits current_model and previous_reply; later classification carries the current selection and the latest final reply tail. Only probabilities strictly greater than the group threshold switch selection. Group edits apply on the next message without resetting selection; reentering `kota/auto` starts first-message classification again. Tool follow-ups and retries use the turn's group snapshot; auxiliary calls use the latest reply's physical model. Fallback tries the chosen model, default, then list order, once per physical provider/model.

Fixed route errors and numbered fallback bridges are tested against bundled Pi's `isRetryableAssistantError`. Pi's untouched default budget is three retries: at most four physical attempts, so six configured cards still permit only four attempts in a wholly failing default-budget turn. Six-attempt tests explicitly raise only the fake session's budget; production settings and Pi are not modified. Read-only owned settings inform the final exhausted copy.

The Jev key (`jev/keys.json`, written only by Rust) is a per-request option; authentication failure writes `jev/rejected-<provider>.json` for that key ID only. Route facts travel over the private `kota.route.v1:` notify codec, never as model input or room history. Dance uses the same keys/rejection markers but a direct pooled Rust classifier client, not a Pi helper or model turn.

`jev.ts` registers the helper-only `/kota-jev-verify` save-time check. Rust answers its private `kota.jev.v1:` key prompt; the fixed input carries no user content.

CLI bodies use stdin, not a shell string. Arguments and working context cannot select arbitrary binaries, cwd, identity or environment. Captures are limited to 1 MiB per stream. Ordinary actions have a 120-second deadline; Bartender sync has 1,815 seconds because its existing CLI waits up to 30 minutes. Cancel/timeout cannot promise that already-dispatched side effects were undone. An omitted bus intent is omitted from argv and keeps the existing CLI default.

## Private authentication channel

`auth.ts` / `auth-protocol.ts` use the public Pi `ModelRuntime` and existing RPC UI carriers, with `kota.auth.v1:` whitelist envelopes. Commands are strict root objects (`login/status/logout/cancel/refresh/catalog/resolve`); no credential, storage path, environment or binary overrides are accepted. Key replies remain ephemeral native dialog responses. Rust owns pre-forward key syntax checks, helper/epoch/prompt binding, provider flock, fresh canonical authentication and activation gates.

Only the launcher-marked `KOTA_LOCAL_AUTH_HELPER=1` process may authenticate or query; Rust supplies its stable `KOTA_LOCAL_INSTALLATION_UUID`. Normal owners allow only asynchronous idle `/kota-auth refresh`, never an await in injection flush hooks. All login operations use a dedicated native-only `modelsPath:null` helper runtime. Only its target provider's native login method is cloned to record a credential-return boolean, never the value or a fingerprint. Azure alone additionally adds Resource URL before Pi's single credential mutation; catalog/resolve and agents retain their owned model overlays. Failure before native credential return is not-written; afterwards it is unknown unless Pi confirms success or raises CredentialSynchronizationError. Logout after invocation remains conservatively unknown on ordinary failure. Authentication never registers tools, sends model messages or appends session entries. Production deadlines are 10 minutes for login, 30 seconds for control; Rust gives helper cancellation five seconds to exit. Committed or uncertain failures do not delete or replay credentials.

## Tests

From this directory:

```sh
export KOTA_LOCAL_TEST_PI_RUNTIME_ROOT="$PWD/../src-tauri/pi-runtime/darwin-arm64"
npm test
npm run test:provider
npm run test:rpc
```

Run `npm ci` in `app-v2`, then prepare that staging tree with `npm run prepare:pi-runtime` there. Native RPC cases validate the checked-in vendor manifest and run its absolute `pi` entrypoint, including inside the auth sandbox. Missing or mismatched staging fails; there is no system Pi fallback. Node-only SDK witnesses use the exact development dependencies in `app-v2`, check their versions against the vendor pin and do not count as bundled-runtime evidence. They never use a global Pi installation.

No real provider, credentials, BBS publication, bus delivery, Ember schedule, Bartender sync, or Kota app is used by these tests. The real Pi subprocess talks only to a loopback API fixture. Its `HOME`, `PI_CODING_AGENT_DIR`, session directory and cwd are created under ignored `test/.tmp/` and removed afterward. It inherits neither provider credentials/proxies nor the real project's `KOTA_*` environment. Test-only `--offline --no-context-files` prevents catalog traffic and ancestor instructions. This isolates test configuration, not operating-system permissions.

The real RPC cases cover seven sources, structured images, same-run steering, unknown/encoded controlled errors, compaction success/cancellation, response abort, and the Clear boundary where a drained message is consumed after compaction despite `clear_queue`. That fixture expects `clear_requested` until consumption/settlement; it does not claim to test the Rust ledger itself. `test/tools.test.mjs` additionally verifies exact approved registration text, actual tool execution against four mock CLIs, raw stdin/results, cross-field rejection, nonzero exits, cancellation, both output limits, and a cold read-only fifteen-resource directory load with unchanged content/metadata through process shutdown.

`test/auth.test.mjs` runs its synthetic authentication controller suite in a sandboxed child with temporary HOME/agent-dir, blocked personal Pi/worktree writes/network, and a simulated-user sentinel. Its assertions and pure codec tests do not establish bundled runtime, Rust/WebView, cross-App locks, or real OAuth/provider acceptance.

## Fake provider CLI

From the repository root:

```sh
node app-v2/kota-ext/test/fake-provider/server.mjs --port 0 \
  --script app-v2/kota-ext/test/fake-provider/scripts/text.json
```

The first stdout line is:

```json
{"type":"ready","baseUrl":"http://127.0.0.1:<port>/v1","provider":"kota-fake","model":"fixture"}
```

It serves OpenAI-compatible stream SSE and non-stream `POST /v1/chat/completions`. It never forwards requests. Scripts are `{ "steps": [...] }` (or a step array), consumed in request order; exhaustion returns an explicit error. Example scripts also cover harmless `read` and long replies.

Step fields:

- `type`: `text`, `long`, `tool`, or `error`.
- Text/long: `text`, optional `repeat`, `chunkSize`, `chunkDelayMs`, `thinking`.
- Tool: `name` (`read`, `bash`, `codemode`, or one of the four Kota tools), `arguments`, optional `chunkSize`. Use only harmless fixture operations; Kota tool tests supply mock CLIs on the child-only PATH.
- Error: `status` (400–599) and `message`.
- Barriers: `holdBeforeHeaders:true` or `holdAfterChunks:N` waits for explicit release, not a guessed sleep.

`GET /__test/state` returns only request sequence/type/completion/abort metadata. `POST /__test/release/<sequence>` releases a barrier. Both are loopback-only. Node tests can import `createFakeProvider`, `waitForRequest`, `release`, and `createTestSession` directly.

Run the isolated smoke client against the reported base URL:

```sh
node app-v2/kota-ext/test/fake-provider/rpc-client.mjs \
  --base-url http://127.0.0.1:<port>/v1
```

The client continuously splits stdout on LF only, preserves raw lines, correlates responses by request ID, bounds records, and closes/kills its own child on cleanup.

## R3 recording

```sh
node app-v2/kota-ext/test/fake-provider/record-fixtures.mjs --out <scratch-artifact-directory>
```

Produces `full.json`, `since.json`, `events.jsonl`, and `RECEIPT.json` with SHA-256 hashes. It asserts real thinking/model/thinking-level/compaction/error/retry/extension-error coverage against the fake API. Only known temporary paths and the ephemeral loopback port are replaced; IDs, timestamps and entry contents otherwise stay unchanged. Stream deltas and command responses are omitted from the event artifact. The extra fault extension is loaded only for this recorder, not by `kota-ext`.
