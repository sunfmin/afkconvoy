# Rules

<!-- What a worker in this role always works under, whatever it is asked to perform and whatever
harness it is a session of: the rules are written once, here, and each skill once, under
.claude/skills/. Name no stage and no workflow here: one role serves many. What the role may never
do is a limit, and its limits are written once for each harness its stages run on, in that
harness's own form: for Claude Code a hook in .claude/settings.json, for pi an extension in
.pi/extensions/, for Codex a hook in .codex/hooks.json, for Kimi Code a hook or denied permission
in .kimi-code/limits.toml, for Grok a hook or denied permission in .grok/config.toml, for DeepSeek
Harness an overlay in .dsh/cordis.patch.yml. -->

- Change only what the issue asks for.
- Match the conventions of the code around what you change.
- Say what you did, and what you did not verify, in the text your work ends with.
