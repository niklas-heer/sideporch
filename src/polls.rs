//! Polls: what they ask, and how ranked ones are counted.
//!
//! A poll is a message whose text is the question. `messages.poll` holds a
//! [`Spec`] as JSON; polls from before there were kinds hold a plain array
//! of options. Votes for single-choice polls live in `poll_votes`, the
//! choices of the other kinds in `poll_marks`.

use serde::{Deserialize, Serialize};

pub const MAX_OPTIONS: usize = 10;
pub const MAX_OPTION_CHARS: usize = 100;
pub const MAX_QUESTION_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Everyone picks one option.
    #[default]
    Single,
    /// Everyone picks every option that works for them.
    Multiple,
    /// Everyone ranks the options; the winner is found by instant runoff.
    Ranked,
}

impl Kind {
    /// Reads a kind's key, or a word that starts `/poll` to pick one. Only
    /// these lowercase words count, so a question can start with "Rank".
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "single" => Some(Self::Single),
            "multiple" | "multi" => Some(Self::Multiple),
            "ranked" => Some(Self::Ranked),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Multiple => "multiple",
            Self::Ranked => "ranked",
        }
    }
}

/// What a poll asks, as stored with its message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    #[serde(default)]
    pub kind: Kind,
    pub options: Vec<String>,
    /// When voting ended, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<i64>,
}

impl Spec {
    /// Reads a stored poll, in either the current or the original format.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str::<Self>(json).ok().or_else(|| {
            serde_json::from_str::<Vec<String>>(json)
                .ok()
                .map(|options| Self {
                    options,
                    ..Self::default()
                })
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Checks a new poll's question and options, trimming the options.
    pub fn new(kind: Kind, question: &str, options: Vec<String>) -> Result<Self, &'static str> {
        let options: Vec<String> = options
            .into_iter()
            .map(|option| option.trim().chars().take(MAX_OPTION_CHARS).collect())
            .filter(|option: &String| !option.is_empty())
            .collect();
        if question.trim().is_empty() || question.chars().count() > MAX_QUESTION_CHARS {
            return Err("Ask a question of at most 300 characters.");
        }
        if !(2..=MAX_OPTIONS).contains(&options.len()) {
            return Err("Give a poll 2 to 10 options.");
        }
        Ok(Self {
            kind,
            options,
            closed_at: None,
        })
    }
}

/// Reads `/poll [ranked|multiple] Question? | One | Two` or the same with
/// quoted parts: `/poll "Question?" "One" "Two"`.
pub fn parse_command(text: &str) -> Option<(String, Spec)> {
    let text = text.trim();
    let (kind, rest) = match text.split_once(char::is_whitespace) {
        Some((word, rest)) => Kind::parse(word).map_or((Kind::Single, text), |kind| (kind, rest)),
        None => (Kind::Single, text),
    };
    let parts: Vec<String> = if rest.contains('|') {
        rest.split('|').map(|part| part.trim().to_owned()).collect()
    } else {
        rest.split(['"', '“', '”'])
            .skip(1)
            .step_by(2)
            .map(|part| part.trim().to_owned())
            .collect()
    };
    let mut parts = parts.into_iter().filter(|part| !part.is_empty());
    let question = parts.next()?;
    let spec = Spec::new(kind, &question, parts.collect()).ok()?;
    Some((question, spec))
}

/// One round of an instant-runoff count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    /// Votes per option; `None` for options already out.
    pub counts: Vec<Option<usize>>,
    /// Options that went out after this round.
    pub eliminated: Vec<usize>,
    /// Ballots that rank no option still in the count.
    pub exhausted: usize,
}

/// The result of counting ranked ballots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub rounds: Vec<Round>,
    /// The winner, or every option still tied at the end. Empty without
    /// ballots.
    pub winners: Vec<usize>,
}

