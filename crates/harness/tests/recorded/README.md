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
