# Recorded samples

What each engine wrote, read by `../standalone.rs` through the crate's public
API (sunfmin/control#729). Paths, session ids and message ids are replaced with
neutral ones; every other value is as the engine wrote it or as its source
spells it.

- `claude-code-status-line.json`: the JSON Claude Code 2.1.322 handed its
  status line command on stdin after one answered prompt, recorded on this
  host. Its `rate_limits` hold the five-hour and seven-day windows.
- `claude-code-stream.jsonl`: Claude Code's `--output-format stream-json`.
  The first line was recorded on this host from Claude Code 2.1.322; it is the
  `rate_limit_event` an allowed request writes, with every window in
  `unifiedWindows`. The rejected `rate_limit_event`, the synthetic assistant
  message and the `result` that follow are a weekly limit as reported on
  anthropics/claude-code#103820, with the parts elided there left out.
- `claude-code-session-rate-limit.jsonl`: Claude Code's session file. No
  session on this host met a limit. The first line is the synthetic assistant
  message Claude Code 2.1.322 writes on a usage limit, with the fields its
  binary writes (`error`, `isApiErrorMessage`, `apiError`, `apiErrorParams`,
  `quotaLimits`) and the values of anthropics/claude-code#103820. The second is
  the server's own throttle, as quoted on anthropics/claude-code#64177
  ("not your usage limit"), which is no usage limit.
- `codex-session.jsonl`: a Codex rollout. No Codex runs on this host: the
  lines take the `TokenCountEvent` / `RateLimitSnapshot` fields of
  codex-rs/protocol and values users reported from their rollouts and 429s
  (openai/codex#23171, #37455, #49214, #58413). The second `token_count` is an
  older Codex's, which said `resets_in_seconds` where a newer one says
  `resets_at`.
- `kimi-usages.json`: Kimi Code keeps no window in its session; its `/usage`
  asks `<base url>/usages` (`https://api.kimi.com/coding/v1/usages`) with the
  account's token. The sample takes the fields Kimi Code 2.5.3 parses
  (`parseQuotaUsages`): no Kimi account is on this host to record one.

The steps samples (sunfmin/control#747), read by `Harness::steps`:

- `claude-code-session-steps.jsonl`: eight lines of a real Claude Code
  2.1.322 session file recorded on this host, of an afkconvoy worker on this
  repo: three answered `Bash` calls, the third of which Claude Code marked
  `is_error` (exit code 1), the `api-request-blob` that repeats a call beside
  them, and a last call with no result yet. Paths and the session id are
  replaced.
- `codex-session-steps.jsonl`: no Codex runs on this host. The lines take the
  `ResponseItem` fields codex-rs writes (openai/codex `64acaba9`,
  `protocol/src/models.rs`): `function_call` with its arguments as a string
  of JSON, `custom_tool_call`, and their outputs as plain strings, whose
  `success` codex-rs never writes. The output texts are `exec_command`'s
  ("Process exited with code") and the older shell's ("Exit code:").
- `kimi-wire-steps.jsonl`: no Kimi account is on this host. The
  `context.append_loop_event` lines take the `tool.call` / `tool.result`
  fields of `@moonshot-ai/kimi-code` 2.5.3 (`wire-manifest.d.ts`,
  `contextEvents.ts`), and its shell's failure text; a sub-agent's lines
  carry its own `agentId`.
- `pi-session-steps.jsonl`: the entries of `@mariozechner/pi-coding-agent`
  0.73.1's `docs/session-format.md`: `toolCall` parts of an assistant
  message and `toolResult` messages, with the failure text its bash tool
  writes.
- `grok-updates-steps.jsonl`: Grok's `updates.jsonl` is its ACP session
  update stream. The lines take the `session/update` envelope and the
  `tool_call` / `tool_call_update` fields of Orca's recordings of Grok
  1.0.77's protocol traffic; how Grok wraps a line on disk was read only from
  its binary's strings and is not confirmed.
- `dsh-session-steps.jsonl`: the decompressed text of a DeepSeek Harness
  `session.v4.jsonl.zstd`, with the header, `tool/call` and `tool/result`
  fields of `@deepseek-ai/dsh-session` 0.2.2-rc.1 and the exit-code line its
  bash tool ends with (it marks no non-zero exit as an error).
