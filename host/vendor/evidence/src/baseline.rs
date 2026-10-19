//! A change's vendor evidence against the index `main` published: the
//! claimed vendor roots it lost or whose comparisons it narrowed, and a
//! chip's reviewed acceptances of them.
//!
//! A root is lost when the base index claims it and no shard of the new
//! index does (a claim exists only with a MATCH). It is narrowed when both
//! claim it and the comparisons reach fewer of its vendor closure's blocks
//! or branch directions than the base's did. Moving a root to another
//! production entry or scenario changes neither.

use crate::Result;
use oer_vendor_evidence_shard::store;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A vendor root an index claims: its archive or ROM `source` and `symbol`,
/// whatever scenario and production entry compare it.
pub type Root = (String, String);

/// How far a claimed root's comparisons reach into its vendor closure: the
/// most reached blocks and branch directions of any of its entries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Reach {
    pub blocks: u64,
    pub directions: u64,
}

impl Reach {
    /// Whether `self` reaches at least as far as `other` in both measures.
    pub fn covers(self, other: Reach) -> bool {
        self.blocks >= other.blocks && self.directions >= other.directions
    }
}

/// Every claimed root of an index with its reach; `None` for a root whose
/// producers measure no coverage (a host stand compiling vendor source).
pub type Claims = BTreeMap<Root, Option<Reach>>;

/// An index's claims, as [`claims`] reads them.
#[derive(Debug, Default)]
pub struct Read {
    pub claims: Claims,
    /// Roots one of whose entries records coverage in a form this format
    /// does not read: their reach is unknown, so only their loss compares.
    pub unread_coverage: BTreeSet<Root>,
}

/// The claims of every shard of the index `directory`, read from each
/// entry's `source`, `symbol` and reached coverage only: an index an earlier
/// format wrote still names them, so a change of the shard format stays
/// comparable. Coverage of another form leaves its root's reach unknown
/// ([`Read::unread_coverage`]). A file without the roots makes the
/// comparison impossible rather than an index that claims nothing.
pub fn claims(directory: &Path) -> Result<Read> {
    #[derive(serde::Deserialize)]
    struct Shard {
        entries: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        source: String,
        symbol: String,
        coverage: Option<serde_json::Value>,
    }
    #[derive(serde::Deserialize)]
    struct Coverage {
        blocks: Count,
        directions: Count,
    }
    #[derive(serde::Deserialize)]
    struct Count {
        reached: u64,
    }
    let mut read = Read::default();
    for name in store::names(directory)? {
        let path = store::path(directory, &name);
        let shard: Shard = serde_json::from_slice(&std::fs::read(&path)?).map_err(|error| {
            format!(
                "{}: names no claimed vendor roots ({error}); the indexes cannot be compared",
                path.display()
            )
        })?;
        for entry in shard.entries {
            let root = (entry.source, entry.symbol);
            let reach = match entry.coverage.map(serde_json::from_value::<Coverage>) {
                None => None,
                Some(Ok(coverage)) => Some(Reach {
                    blocks: coverage.blocks.reached,
                    directions: coverage.directions.reached,
                }),
                Some(Err(_)) => {
                    read.unread_coverage.insert(root.clone());
                    None
                }
            };
            let claim = read.claims.entry(root).or_default();
            *claim = match (*claim, reach) {
                (Some(a), Some(b)) => Some(Reach {
                    blocks: a.blocks.max(b.blocks),
                    directions: a.directions.max(b.directions),
                }),
                (a, b) => a.or(b),
            };
        }
    }
    // A root's best reach is unknown when any of its entries' is.
    for root in &read.unread_coverage {
        read.claims.insert(root.clone(), None);
    }
    Ok(read)
}