/// Counts ranked ballots by instant runoff. Each ballot lists option
/// indexes, favorite first, and counts for its highest-ranked option still
/// in the count. An option with more than half of those votes wins;
/// otherwise the option with the fewest votes goes out and its ballots move
/// on to their next choice. Ties for the fewest are broken by the earlier
/// rounds, latest first; options tied through every round go out together,
/// unless that would leave none, in which case they share the win.
pub fn instant_runoff(options: usize, ballots: &[Vec<usize>]) -> Outcome {
    let mut active = vec![true; options];
    let mut rounds: Vec<Round> = Vec::new();
    if ballots.iter().all(Vec::is_empty) {
        return Outcome::default();
    }
    loop {
        let mut counts: Vec<Option<usize>> = active
            .iter()
            .map(|&on| if on { Some(0) } else { None })
            .collect();
        let mut exhausted = 0_usize;
        for ballot in ballots {
            let choice = ballot
                .iter()
                .copied()
                .find(|&option| active.get(option).copied().unwrap_or(false));
            match choice.and_then(|option| counts.get_mut(option)) {
                Some(Some(count)) => *count = count.saturating_add(1),
                _ => exhausted = exhausted.saturating_add(1),
            }
        }
        let continuing: Vec<(usize, usize)> = counts
            .iter()
            .enumerate()
            .filter_map(|(option, count)| count.map(|count| (option, count)))
            .collect();
        let valid: usize = continuing.iter().map(|(_, count)| count).sum();
        let best = continuing
            .iter()
            .map(|(_, count)| *count)
            .max()
            .unwrap_or(0);
        let leaders: Vec<usize> = continuing
            .iter()
            .filter(|(_, count)| *count == best)
            .map(|(option, _)| *option)
            .collect();
        let majority = best.saturating_mul(2) > valid;
        if continuing.len() <= 1 || (majority && leaders.len() == 1) {
            rounds.push(Round {
                counts,
                eliminated: Vec::new(),
                exhausted,
            });
            return Outcome {
                rounds,
                winners: leaders,
            };
        }
        let fewest = continuing
            .iter()
            .map(|(_, count)| *count)
            .min()
            .unwrap_or(0);
        let mut lowest: Vec<usize> = continuing
            .iter()
            .filter(|(_, count)| *count == fewest)
            .map(|(option, _)| *option)
            .collect();
        // Look back through earlier rounds for a difference.
        for earlier in rounds.iter().rev() {
            if lowest.len() <= 1 {
                break;
            }
            let count_of = |option: usize| earlier.counts.get(option).copied().flatten();
            let least = lowest.iter().filter_map(|&option| count_of(option)).min();
            if let Some(least) = least {
                lowest.retain(|&option| count_of(option) == Some(least));
            }
        }
        if lowest.len() == continuing.len() {
            rounds.push(Round {
                counts,
                eliminated: Vec::new(),
                exhausted,
            });
            return Outcome {
                rounds,
                winners: lowest,
            };
        }
        for &option in &lowest {
            if let Some(on) = active.get_mut(option) {
                *on = false;
            }
        }
        rounds.push(Round {
            counts,
            eliminated: lowest,
            exhausted,
        });
    }
}

