# Pi 1.1 session compatibility fixture

`session.jsonl` was written by the bundled Pi 1.1.0 executable, with Kota's
extension and isolated loopback model/Jev servers plus a local stdio MCP server.
It contains only synthetic inputs and answers. `SOURCE.json` pins the old
runtime archive, source commit and transcript checksum.

The first input selects model A and runs codemode to call an MCP tool, list its
resources and read one. The second selects model B and receives a synthetic
non-retryable provider error. This leaves a persisted B selection after the
last successful A reply, so reopening cannot pass merely by choosing the last
successful model.

Only generated filesystem paths were normalized. The upgrade test binds the
session header's cwd to its temporary workspace, then checks every old entry,
retries the old input without another Jev request, sends a new input, and uses
MCP again. It also checks that the original transcript is an unchanged prefix
of the same file. No user session, credential, or model API is involved.