impl Read {
    /// The claims of an index the current format wrote, as the change's own
    /// is: coverage it does not read is an error, never an unknown reach
    /// that would read as a narrowing to nothing.
    pub fn current(self) -> Result<Claims> {
        match self.unread_coverage.first() {
            Some((source, symbol)) => Err(format!(
                "the index records the coverage of vendor root {source}::{symbol} in a form `baseline::claims` does not read; update it with the shard format"
            )
            .into()),
            None => Ok(self.claims),
        }
    }
}

/// A chip's reviewed acceptances of what its index lost on purpose
/// (`verification/<chip>/decisions/accepted-losses.toml`), each with its
/// reason: retired roots it no longer claims, and narrowed roots with the
/// reach their comparisons keep, so a later narrowing fails again.
#[derive(Debug, Default)]
pub struct Accepted {
    pub path: String,
    pub retired: BTreeSet<Root>,
    pub narrowed: BTreeMap<Root, Reach>,
}

impl Accepted {
    /// The acceptances of `chip`'s project below `root`; none without the file.
    pub fn load(root: &Path, chip: &str) -> Result<Self> {
        let path = oer_vendor_artifacts::project::Project::new(root, chip)?.accepted_losses();
        match std::fs::read_to_string(root.join(&path)) {
            Ok(text) => Self::parse(path, &text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                path,
                ..Self::default()
            }),
            Err(error) => Err(error.into()),
        }
    }

    pub fn parse(path: String, text: &str) -> Result<Self> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            #[serde(default)]
            retired: Vec<Retired>,
            #[serde(default)]
            narrowed: Vec<Narrowed>,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Retired {
            source: String,
            symbol: String,
            reason: String,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Narrowed {
            source: String,
            symbol: String,
            blocks: u64,
            directions: u64,
            reason: String,
        }
        let file: File = toml::from_str(text).map_err(|e| format!("{path}: {e}"))?;
        let reasons = file
            .retired
            .iter()
            .map(|r| &r.reason)
            .chain(file.narrowed.iter().map(|n| &n.reason));
        if reasons.into_iter().any(|reason| reason.trim().is_empty()) {
            return Err(format!("{path}: every acceptance gives its reason").into());
        }
        let retired = file
            .retired
            .into_iter()
            .map(|r| (r.source, r.symbol))
            .collect();
        let mut narrowed = BTreeMap::new();
        for n in file.narrowed {
            let reach = Reach {
                blocks: n.blocks,
                directions: n.directions,
            };
            if narrowed.insert((n.source, n.symbol), reach).is_some() {
                return Err(format!("{path}: a root is accepted as narrowed twice").into());
            }
        }
        Ok(Self {
            path,
            retired,
            narrowed,
        })
    }

    /// An error when an acceptance contradicts the index `claims`: a retired
    /// root it still claims, or a narrowed one it does not claim.
    pub fn check(&self, chip: &str, claims: &Claims) -> Result<()> {
        if let Some((source, symbol)) = self.retired.iter().find(|r| claims.contains_key(*r)) {
            return Err(format!(
                "{chip}: the retired vendor root {source}::{symbol} is still claimed; remove its retirement from {}",
                self.path
            )
            .into());
        }
        if let Some((source, symbol)) = self.narrowed.keys().find(|r| !claims.contains_key(*r)) {
            return Err(format!(
                "{chip}: the narrowed vendor root {source}::{symbol} is not claimed; retire it instead in {}",
                self.path
            )
            .into());
        }
        // An acceptance the comparisons outgrew would let them narrow back to
        // it unnoticed.
        let outgrown = self.narrowed.iter().find(|(root, accepted)| {
            let now = claims[*root].unwrap_or_default();
            now.blocks > accepted.blocks || now.directions > accepted.directions
        });
        if let Some(((source, symbol), _)) = outgrown {
            return Err(format!(
                "{chip}: the comparisons of vendor root {source}::{symbol} reach beyond its accepted narrowing; remove or update it in {}",
                self.path
            )
            .into());
        }
        Ok(())
    }
}

