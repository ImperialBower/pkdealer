//! Ground-truth collusion labels — the YAML sidecar used to grade Boss
//! detections against a known-colluding scenario.
//!
//! Labels are keyed by player [`Uuid`] rather than by seat or display name,
//! so they remain valid even when a colluding pair moves seats or is dealt
//! into hands under different aliases across a session.
//! [`GroundTruthLabels::resolve`] bridges human-readable fixture names
//! (e.g. `"gto_1"`) to the UUIDs a recorded
//! [`pkcore::hand_history::HandCollection`] actually assigned them, so a
//! scenario author never has to hand-type UUIDs.

use std::collections::HashMap;

use pkcore::hand_history::HandCollection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::BossError;

/// The relationship between two colluding seats: whether one seat passively
/// defers to the other ("spectator") or both actively coordinate as equals
/// ("peer").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelVector {
    /// One player folds, checks down, or plays weak to protect or inform
    /// the other, without symmetrical benefit in return.
    Spectator,
    /// Both players actively coordinate for mutual benefit.
    Peer,
}

/// The collusion technique used to construct a labeled scenario.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelStyle {
    /// One player folds or checks down hands they would otherwise contest,
    /// to avoid contesting a pot with their partner.
    SoftPlay,
    /// Alternating aggression forces a third player to face repeated raises
    /// from two colluding seats ("the sandwich").
    Whipsaw,
    /// One player deliberately loses chips to their partner across hands.
    ChipDump,
}

/// A single labeled colluding pair, keyed by stable player [`Uuid`], with
/// the display names each player was seated under at label time retained
/// for readability of the YAML sidecar.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::labels::{LabeledPair, LabelStyle, LabelVector};
/// use uuid::Uuid;
///
/// let pair = LabeledPair {
///     a: Uuid::from_u128(1),
///     b: Uuid::from_u128(2),
///     a_name: "mallory_1".to_string(),
///     b_name: "trudy_1".to_string(),
///     vector: LabelVector::Spectator,
///     style: LabelStyle::ChipDump,
/// };
/// assert_eq!(pair.a_name, "mallory_1");
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabeledPair {
    /// First colluding player's stable identity.
    pub a: Uuid,
    /// Second colluding player's stable identity.
    pub b: Uuid,
    /// First colluding player's display name at label time.
    pub a_name: String,
    /// Second colluding player's display name at label time.
    pub b_name: String,
    /// Spectator/peer relationship between `a` and `b`.
    pub vector: LabelVector,
    /// Collusion technique used to construct the scenario.
    pub style: LabelStyle,
}

/// Ground-truth collusion labels for a recorded arena session: every
/// known-colluding seat pair, used to grade Boss detections.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::labels::{GroundTruthLabels, LabeledPair, LabelStyle, LabelVector};
/// use uuid::Uuid;
///
/// let labels = GroundTruthLabels {
///     colluding_pairs: vec![LabeledPair {
///         a: Uuid::from_u128(1),
///         b: Uuid::from_u128(2),
///         a_name: "mallory_1".to_string(),
///         b_name: "trudy_1".to_string(),
///         vector: LabelVector::Spectator,
///         style: LabelStyle::ChipDump,
///     }],
/// };
/// assert!(labels.is_colluding(Uuid::from_u128(1), Uuid::from_u128(2)));
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundTruthLabels {
    /// Every known-colluding seat pair in the session.
    pub colluding_pairs: Vec<LabeledPair>,
}

impl GroundTruthLabels {
    /// Parses ground-truth labels from a YAML sidecar.
    ///
    /// # Errors
    /// Returns [`BossError::Parse`] if `yaml` is not valid
    /// `GroundTruthLabels` YAML.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::labels::GroundTruthLabels;
    ///
    /// let labels = GroundTruthLabels::from_yaml("colluding_pairs: []\n").unwrap();
    /// assert!(labels.colluding_pairs.is_empty());
    /// ```
    pub fn from_yaml(yaml: &str) -> Result<Self, BossError> {
        serde_yaml_bw::from_str(yaml).map_err(|e| BossError::Parse(e.to_string()))
    }

    /// Serializes ground-truth labels to YAML.
    ///
    /// # Errors
    /// Returns [`BossError::Parse`] if serialization fails. This should
    /// not happen for a well-formed `GroundTruthLabels` value; the error
    /// path exists so callers can propagate failures uniformly through
    /// [`BossError`] rather than a `serde_yaml_bw`-specific type.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::labels::GroundTruthLabels;
    ///
    /// let labels = GroundTruthLabels { colluding_pairs: vec![] };
    /// let yaml = labels.to_yaml().unwrap();
    /// assert!(yaml.contains("colluding_pairs"));
    /// ```
    pub fn to_yaml(&self) -> Result<String, BossError> {
        serde_yaml_bw::to_string(self).map_err(|e| BossError::Parse(e.to_string()))
    }

    /// Reports whether `x` and `y` are a labeled colluding pair, regardless
    /// of argument order.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::labels::{GroundTruthLabels, LabeledPair, LabelStyle, LabelVector};
    /// use uuid::Uuid;
    ///
    /// let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    /// let labels = GroundTruthLabels {
    ///     colluding_pairs: vec![LabeledPair {
    ///         a,
    ///         b,
    ///         a_name: "a".to_string(),
    ///         b_name: "b".to_string(),
    ///         vector: LabelVector::Peer,
    ///         style: LabelStyle::Whipsaw,
    ///     }],
    /// };
    /// assert!(labels.is_colluding(a, b));
    /// assert!(labels.is_colluding(b, a));
    /// assert!(!labels.is_colluding(a, c));
    /// ```
    #[must_use]
    pub fn is_colluding(&self, x: Uuid, y: Uuid) -> bool {
        self.colluding_pairs
            .iter()
            .any(|p| (p.a == x && p.b == y) || (p.a == y && p.b == x))
    }

