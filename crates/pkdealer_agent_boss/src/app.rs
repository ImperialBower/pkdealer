//! The live Boss pipeline: poll `ExportSession`, redact at ingest, run the blind
//! SPRT detector, and emit per-pair verdicts as structured logs + `OTel`.
//!
//! # Trust boundary (be honest about it)
//!
//! `ExportSession` is spectator-gated, so this process holds the same token a
//! Vector-A cheater would, and momentarily holds the un-redacted
//! `HandCollection` between receiving the export and calling [`ingest`]. The
//! typed firewall guarantees the *detection library* never receives a card — not
//! that the process never touches card bytes. [`ingest`] drops the collection
//! the instant it has produced `RedactedHand`s, before any detector code runs.
//! For a provably-blind path, prefer the offline `pkdealer_boss` analyzer, which
//! reads an already-exported file and needs no token.
//!
//! # Validation status
//!
//! Authored for EPIC-70 Phase 4 but **not run against a live arena** in this
//! session (that needs a multi-container `docker compose` stack). The pure
//! pieces — [`ingest`] (redact-at-ingest) and [`evaluate`] (flag/false-positive
//! decision) — are unit-tested; the [`run`] poll loop is exercised only by those
//! pieces, not end-to-end.

use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use pkcore::hand_history::HandCollection;
use pkdealer_boss::detector::{SprtParams, Verdict, assess};
use pkdealer_boss::labels::GroundTruthLabels;
use pkdealer_boss::redacted::{RedactedHand, redact};
use pkdealer_boss::signals::Pair;
use pkdealer_proto::dealer::dealer_service_client::DealerServiceClient;
use pkdealer_proto::dealer::{ExportSessionRequest, GetSessionInfoRequest, RecordFormat};

use crate::otel::Metrics;

/// gRPC metadata key carrying the caller's visibility token. Mirrors
/// `PLAYER_TOKEN_METADATA_KEY` in the service; `ExportSession` requires the
/// spectator token because its payload carries every seat's hole cards.
const PLAYER_TOKEN_METADATA_KEY: &str = "x-player-token";

/// Inputs for a live Boss run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// Dealer service endpoint, e.g. `http://pkdealer_service:50051`.
    pub endpoint: String,
    /// Spectator token presented on `ExportSession` (the service gates it).
    pub spectator_token: String,
    /// Optional ground-truth labels sidecar (YAML). Enables the
    /// `false_positive` counter; without it a blind boss cannot know truth.
    pub labels: Option<PathBuf>,
    /// Delay between `GetSessionInfo` polls.
    pub interval: Duration,
    /// When true, poll once and return (a smoke check); otherwise loop forever.
    pub once: bool,
}

/// Errors from a live Boss run.
#[derive(Debug)]
pub enum AgentBossError {
    /// The dealer endpoint could not be dialed.
    Connect(String),
    /// The labels sidecar could not be read.
    Io(String),
    /// The labels sidecar did not parse.
    Labels(String),
}

impl fmt::Display for AgentBossError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "cannot connect to dealer: {e}"),
            Self::Io(e) => write!(f, "cannot read labels: {e}"),
            Self::Labels(e) => write!(f, "cannot parse labels: {e}"),
        }
    }
}

impl std::error::Error for AgentBossError {}

/// A flag the Boss raised this cycle. Returned by [`evaluate`] so the decision
/// is testable independently of `OTel`/logging side effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagEvent {
    /// The newly-flagged pair.
    pub pair: Pair,
    /// Hand index at which it crossed the threshold.
    pub hand: u32,
    /// True when a labels sidecar proves the pair is actually honest.
    pub false_positive: bool,
}

/// Parses the `ExportSession` JSON payload into a `HandCollection` and redacts
/// it, dropping every hole card and the deck at the boundary. The collection is
/// consumed here and never returned, so no detector code can see a card.
///
/// # Errors
///
/// Returns the serde error string when the payload is not a valid
/// `HandCollection` JSON document.
///
/// # Examples
///
/// ```
/// use pkdealer_agent_boss::app::ingest;
/// // An empty collection round-trips to zero redacted hands.
/// let payload = serde_json::to_string(&pkcore::hand_history::HandCollection::new()).unwrap();
/// assert!(ingest(&payload).unwrap().is_empty());
/// ```
pub fn ingest(payload: &str) -> Result<Vec<RedactedHand>, String> {
    let collection: HandCollection = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    Ok(redact(&collection))
}

