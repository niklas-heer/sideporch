+++
schema_version = 1
id = "01M3P13Y38NXSD86A34VFZRK9M"
title = "Keep search in SQLite FTS5, with filters, recency ranking and spelling correction"
date = "2026-09-29"
status = "accepted"
tags = ["search", "storage"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Search stays in SQLite FTS5, inside the database, and gets better around it:

- The index is rebuilt (a `Migration::Code` step) with prefix indexes (`prefix = '2 3'`), since every word is matched as a prefix, and an `fts5vocab` view of its words. Poll options are indexed too.
- The search box understands `"phrases"`, `OR`, `-excluded` words and the filters `from:`, `in:` (channels and `@person` conversations), `has:` (file, image, link, poll, gif, reaction), `is:` (pinned, saved, thread), `mentions:me` and `before:`/`after:`/`on:` (days or months, in the searcher's time zone). Filters work without words.
- Results are ranked by BM25 divided by `1 + age / 90 days`, so recent matches rise; "Newest" sorts by time. Pages hold 30 results.
- When words match nothing, each unknown word is replaced by the most frequent indexed word within one edit (two for words over five letters, Damerau–Levenshtein) and the corrected search is shown, with a note.
- People and channels whose names match are listed above messages, and the sidebar's search box shows channels, people and messages while typing (`/search/suggest`).

## Context

On 2026-09-29 Niklas asked for search as good as possible, locally, possibly by embedding a Rust search library.

Tantivy was the main alternative: fuzzy term queries, many stemmers and fast BM25. It would add a second store beside SQLite that has to be kept in step with every write, edit and deletion, included in backups and upgrades, and rebuilt after restores; and several megabytes and build time to a binary meant to stay small. FTS5 already gives BM25, prefix queries, phrase queries and snippets inside the same transactions as the messages. Typo tolerance, the main thing FTS5 lacks, is covered by corrections from its own vocabulary.

Stemming (FTS5's `porter` tokenizer) was left out: it only knows English, would make the vocabulary unreadable for suggestions, and prefix matching already finds most word forms.

## Consequences

- The upgrade rebuilds the index once, which takes a while on large databases.
- Correction scans the vocabulary only when a search finds nothing; on very large instances that scan could take tens of milliseconds.
- Words are matched as prefixes, not stems, so irregular forms ("ran" for "run") don't match.
- Revisit Tantivy if people need typo tolerance within results that do match, or language-aware stemming.