    /// Resolves human-readable player names to their session UUIDs using
    /// `collection`, then builds [`GroundTruthLabels`] from the resolved
    /// `(a_name, b_name, vector, style)` tuples.
    ///
    /// Name resolution is latest-hand-wins: later hands in `collection`
    /// override earlier ones for the same name, mirroring how a session
    /// recorder assigns stable `player_id`s across re-seatings.
    ///
    /// # Errors
    /// Returns [`BossError::Parse`] if any name in `pairs` is not seated,
    /// with an identity, in any hand of `collection`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::labels::{GroundTruthLabels, LabelStyle, LabelVector};
    /// use pkcore::hand_history::HandCollection;
    ///
    /// let collection = HandCollection::new();
    /// let err = GroundTruthLabels::resolve(
    ///     &collection,
    ///     &[(
    ///         "nobody".to_string(),
    ///         "also_nobody".to_string(),
    ///         LabelVector::Peer,
    ///         LabelStyle::Whipsaw,
    ///     )],
    /// )
    /// .unwrap_err();
    /// assert!(err.to_string().contains("unknown player name"));
    /// ```
    pub fn resolve(
        collection: &HandCollection,
        pairs: &[(String, String, LabelVector, LabelStyle)],
    ) -> Result<Self, BossError> {
        // Latest-hand-wins name → id map (mirrors seat_ids_from_collection in agent_rules).
        let mut by_name: HashMap<&str, Uuid> = HashMap::new();
        for hand in collection.hands() {
            for p in &hand.players {
                if let Some(id) = p.player_id {
                    by_name.insert(p.name.as_str(), id);
                }
            }
        }
        let mut colluding_pairs = Vec::with_capacity(pairs.len());
        for (a_name, b_name, vector, style) in pairs {
            let a = *by_name
                .get(a_name.as_str())
                .ok_or_else(|| BossError::Parse(format!("unknown player name: {a_name}")))?;
            let b = *by_name
                .get(b_name.as_str())
                .ok_or_else(|| BossError::Parse(format!("unknown player name: {b_name}")))?;
            colluding_pairs.push(LabeledPair {
                a,
                b,
                a_name: a_name.clone(),
                b_name: b_name.clone(),
                vector: *vector,
                style: *style,
            });
        }
        Ok(Self { colluding_pairs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn labels_yaml_roundtrip() {
        let labels = GroundTruthLabels {
            colluding_pairs: vec![LabeledPair {
                a: fixtures::MALLORY,
                b: fixtures::TRUDY,
                a_name: "mallory_1".into(),
                b_name: "trudy_1".into(),
                vector: LabelVector::Spectator,
                style: LabelStyle::ChipDump,
            }],
        };
        let yaml = labels.to_yaml().unwrap();
        assert!(yaml.contains("chip_dump") && yaml.contains("spectator"));
        let back = GroundTruthLabels::from_yaml(&yaml).unwrap();
        assert_eq!(back.colluding_pairs.len(), 1);
        assert_eq!(back.colluding_pairs[0].a, fixtures::MALLORY);
    }

    #[test]
    fn is_colluding_is_order_insensitive() {
        let labels = GroundTruthLabels {
            colluding_pairs: vec![LabeledPair {
                a: fixtures::MALLORY,
                b: fixtures::TRUDY,
                a_name: "mallory_1".into(),
                b_name: "trudy_1".into(),
                vector: LabelVector::Peer,
                style: LabelStyle::SoftPlay,
            }],
        };
        assert!(labels.is_colluding(fixtures::MALLORY, fixtures::TRUDY));
        assert!(labels.is_colluding(fixtures::TRUDY, fixtures::MALLORY));
        assert!(!labels.is_colluding(fixtures::MALLORY, fixtures::GTO));
    }

    #[test]
    fn collude_with_resolves_composed_name_to_uuid() {
        let c = fixtures::collection(vec![fixtures::build_hand(fixtures::HandSpec {
            no: 1,
            players: vec![
                fixtures::player(0, "gto_1", fixtures::GTO, 1_000.0, None),
                fixtures::player(1, "gto_2", fixtures::TAG, 1_000.0, None),
            ],
            preflop: vec![],
            flop: None,
            turn: None,
            river: None,
            nets: vec![(0, 0.0), (1, 0.0)],
        })]);
        let labels = GroundTruthLabels::resolve(
            &c,
            &[(
                "gto_1".into(),
                "gto_2".into(),
                LabelVector::Spectator,
                LabelStyle::SoftPlay,
            )],
        )
        .unwrap();
        assert_eq!(labels.colluding_pairs[0].a, fixtures::GTO);
        assert_eq!(labels.colluding_pairs[0].b, fixtures::TAG);
    }

    #[test]
    fn resolve_unknown_name_errors() {
        let c = fixtures::collection(vec![]);
        let err = GroundTruthLabels::resolve(
            &c,
            &[(
                "nobody".into(),
                "also_nobody".into(),
                LabelVector::Peer,
                LabelStyle::Whipsaw,
            )],
        )
        .unwrap_err();
        assert!(matches!(err, BossError::Parse(_)));
    }

    #[test]
    fn from_yaml_garbage_errors() {
        assert!(GroundTruthLabels::from_yaml(": not yaml [").is_err());
    }
}
