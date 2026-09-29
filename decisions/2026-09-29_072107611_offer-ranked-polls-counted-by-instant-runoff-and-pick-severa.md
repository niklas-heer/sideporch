+++
schema_version = 1
id = "01M3P0KT8V9X35800T693AVCKJ"
title = "Offer ranked polls counted by instant runoff, and pick-several polls"
date = "2026-09-29"
status = "accepted"
tags = ["product", "polls"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Polls come in three kinds: pick one, pick several, and ranked. Ranked polls are counted by instant runoff: each ballot counts for its highest-ranked option still in the count; an option with more than half of those votes wins, otherwise the option with the fewest goes out and the count repeats. Ties for the fewest are broken by the earlier rounds, latest first; options tied through every round go out together, unless none would remain, and then they share the win. The page shows every round.

`messages.poll` now holds `{"kind", "options", "closed_at"}`; polls stored before as a plain array of options still read as pick-one polls. Pick-one votes stay in `poll_votes`; the other kinds use `poll_marks` (rank 0 for pick-several, 1 = favorite for ranked). The author or an admin can end a poll.

## Context

On 2026-09-29 Niklas asked for a ranked poll for choices like where a team eats: people rank their choices, and the winner is found by eliminating the last option and redistributing its votes, which is instant runoff.

Alternatives: Borda count or Condorcet methods also use rankings, but instant runoff is what Niklas described and is easy to explain round by round. Approval voting ("pick several") was added alongside because it answers "which day works for everyone" better than either.

## Consequences

- Ballots are visible to other members, like pick-one votes are.
- Counting happens when a poll is rendered; with at most 10 options and a team's worth of ballots this is cheap.
- Backward tie-breaking can decide a final two-way tie by an earlier round; the page shows the rounds so people can follow why.
