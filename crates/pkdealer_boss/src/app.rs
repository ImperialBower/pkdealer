//! The offline Boss pipeline: read a recorded session (+ optional ground-truth
//! labels), run the blind detection pipeline, and render the report.
//!
//! This is the provably-blind path: it reads an already-recorded session file,
//! needs no spectator token, and runs [`redact`] before any detection code
//! touches the data.

use std::path::PathBuf;

use pkcore::hand_history::HandCollection;

use crate::detector::{SprtParams, assess};
use crate::error::BossError;
use crate::labels::GroundTruthLabels;
use crate::redacted::redact;
use crate::report::render;
use crate::scorer::score;
use crate::signals::{aggregate, names_from, pairs_in};

/// Inputs for one Boss run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// Recorded `HandCollection` session file (YAML or `ExportSession` JSON).
    pub session: PathBuf,
    /// Optional ground-truth labels sidecar (YAML); enables the scorer section.
    pub labels: Option<PathBuf>,
}

/// Runs the pipeline and returns the rendered report.
///
/// # Errors
///
/// Returns [`BossError::Io`] when a file cannot be read, [`BossError::Parse`]
/// when the session or labels payload does not parse, and [`BossError::Empty`]
/// when the session holds no attributable hands.
///
/// # Examples
///
/// ```no_run
/// use pkdealer_boss::app::{run, RunConfig};
///
/// let report = run(&RunConfig { session: "session.yaml".into(), labels: None })?;
/// println!("{report}");
/// # Ok::<(), pkdealer_boss::error::BossError>(())
/// ```
pub fn run(config: &RunConfig) -> Result<String, BossError> {
    let raw = std::fs::read_to_string(&config.session)?;
    let collection = parse_collection(&raw)?;
    let hands = redact(&collection);
    if hands.is_empty() {
        return Err(BossError::Empty);
    }

    let params = SprtParams::default();
    let signals: Vec<_> = pairs_in(&hands)
        .iter()
        .map(|pair| aggregate(&hands, pair))
        .collect();
    let verdicts = assess(&hands, &params);
    let names = names_from(&hands);

    let report = match &config.labels {
        Some(path) => {
            let raw = std::fs::read_to_string(path)?;
            let labels = GroundTruthLabels::from_yaml(&raw)?;
            Some(score(&collection, &labels, &verdicts))
        }
        None => None,
    };

    Ok(render(
        &verdicts,
        &signals,
        &names,
        report.as_ref(),
        &params,
    ))
}

/// Parses a `HandCollection` from YAML (the EPIC-25 disk sink) or JSON (the
/// `ExportSession` payload), chosen by the first non-space byte.
fn parse_collection(raw: &str) -> Result<HandCollection, BossError> {
    if raw.trim_start().starts_with('{') {
        serde_json::from_str(raw).map_err(|e| BossError::Parse(e.to_string()))
    } else {
        HandCollection::from_yaml(raw).map_err(|e| BossError::Parse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::labels::{GroundTruthLabels, LabelStyle, LabelVector};

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn run_end_to_end_on_recorded_yaml() {
        let c = fixtures::collection(fixtures::dump_corpus(120));
        let yaml = c.to_yaml().unwrap();
        let session = scratch("boss_e2e_session.yaml");
        let labels_path = scratch("boss_e2e_labels.yaml");
        std::fs::write(&session, yaml).unwrap();
        let labels = GroundTruthLabels::resolve(
            &c,
            &[(
                "mallory_1".into(),
                "trudy_1".into(),
                LabelVector::Spectator,
                LabelStyle::ChipDump,
            )],
        )
        .unwrap();
        std::fs::write(&labels_path, labels.to_yaml().unwrap()).unwrap();

        let report = run(&RunConfig {
            session,
            labels: Some(labels_path),
        })
        .unwrap();
        assert!(report.contains("mallory_1 + trudy_1"));
        assert!(report.contains("DETECTED"));
        assert!(report.contains("false positives: 0"));
    }

    #[test]
    fn run_missing_file_is_io_error() {
        let err = run(&RunConfig {
            session: "/nonexistent/x.yaml".into(),
            labels: None,
        })
        .unwrap_err();
        assert!(matches!(err, BossError::Io(_)));
    }

    #[test]
    fn run_garbage_payload_is_parse_error() {
        let session = scratch("boss_garbage.yaml");
        std::fs::write(&session, "not yaml [").unwrap();
        let err = run(&RunConfig {
            session,
            labels: None,
        })
        .unwrap_err();
        assert!(matches!(err, BossError::Parse(_)));
    }

    #[test]
    fn run_empty_collection_is_empty_error() {
        let session = scratch("boss_empty.yaml");
        std::fs::write(&session, fixtures::collection(vec![]).to_yaml().unwrap()).unwrap();
        let err = run(&RunConfig {
            session,
            labels: None,
        })
        .unwrap_err();
        assert!(matches!(err, BossError::Empty));
    }
}