/// What a change loses against the base index.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Losses {
    /// Roots the base claims and the new index does not.
    pub lost: Vec<Root>,
    /// Roots both claim whose comparisons reach less than the base's: the
    /// base's reach, then the new one (none when no longer measured).
    pub narrowed: Vec<(Root, Reach, Reach)>,
}

impl Losses {
    pub fn is_empty(&self) -> bool {
        self.lost.is_empty() && self.narrowed.is_empty()
    }
}

/// What the `new` claims lose against the `base` ones beyond the `accepted`
/// acceptances. Case counts, observation, state, totals and untriaged
/// locations are reported only.
pub fn losses(base: &Claims, new: &Claims, accepted: &Accepted) -> Losses {
    let mut losses = Losses::default();
    for (root, reach) in base {
        let Some(now) = new.get(root) else {
            if !accepted.retired.contains(root) {
                losses.lost.push(root.clone());
            }
            continue;
        };
        let (Some(before), now) = (*reach, now.unwrap_or_default()) else {
            continue;
        };
        let kept = accepted
            .narrowed
            .get(root)
            .is_some_and(|reach| now.covers(*reach));
        if !now.covers(before) && !kept {
            losses.narrowed.push((root.clone(), before, now));
        }
    }
    losses
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(symbol: &str) -> Root {
        ("libphy".to_owned(), symbol.to_owned())
    }

    fn reach(blocks: u64, directions: u64) -> Option<Reach> {
        Some(Reach { blocks, directions })
    }

    #[test]
    fn a_root_is_lost_when_no_shard_claims_it_and_narrowed_when_it_reaches_less() {
        let base = Claims::from([
            (root("set_chan"), reach(10, 8)),
            (root("cal"), reach(5, 4)),
            (root("old"), reach(1, 0)),
            (root("narrow"), reach(9, 9)),
            (root("stand"), None),
        ]);
        let new = Claims::from([
            // Moved scenario or production entry and reaches further: kept.
            (root("set_chan"), reach(12, 8)),
            (root("narrow"), reach(9, 7)),
            (root("stand"), None),
            (root("added"), reach(3, 3)),
        ]);
        let mut accepted = Accepted::default();
        accepted.retired.insert(root("old"));
        let losses = losses(&base, &new, &accepted);
        assert_eq!(losses.lost, [root("cal")]);
        assert_eq!(
            losses.narrowed,
            [(root("narrow"), reach(9, 9).unwrap(), reach(9, 7).unwrap())]
        );
        assert!(super::losses(&base, &base, &Accepted::default()).is_empty());
    }

    #[test]
    fn an_accepted_narrowing_holds_its_reach_and_no_further_narrowing() {
        let base = Claims::from([(root("narrow"), reach(9, 9))]);
        let mut accepted = Accepted::default();
        accepted
            .narrowed
            .insert(root("narrow"), reach(9, 7).unwrap());
        let kept = Claims::from([(root("narrow"), reach(9, 7))]);
        assert!(losses(&base, &kept, &accepted).is_empty());
        let further = Claims::from([(root("narrow"), reach(8, 7))]);
        assert_eq!(losses(&base, &further, &accepted).narrowed.len(), 1);
        // A root whose comparisons are no longer measured reaches nothing.
        let unmeasured = Claims::from([(root("narrow"), None)]);
        assert_eq!(
            losses(&base, &unmeasured, &Accepted::default()).narrowed,
            [(root("narrow"), reach(9, 9).unwrap(), Reach::default())]
        );
    }

    #[test]
    fn an_acceptance_names_its_root_reason_and_reach_and_matches_the_index() {
        let parse = |text: &str| Accepted::parse("decisions/accepted-losses.toml".into(), text);
        let text = r#"
            [[retired]]
            source = "libphy"
            symbol = "old"
            reason = "replaced"
            [[narrowed]]
            source = "libphy"
            symbol = "narrow"
            blocks = 9
            directions = 7
            reason = "the diagnostic path is excluded"
        "#;
        let accepted = parse(text).unwrap();
        assert!(accepted.retired.contains(&root("old")));
        assert_eq!(accepted.narrowed[&root("narrow")], reach(9, 7).unwrap());
        assert!(parse("").unwrap().narrowed.is_empty());
        let unreasoned = "[[retired]]\nsource = \"a\"\nsymbol = \"b\"\nreason = \" \"\n";
        assert!(
            parse(unreasoned)
                .unwrap_err()
                .to_string()
                .contains("reason")
        );
        assert!(
            parse("[[retired]]\nsource = \"a\"\nsymbol = \"b\"\nreason = \"c\"\nwhy = 1\n")
                .is_err()
        );
        let claims = Claims::from([(root("narrow"), reach(9, 7))]);
        assert!(accepted.check("chip", &claims).is_ok());
        // Comparisons that reach beyond the accepted narrowing outgrew it: kept,
        // it would let them narrow back unnoticed.
        for outgrown in [reach(12, 7), reach(9, 8)] {
            let claims = Claims::from([(root("narrow"), outgrown)]);
            let error = accepted.check("chip", &claims).unwrap_err().to_string();
            assert!(error.contains("beyond its accepted narrowing"), "{error}");
        }
        let both = Claims::from([(root("narrow"), reach(9, 7)), (root("old"), None)]);
        let error = accepted.check("chip", &both).unwrap_err().to_string();
        assert!(error.contains("libphy::old is still claimed"), "{error}");
        let error = accepted
            .check("chip", &Claims::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("libphy::narrow is not claimed"), "{error}");
    }

    #[test]
    fn claims_keep_the_best_reach_of_a_root_and_read_an_earlier_format() {
        let directory = tempfile::tempdir().unwrap();
        assert!(claims(directory.path()).unwrap().claims.is_empty());
        // An earlier format: another schema and fields this one lacks.
        let earlier = r#"{"schema": 1, "gone": true, "entries": [
            {"source": "libphy", "symbol": "set_chan", "gone": 0,
             "coverage": {"blocks": {"reached": 4, "total": 9}, "directions": {"reached": 7, "total": 8}}},
            {"source": "libphy", "symbol": "set_chan",
             "coverage": {"blocks": {"reached": 6, "total": 9}, "directions": {"reached": 2, "total": 8}}},
            {"source": "libphy", "symbol": "stand"}]}"#;
        std::fs::write(store::path(directory.path(), "radio"), earlier).unwrap();
        let read = claims(directory.path()).unwrap();
        assert_eq!(read.claims[&root("set_chan")], reach(6, 7));
        assert_eq!(read.claims[&root("stand")], None);
        assert!(read.unread_coverage.is_empty());
        // Coverage of another form in any entry leaves the root's reach
        // unknown, not the other entries' partial one; the root stays claimed.
        let other = r#"{"entries": [{"source": "libphy", "symbol": "set_chan",
            "coverage": {"blocks": 4, "branches": {"hit": 7}}},
            {"source": "libphy", "symbol": "set_chan",
             "coverage": {"blocks": {"reached": 2, "total": 9}, "directions": {"reached": 1, "total": 8}}}]}"#;
        std::fs::write(store::path(directory.path(), "radio"), other).unwrap();
        let read = claims(directory.path()).unwrap();
        assert_eq!(read.claims[&root("set_chan")], None);
        assert_eq!(read.unread_coverage, [root("set_chan")].into());
        // The change's own index is the current format: unread coverage there
        // is an error.
        let error = read.current().unwrap_err().to_string();
        assert!(error.contains("libphy::set_chan"), "{error}");
        std::fs::write(store::path(directory.path(), "radio"), r#"{"schema": 1}"#).unwrap();
        let error = claims(directory.path()).unwrap_err().to_string();
        assert!(error.contains("cannot be compared"), "{error}");
    }
}
