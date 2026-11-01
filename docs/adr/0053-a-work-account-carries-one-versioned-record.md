---
status: accepted
---

# A work account carries one versioned record

Amends ADR-0044 for sunfmin/control#426. The one work account comment carries its facts as one versioned JSON object; its human text is rendered from that object. Reading statistics from prose made changes of wording change the measurements and left some facts unavailable to other systems.

The object sits in a collapsed details block with a fenced JSON block. HTML and fence characters in values are escaped in the JSON, so a command containing `--`, HTML or a fence remains data. An HTML comment holds only the account's identity marker: it cannot safely carry arbitrary commands. Start retention, overlapping durations, exclusion of landing workers and retention of earlier red gate runs remain as in ADR-0044.

The same record supplies the summary and `accounts --rows`, which answers NDJSON with the issue URL. Unknown facts are null. Legacy conversion preserves existing summary measures and leaves facts absent from machine-readable evidence null; it rewrites no comment. Unknown schema versions are unreadable rather than guessed from the rendered text. The help of `accounts` defines the schema. A field's meaning never changes within a version; a breaking change raises it. Tokens, watch rounds and bulk rewriting of old accounts belong to later work.