/// Reads a ranking from a form: option index to rank (1 is the favorite).
/// Unranked options are left out; equal ranks keep the options' order.
pub fn ranking_from_ranks(options: usize, ranks: &[(usize, u32)]) -> Vec<usize> {
    let mut ranked: Vec<(u32, usize)> = ranks
        .iter()
        .filter(|(option, rank)| *option < options && *rank > 0)
        .map(|&(option, rank)| (rank, option))
        .collect();
    ranked.sort_unstable();
    let mut ranking: Vec<usize> = Vec::with_capacity(ranked.len());
    for (_, option) in ranked {
        if !ranking.contains(&option) {
            ranking.push(option);
        }
    }
    ranking
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ballots(list: &[&[usize]]) -> Vec<Vec<usize>> {
        list.iter().map(|ballot| ballot.to_vec()).collect()
    }

    #[test]
    fn a_majority_of_first_choices_wins_at_once() {
        let outcome = instant_runoff(3, &ballots(&[&[0, 1], &[0], &[1, 0]]));
        assert_eq!(outcome.winners, vec![0]);
        assert_eq!(outcome.rounds.len(), 1);
        assert_eq!(outcome.rounds[0].counts, vec![Some(2), Some(1), Some(0)]);
    }

    #[test]
    fn votes_move_to_the_next_choice() {
        // Pizza 2, Tacos 2, Soup 1: Soup goes out, and its voter liked Tacos next.
        let outcome = instant_runoff(3, &ballots(&[&[0, 2], &[0], &[1, 0], &[1], &[2, 1, 0]]));
        assert_eq!(outcome.winners, vec![1]);
        assert_eq!(outcome.rounds.len(), 2);
        assert_eq!(outcome.rounds[0].eliminated, vec![2]);
        assert_eq!(outcome.rounds[1].counts, vec![Some(2), Some(3), None]);
    }

    #[test]
    fn a_broadly_liked_option_beats_a_polarizing_one() {
        // A leads the first choices, but more people prefer B to A: once
        // C goes out, its fans' votes move to B, which wins 5 to 4.
        let outcome = instant_runoff(
            3,
            &ballots(&[
                &[0, 2],
                &[0, 2],
                &[0, 2],
                &[0, 2],
                &[1, 2],
                &[1, 2],
                &[1, 2],
                &[2, 1],
                &[2, 1],
            ]),
        );
        assert_eq!(outcome.winners, vec![1]);
    }

    #[test]
    fn exhausted_ballots_drop_out_of_the_majority() {
        let outcome = instant_runoff(3, &ballots(&[&[0], &[0], &[1], &[2], &[2, 0]]));
        // Round 1: A 2, B 1, C 2 → B out, its ballot is exhausted.
        // Round 2: A 2, C 2 of 4 → tie, broken by round 1: equal → shared.
        assert_eq!(outcome.rounds[1].exhausted, 1);
        assert_eq!(outcome.winners, vec![0, 2]);
    }

    #[test]
    fn ties_for_last_look_at_earlier_rounds() {
        // Round 1: A 3, B 2, C 2, D 1 → D out, moving to C.
        // Round 2: A 3, B 2, C 3 → B has the fewest.
        let outcome = instant_runoff(
            4,
            &ballots(&[&[0], &[0], &[0], &[1], &[1], &[2], &[2], &[3, 2]]),
        );
        assert_eq!(outcome.rounds[0].eliminated, vec![3]);
        assert_eq!(outcome.rounds[1].eliminated, vec![1]);
        // Round 3: A 3, C 3, but A led C in round 1, so C goes out.
        assert_eq!(outcome.rounds[2].eliminated, vec![2]);
        assert_eq!(outcome.winners, vec![0]);

        // B and C tie for last in round 2 but C had more in round 1.
        let outcome = instant_runoff(
            4,
            &ballots(&[
                &[0],
                &[0],
                &[0],
                &[0],
                &[1],
                &[1],
                &[3, 1],
                &[2],
                &[2],
                &[2],
            ]),
        );
        // Round 1: A 4, B 2, C 3, D 1 → D out to B. Round 2: A 4, B 3, C 3.
        assert_eq!(outcome.rounds[1].eliminated, vec![1]);
    }

    #[test]
    fn no_ballots_no_winner() {
        assert_eq!(instant_runoff(3, &[]), Outcome::default());
        assert_eq!(instant_runoff(3, &ballots(&[&[]])), Outcome::default());
    }

    #[test]
    fn rankings_from_forms() {
        assert_eq!(ranking_from_ranks(3, &[(2, 1), (0, 2)]), vec![2, 0]);
        assert_eq!(
            ranking_from_ranks(3, &[(1, 1), (0, 1), (9, 2), (2, 0)]),
            vec![0, 1]
        );
    }

    #[test]
    fn commands_pick_the_kind() {
        let (question, spec) = parse_command("ranked Where do we eat? | Pizza | Tacos").unwrap();
        assert_eq!(question, "Where do we eat?");
        assert_eq!(spec.kind, Kind::Ranked);
        assert_eq!(spec.options, vec!["Pizza", "Tacos"]);
        let (question, spec) = parse_command("Rank these? | A | B").unwrap();
        assert_eq!(
            (question.as_str(), spec.kind),
            ("Rank these?", Kind::Single)
        );
        let (_, spec) = parse_command(r#"multiple "Which days?" "Mon" "Tue""#).unwrap();
        assert_eq!(spec.kind, Kind::Multiple);
        assert!(parse_command("just one").is_none());
    }

    #[test]
    fn stored_polls_in_both_formats() {
        let old = Spec::from_json(r#"["A","B"]"#).unwrap();
        assert_eq!(old.kind, Kind::Single);
        let spec = Spec::new(Kind::Ranked, "Q?", vec!["A".into(), " B ".into()]).unwrap();
        assert_eq!(Spec::from_json(&spec.to_json()), Some(spec));
    }
}