/// Decides which verdicts represent a *newly* flagged pair this cycle, marking
/// them in `flagged` so the same pair is never re-emitted. A pair is a false
/// positive when `labels` proves it honest.
///
/// Pure and side-effect-free apart from the `flagged` set it advances, so the
/// flag/false-positive logic is unit-tested without a live service or `OTel`.
///
/// # Examples
///
/// ```
/// use std::collections::HashSet;
/// use pkdealer_agent_boss::app::evaluate;
/// // No verdicts → no flags.
/// let mut flagged = HashSet::new();
/// assert!(evaluate(&[], None, &mut flagged).is_empty());
/// ```
// The `flagged` set is always the default-hasher set the loop owns; generalizing
// this internal helper over `BuildHasher` would be noise for one caller.
#[allow(clippy::implicit_hasher)]
#[must_use]
pub fn evaluate(
    verdicts: &[Verdict],
    labels: Option<&GroundTruthLabels>,
    flagged: &mut HashSet<Pair>,
) -> Vec<FlagEvent> {
    let mut events = Vec::new();
    for verdict in verdicts {
        let Some(hand) = verdict.flagged_at_hand else {
            continue;
        };
        // Only emit the first time a pair crosses the threshold.
        if !flagged.insert(verdict.pair) {
            continue;
        }
        let false_positive =
            labels.is_some_and(|l| !l.is_colluding(verdict.pair.a, verdict.pair.b));
        events.push(FlagEvent {
            pair: verdict.pair,
            hand,
            false_positive,
        });
    }
    events
}

/// A stable, human-readable label for a pair (`a+b`), used as the `OTel` `pair`
/// attribute and in log lines.
#[must_use]
pub fn pair_label(pair: &Pair) -> String {
    format!("{}+{}", pair.a, pair.b)
}

/// Records a cycle's verdicts to `OTel` and logs any new flags.
fn observe(metrics: &Metrics, verdicts: &[Verdict], events: &[FlagEvent]) {
    for verdict in verdicts {
        metrics.record_llr(&pair_label(&verdict.pair), verdict.llr);
    }
    for event in events {
        let label = pair_label(&event.pair);
        metrics.record_flag_hand(&label, event.hand);
        tracing::warn!(pair = %label, hand = event.hand, "FLAG: suspected collusion");
        if event.false_positive {
            metrics.record_false_positive(&label);
            tracing::warn!(pair = %label, "flagged pair is labelled honest (false positive)");
        }
    }
}

/// Loads the optional labels sidecar.
fn load_labels(config: &RunConfig) -> Result<Option<GroundTruthLabels>, AgentBossError> {
    match &config.labels {
        None => Ok(None),
        Some(path) => {
            let raw =
                std::fs::read_to_string(path).map_err(|e| AgentBossError::Io(e.to_string()))?;
            let labels = GroundTruthLabels::from_yaml(&raw)
                .map_err(|e| AgentBossError::Labels(e.to_string()))?;
            Ok(Some(labels))
        }
    }
}

/// Pulls the current completed-hand count from the service.
async fn session_hand_count(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
) -> Option<u32> {
    match client.get_session_info(GetSessionInfoRequest {}).await {
        Ok(resp) => Some(resp.into_inner().hand_count),
        Err(_) => None,
    }
}

/// Exports the session as JSON under the spectator token, returning the payload.
async fn export_payload(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
    spectator_token: &str,
) -> Option<String> {
    let mut request = tonic::Request::new(ExportSessionRequest {
        format: RecordFormat::Json as i32,
        drain: false,
    });
    if let Ok(value) = spectator_token.parse() {
        request
            .metadata_mut()
            .insert(PLAYER_TOKEN_METADATA_KEY, value);
    }
    match client.export_session(request).await {
        Ok(resp) => Some(resp.into_inner().payload),
        Err(_) => None,
    }
}

/// Runs the live Boss: connect, then poll `ExportSession` on the watermark
/// cadence, redacting at ingest and emitting per-pair verdicts.
///
/// # Errors
///
/// Returns [`AgentBossError::Connect`] when the dealer cannot be dialed, and the
/// `Io`/`Labels` variants when a supplied labels sidecar cannot be loaded. Poll
/// failures mid-run are best-effort (logged and retried), never fatal.
pub async fn run(config: &RunConfig) -> Result<(), AgentBossError> {
    let labels = load_labels(config)?;
    let mut client = DealerServiceClient::connect(config.endpoint.clone())
        .await
        .map_err(|e| AgentBossError::Connect(e.to_string()))?;
    let metrics = Metrics::new();
    let params = SprtParams::default();

    let mut last_count = 0u32;
    let mut flagged: HashSet<Pair> = HashSet::new();

    tracing::info!(endpoint = %config.endpoint, "boss online — polling for completed hands");
    loop {
        if let Some(count) = session_hand_count(&mut client).await {
            if count > last_count {
                if let Some(payload) = export_payload(&mut client, &config.spectator_token).await {
                    match ingest(&payload) {
                        Ok(hands) => {
                            let verdicts = assess(&hands, &params);
                            let events = evaluate(&verdicts, labels.as_ref(), &mut flagged);
                            observe(&metrics, &verdicts, &events);
                            last_count = count;
                        }
                        Err(e) => tracing::warn!(error = %e, "skipping unparseable export"),
                    }
                }
            }
        }
        if config.once {
            return Ok(());
        }
        tokio::time::sleep(config.interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkcore::analysis::player_stats::Confidence;
    use pkdealer_boss::labels::{LabelStyle, LabelVector, LabeledPair};
    use uuid::Uuid;

    // A colluding pair used across the synthetic-verdict tests.
    const MALLORY: Uuid = Uuid::from_u128(0xA1);
    const TRUDY: Uuid = Uuid::from_u128(0xA2);

    /// A verdict that flagged at `hand`, or never flagged when `hand` is `None`.
    fn verdict(a: Uuid, b: Uuid, hand: Option<u32>) -> Verdict {
        Verdict {
            pair: Pair::new(a, b),
            llr: 99.0,
            hands_observed: 120,
            confidence: Confidence::High,
            flagged_at_hand: hand,
        }
    }

    #[test]
    fn ingest_empty_collection_yields_no_hands() {
        let payload = serde_json::to_string(&HandCollection::new()).unwrap();
        assert!(ingest(&payload).unwrap().is_empty());
    }

    #[test]
    fn ingest_rejects_garbage() {
        assert!(ingest("not json {").is_err());
    }

    #[test]
    fn evaluate_ignores_unflagged_verdicts() {
        let mut flagged = HashSet::new();
        let events = evaluate(&[verdict(MALLORY, TRUDY, None)], None, &mut flagged);
        assert!(events.is_empty());
        assert!(flagged.is_empty());
    }

    #[test]
    fn evaluate_flags_a_crossing_pair_once() {
        let verdicts = [verdict(MALLORY, TRUDY, Some(57))];
        let mut flagged = HashSet::new();

        let first = evaluate(&verdicts, None, &mut flagged);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].pair, Pair::new(MALLORY, TRUDY));
        assert_eq!(first[0].hand, 57);
        assert!(!first[0].false_positive); // no labels → cannot be an FP

        // Second pass over the same verdict emits nothing — already flagged.
        let second = evaluate(&verdicts, None, &mut flagged);
        assert!(second.is_empty(), "a flagged pair is never re-emitted");
    }

    #[test]
    fn evaluate_true_positive_when_labels_agree() {
        let verdicts = [verdict(MALLORY, TRUDY, Some(51))];
        let labels = GroundTruthLabels {
            colluding_pairs: vec![LabeledPair {
                a: MALLORY,
                b: TRUDY,
                a_name: "mallory_1".into(),
                b_name: "trudy_1".into(),
                vector: LabelVector::Peer,
                style: LabelStyle::ChipDump,
            }],
        };
        let mut flagged = HashSet::new();
        let events = evaluate(&verdicts, Some(&labels), &mut flagged);
        assert_eq!(events.len(), 1);
        assert!(
            !events[0].false_positive,
            "labelled colluders are true positives"
        );
    }

    #[test]
    fn evaluate_false_positive_when_labels_disagree() {
        // A flag against a pair the (empty) labels do not list as colluding.
        let verdicts = [verdict(
            Uuid::from_u128(0xaa),
            Uuid::from_u128(0xbb),
            Some(60),
        )];
        let labels = GroundTruthLabels {
            colluding_pairs: vec![],
        };
        let mut flagged = HashSet::new();
        let events = evaluate(&verdicts, Some(&labels), &mut flagged);
        assert_eq!(events.len(), 1);
        assert!(events[0].false_positive);
    }
}
