#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_rules`
//!
//! A rule-based poker bot that drives decisions through a pkcore [`BotProfile`].
//! The profile controls aggression, bluff frequency, and bet sizing; the bot
//! uses [`RuleBasedDecider`] to translate profile parameters into concrete
//! poker actions.
//!
//! ## Usage
//!
//! ```text
//! # Named built-in profile (default: gto)
//! cargo run --bin pkdealer_agent_rules -- --name alice --profile gto
//!
//! # YAML file on disk
//! cargo run --bin pkdealer_agent_rules -- --name bob --profile data/bots/loose_aggressive.yaml
//!
//! # Override individual pkcore 0.3.0 (EPIC-36) decision-capability knobs on top
//! # of any profile — here: Monte-Carlo equity, position-aware ranges, draw outs.
//! cargo run --bin pkdealer_agent_rules -- --name carol --profile gto \
//!     --equity fast --equity-samples 4000 --ranges position-aware --outs on
//! ```
//!
//! ## Supported built-in profile names
//!
//! `gto`, `tight_passive` (`tp`), `loose_aggressive` (`lag`),
//! `tight_aggressive` (`tag`), `loose_passive` (`lp`), `maniac`, `abc`,
//! `short_stack_ninja` (`ssn`), `joker`, `strong_all_on` (`strong`),
//! `weak_all_off` (`weak`)
//!
//! `strong_all_on` and `weak_all_off` are the EPIC-36 reference profiles: the
//! same `gto` base with every graded [`pkcore::bot::decision_config::DecisionConfig`]
//! knob dialed all the way
//! up (exact-ish equity, position-aware ranges, strict pot odds) versus all the
//! way down (hand-rank proxy, flat ranges, pot odds ignored).
//!
//! ## Decision-capability overrides (EPIC-36)
//!
//! Each flag below overrides one knob of the loaded profile's
//! [`pkcore::bot::decision_config::DecisionConfig`]. Omitting a flag leaves that
//! knob at whatever the profile specified (its own `decision:` section, or the
//! historical default).
//!
//! | Flag | Values | Knob |
//! |------|--------|------|
//! | `--equity` | `off`, `fast`, `exact` | Postflop hand-strength source |
//! | `--equity-samples` | `u32` (default `2000`) | Monte-Carlo budget for `--equity fast` |
//! | `--ranges` | `flat`, `position-aware` | Preflop range source |
//! | `--pot-odds-discipline` | `0.0`–`1.0` | Call-threshold strictness |
//! | `--outs` | `off`, `on` | Draw/outs equity augmentation |
//! | `--exploit` | `off`, `light`, `heavy` | Opponent-adjusted exploitation |
//! | `--preflop-charts` | `off`, `hup`, `solver` | Preflop decision-chart source |
//!
//! ## Environment variables
//!
//! | Variable | Default | Purpose |
//! |----------|---------|---------|
//! | `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | Override `--endpoint` |

use std::collections::HashMap;
use std::process;

use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use pkcore::Forgiving;
use pkcore::analysis::player_stats::StatsRegistry;
use pkcore::bot::decider::{BotDecider, RuleBasedDecider};
use pkcore::bot::decision_config::{
    EquityMode, ExploitMode, PotOddsConfig, PreflopCharts, RangeMode, Toggle,
};
use pkcore::bot::player_action::PlayerAction as PkcoreAction;
use pkcore::bot::profile::BotProfile;
use pkcore::bot::table_snapshot::{SeatInfo, TableSnapshot};
use pkcore::cards::Cards;
use pkcore::games::GamePhase;
use pkcore::games::betting_structure::{BetTier, BettingStructure};
use pkcore::hand_history::HandCollection;
use pkdealer_agent_core::{AgentConfig, Decision, HandState, PokerAgent, run_agent};
use pkdealer_proto::dealer::dealer_service_client::DealerServiceClient;
use pkdealer_proto::dealer::{ExportSessionRequest, GetSessionInfoRequest, RecordFormat};
use tokio::sync::Mutex;
use tonic::transport::Channel;
use uuid::Uuid;

#[cfg(feature = "collusion")]
mod collude;
#[cfg(feature = "collusion")]
use collude::{CollusionChannel, CollusionConfig, CollusionStyle};

/// gRPC metadata key carrying the caller's auth/visibility token. Mirrors
/// `PLAYER_TOKEN_METADATA_KEY` in the service; `ExportSession` requires the
/// spectator token here because its payload carries every seat's hole cards.
pub(crate) const PLAYER_TOKEN_METADATA_KEY: &str = "x-player-token";

/// Rule-based poker bot connected to a pkdealer gRPC service.
#[derive(Debug, Parser)]
#[command(
    name = "pkdealer_agent_rules",
    about = "Rule-based poker agent driven by a pkcore BotProfile"
)]
struct Args {
    /// gRPC service address.
    #[arg(
        long,
        env = "PKDEALER_ENDPOINT",
        default_value = "http://127.0.0.1:50051"
    )]
    endpoint: String,

    /// Player name displayed at the table.
    #[arg(long, default_value = "rules")]
    name: String,

    /// Optional specific seat number (0–8). Omit to take the next available seat.
    #[arg(long)]
    seat: Option<u32>,

    /// Buy-in chip count.
    #[arg(long, default_value_t = 10_000)]
    chips: u32,

    /// Opaque seat-resume token. Empty (default) disables resume.
    #[arg(long, default_value = "")]
    client_secret: String,

    /// Profile name (`gto`, `tight_passive`, `lag`, …) or path to a YAML file.
    #[arg(long, default_value = "gto")]
    profile: String,

    /// Override the postflop equity source. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    equity: Option<EquityArg>,

    /// Monte-Carlo sample budget used when `--equity fast` is selected.
    #[arg(long, default_value_t = 2_000)]
    equity_samples: u32,

    /// Override the preflop range source. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    ranges: Option<RangesArg>,

    /// Override pot-odds discipline in `[0.0, 1.0]` (1.0 = strict break-even,
    /// 0.0 = ignore pot odds). Out-of-range values are clamped.
    #[arg(long)]
    pot_odds_discipline: Option<f64>,

    /// Override draw/outs equity augmentation. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    outs: Option<ToggleArg>,

    /// Override opponent-adjusted exploitation. When enabled (`light`/`heavy`),
    /// the agent pulls completed-hand history from the service via
    /// `ExportSession` and feeds the decider real opponent stats, so the
    /// adjustment engages against live opponents. Requires `--spectator-token`
    /// (the service gates `ExportSession`). Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    exploit: Option<ExploitArg>,

    /// Override the preflop decision-chart source. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    preflop_charts: Option<PreflopChartsArg>,

    /// Spectator token used to pull completed-hand histories via `ExportSession`
    /// when `exploit` is enabled. The service gates `ExportSession` on this token
    /// (its payload carries every seat's hole cards), so opponent-stat
    /// exploitation only engages when the operator hands the bot this shared
    /// secret. Matches the service's `PKDEALER_SPECTATOR_TOKEN` (default
    /// `spectator`). Ignored when `exploit` resolves to `off`.
    #[arg(long, env = "PKDEALER_SPECTATOR_TOKEN", default_value = "spectator")]
    spectator_token: String,

    /// Collude with the named partner (arena-composed name, e.g. `trudy_1`).
    /// EPIC-70: enables the cheating wrapper; requires a spectator token for
    /// the `spectator` channel. Absent ⇒ the agent is fully honest.
    #[cfg(feature = "collusion")]
    #[arg(long)]
    collude_with: Option<String>,

    /// Card-leak channel: `spectator` (Vector A) or `peer` (Vector B).
    #[cfg(feature = "collusion")]
    #[arg(long, value_enum, default_value = "spectator")]
    collusion_channel: CollusionChannelArg,

    /// Backchannel broker address used by the `peer` collusion channel
    /// (Vector B), e.g. `127.0.0.1:9099` — the pair swap hole cards here, over
    /// a relay the dealer never sees. Matches the broker's
    /// `PKDEALER_BACKCHANNEL_BIND`. Ignored by the `spectator` channel.
    #[cfg(feature = "collusion")]
    #[arg(long, env = "PKDEALER_BACKCHANNEL", default_value = "127.0.0.1:9099")]
    backchannel: String,

    /// Collusion strategy: `soft`, `whipsaw`, or `dump`.
    #[cfg(feature = "collusion")]
    #[arg(long, value_enum, default_value = "soft")]
    collusion_style: CollusionStyleArg,
}

/// CLI mirror of [`EquityMode`] (the `samples` budget comes from `--equity-samples`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum EquityArg {
    /// Hand-rank proxy — the historical behavior.
    Off,
    /// Seeded Monte Carlo with the `--equity-samples` budget.
    Fast,
    /// Exact enumeration of remaining runouts.
    Exact,
}

/// CLI mirror of [`RangeMode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RangesArg {
    /// Flat `range_strategy.open_raise` lookup — the historical behavior.
    Flat,
    /// Position-aware lookup via the profile's playbook.
    PositionAware,
}

/// CLI mirror of [`Toggle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ToggleArg {
    /// Capability disabled — the historical behavior.
    Off,
    /// Capability enabled.
    On,
}

/// CLI mirror of [`ExploitMode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ExploitArg {
    /// No opponent adjustment — the historical behavior.
    Off,
    /// Light adjustment (higher sample gate before adjusting).
    Light,
    /// Heavy adjustment (lower sample gate; adjusts sooner).
    Heavy,
}

/// CLI mirror of [`PreflopCharts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PreflopChartsArg {
    /// No chart — the historical range-membership behavior.
    Off,
    /// Heads-up precomputed odds table.
    Hup,
    /// Offline-generated GTO charts.
    Solver,
}

/// CLI mirror of [`CollusionChannel`] (EPIC-70).
#[cfg(feature = "collusion")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CollusionChannelArg {
    /// Vector A: partner cards read live via the spectator token.
    Spectator,
    /// Vector B: partner cards exchanged over the backchannel broker.
    Peer,
}

/// CLI mirror of [`CollusionStyle`] (EPIC-70).
#[cfg(feature = "collusion")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CollusionStyleArg {
    /// Never bet/raise into the partner heads-up.
    Soft,
    /// Re-raise behind the partner to squeeze third parties.
    Whipsaw,
    /// Fold the weaker team hand to the partner.
    Dump,
}

/// Resolves and validates the collusion flags into a [`CollusionConfig`].
///
/// Mirrors the `--exploit` validation posture: configuration errors are
/// fatal at startup (a colluder that cannot leak is a broken experiment,
/// not a degraded bot). Returns `Ok(None)` when `--collude-with` is absent —
/// the agent is then byte-for-byte the honest bot.
///
/// # Errors
///
/// Returns a human-readable message when the partner names this agent, when
/// the `spectator` channel is selected without a `--spectator-token`, or when
/// the `peer` channel is selected without a `--backchannel` address.
#[cfg(feature = "collusion")]
fn validate_collusion(args: &Args) -> Result<Option<CollusionConfig>, String> {
    let Some(partner) = args.collude_with.clone() else {
        return Ok(None);
    };
    if partner == args.name {
        return Err("--collude-with must name a different player".to_string());
    }
    let channel = match args.collusion_channel {
        CollusionChannelArg::Spectator => CollusionChannel::Spectator,
        CollusionChannelArg::Peer => CollusionChannel::Peer,
    };
    // Each channel validates only its own endpoint — exhaustively, so a new
    // channel cannot slip through unvalidated.
    match channel {
        CollusionChannel::Spectator => {
            if args.spectator_token.is_empty() {
                return Err(
                    "the spectator collusion channel requires --spectator-token".to_string()
                );
            }
        }
        CollusionChannel::Peer => {
            if args.backchannel.is_empty() {
                return Err(
                    "the peer collusion channel requires --backchannel (PKDEALER_BACKCHANNEL)"
                        .to_string(),
                );
            }
        }
    }
    let style = match args.collusion_style {
        CollusionStyleArg::Soft => CollusionStyle::Soft,
        CollusionStyleArg::Whipsaw => CollusionStyle::Whipsaw,
        CollusionStyleArg::Dump => CollusionStyle::Dump,
    };
    Ok(Some(CollusionConfig {
        partner,
        channel,
        style,
    }))
}

/// Opens the card channel named by a validated [`CollusionConfig`] and boxes it
/// as the channel-agnostic [`collude::PartnerCardSource`] the decide path uses.
///
/// `Spectator` dials the dealer a second time with the spectator token;
/// `Peer` dials the backchannel broker. Everything downstream of this function
/// is identical for both — that is the A/B equivalence the Boss relies on.
///
/// # Errors
///
/// Returns a human-readable message when the channel's endpoint is
/// unreachable. The caller exits: a colluder that cannot leak is a broken
/// experiment, not a degraded bot.
#[cfg(feature = "collusion")]
async fn connect_partner_source(
    args: &Args,
    config: &CollusionConfig,
) -> Result<Box<dyn collude::PartnerCardSource>, String> {
    match config.channel {
        CollusionChannel::Spectator => collude::spectator::SpectatorLeak::connect(
            args.endpoint.clone(),
            args.spectator_token.clone(),
            config.partner.clone(),
        )
        .await
        .map(|leak| Box::new(leak) as Box<dyn collude::PartnerCardSource>),
        CollusionChannel::Peer => {
            pkdealer_agent_core::backchannel::BackchannelClient::connect(&args.backchannel)
                .await
                .map(|client| {
                    Box::new(collude::backchannel_source::PeerSource { client })
                        as Box<dyn collude::PartnerCardSource>
                })
        }
    }
}

/// Applies any `--equity` / `--ranges` / … CLI overrides onto a loaded
/// profile's [`pkcore::bot::decision_config::DecisionConfig`], leaving each knob
/// untouched when its flag was omitted.
///
/// Returns `true` when at least one knob was overridden, so the caller can log
/// the change.
///
/// # Examples
///
/// ```text
/// # gto profile, but force Monte-Carlo equity and looser pot-odds discipline:
/// pkdealer_agent_rules --profile gto --equity fast --pot-odds-discipline 0.5
/// ```
fn apply_decision_overrides(profile: &mut BotProfile, args: &Args) -> bool {
    let mut changed = false;
    if let Some(equity) = args.equity {
        profile.decision.equity = match equity {
            EquityArg::Off => EquityMode::Off,
            EquityArg::Fast => EquityMode::Fast {
                samples: args.equity_samples,
            },
            EquityArg::Exact => EquityMode::Exact,
        };
        changed = true;
    }
    if let Some(ranges) = args.ranges {
        profile.decision.ranges = match ranges {
            RangesArg::Flat => RangeMode::Flat,
            RangesArg::PositionAware => RangeMode::PositionAware,
        };
        changed = true;
    }
    if let Some(discipline) = args.pot_odds_discipline {
        profile.decision.pot_odds = PotOddsConfig {
            discipline: discipline.clamp(0.0, 1.0),
        };
        changed = true;
    }
    if let Some(outs) = args.outs {
        profile.decision.outs = match outs {
            ToggleArg::Off => Toggle::Off,
            ToggleArg::On => Toggle::On,
        };
        changed = true;
    }
    if let Some(exploit) = args.exploit {
        profile.decision.exploit = match exploit {
            ExploitArg::Off => ExploitMode::Off,
            ExploitArg::Light => ExploitMode::Light,
            ExploitArg::Heavy => ExploitMode::Heavy,
        };
        changed = true;
    }
    if let Some(charts) = args.preflop_charts {
        profile.decision.preflop_charts = match charts {
            PreflopChartsArg::Off => PreflopCharts::Off,
            PreflopChartsArg::Hup => PreflopCharts::Hup,
            PreflopChartsArg::Solver => PreflopCharts::Solver,
        };
        changed = true;
    }
    changed
}

struct RulesAgent {
    profile: BotProfile,
    /// Opponent-stat source, present only when `profile.decision.exploit` is not
    /// `Off` and a service connection for `ExportSession` was established. When
    /// `None`, the agent behaves exactly as it did before opponent stats were
    /// wired: the snapshot carries `opponent_stats: None` and the `exploit` knob
    /// is inert (see [`ExploitPuller`]).
    exploit: Option<ExploitPuller>,
    /// EPIC-70: active collusion runtime — the resolved partner assignment plus
    /// its card channel. `None` ⇒ the agent is byte-for-byte honest.
    #[cfg(feature = "collusion")]
    collusion: Option<Colluder>,
}

/// A validated collusion assignment bound to its live card channel (EPIC-70).
#[cfg(feature = "collusion")]
struct Colluder {
    /// The resolved partner / channel / style assignment.
    config: CollusionConfig,
    /// Live partner-card channel, behind the channel-agnostic trait: Vector A
    /// (spectator leak) and Vector B (peer backchannel) are indistinguishable
    /// from here on.
    source: Box<dyn collude::PartnerCardSource>,
}

impl RulesAgent {
    /// Honest constructor — collusion (when compiled in) starts disabled, so a
    /// `RulesAgent::new(profile, exploit)` is byte-for-byte the pre-EPIC-70 bot.
    fn new(profile: BotProfile, exploit: Option<ExploitPuller>) -> Self {
        Self {
            profile,
            exploit,
            #[cfg(feature = "collusion")]
            collusion: None,
        }
    }

    /// The honest decider's base action, then — only when colluding and the
    /// partner's cards were read this turn — the style adjustment. A failed
    /// leak read means the turn is decided honestly (best-effort, per
    /// decision), so a colluder degrades gracefully to its underlying bot.
    /// Missing seat identities (either side's `player_id`) degrade the same
    /// way: no leak attempt, honest play.
    ///
    /// The `async` signature is stable across builds; with the `collusion`
    /// feature off there is no partner-card `await`, hence the targeted lint
    /// allow.
    #[cfg_attr(not(feature = "collusion"), allow(clippy::unused_async))]
    async fn choose(&self, state: &HandState, snapshot: &TableSnapshot<'_>) -> PkcoreAction {
        let base = RuleBasedDecider.decide(&self.profile, snapshot);
        #[cfg(feature = "collusion")]
        if let Some(colluder) = &self.collusion {
            // Both identities come off the same snapshot the dealer just sent,
            // so the pair agree on who is who without any out-of-band config.
            if let (Some(partner), Some(me)) = (
                state
                    .stacks
                    .iter()
                    .find(|s| s.name == colluder.config.partner),
                state.stacks.iter().find(|s| s.seat == state.seat),
            ) {
                if let (Some(partner_id), Some(my_id)) = (partner.player_id, me.player_id) {
                    let my_cards = Cards::forgiving_from_str(&state.hole_cards);
                    if let Some(partner_hole) = colluder
                        .source
                        .partner_hole(state.hand_no, state.seat, my_id, &my_cards, partner_id)
                        .await
                    {
                        return collude::strategy::apply_style(
                            colluder.config.style,
                            base,
                            snapshot,
                            partner.seat,
                            &partner_hole,
                        );
                    }
                }
            }
        }
        #[cfg(not(feature = "collusion"))]
        let _ = state;
        base
    }
}

#[async_trait]
impl PokerAgent for RulesAgent {
    /// Converts the gRPC-derived [`HandState`] into a pkcore [`TableSnapshot`]
    /// and delegates the decision to [`RuleBasedDecider`] via [`Self::choose`]
    /// (which applies any EPIC-70 collusion adjustment).
    ///
    /// When an [`ExploitPuller`] is attached, it first refreshes the per-player
    /// [`StatsRegistry`] from the service's completed-hand history (throttled to
    /// only re-ingest when a new hand has finished) and threads it — plus the
    /// derived `seat → player_id` map — onto the snapshot so the decider's
    /// `exploit` path engages against the real opponents.
    async fn decide(&self, state: &HandState) -> Decision {
        let action = if let Some(puller) = &self.exploit {
            puller.refresh().await;
            let guard = puller.state.lock().await;
            let snapshot = snapshot_with_stats(state, Some(&guard.registry), &guard.seat_ids);
            self.choose(state, &snapshot).await
        } else {
            let snapshot = hand_state_to_snapshot(state);
            self.choose(state, &snapshot).await
        };
        pkcore_action_to_decision(action)
    }
}

/// Pulls completed-hand history from the pkdealer service and maintains the
/// per-player [`StatsRegistry`] the decider's `exploit` path reads.
///
/// pkdealer splits hand *recording* (the service holds the authoritative
/// `HandCollection`) from *deciding* (this agent), and pkcore's `StatsRegistry`
/// can only be built by ingesting `HandHistory`. So the puller re-ingests the
/// service's collection on its own gRPC connection: a cheap `GetSessionInfo`
/// throttle re-exports and rebuilds only when the completed-hand count has
/// grown. See `docs/GUIDE_Bot_Decision_Capabilities.md` → "Closing the wire
/// gap" and pkcore EPIC-26a for the future serialize-the-registry path.
struct ExploitPuller {
    /// Dedicated connection for `GetSessionInfo` / `ExportSession`, separate
    /// from the play connection owned by `run_agent`. `tokio::Mutex` because the
    /// generated client needs `&mut self` per call and `decide` only has `&self`.
    client: Mutex<DealerServiceClient<Channel>>,
    /// Spectator token presented on `ExportSession` (the service gates it).
    spectator_token: String,
    /// The evolving registry, its `seat → player_id` map, and the last ingested
    /// hand count (the throttle watermark).
    state: Mutex<ExploitState>,
}

/// The mutable read the [`ExploitPuller`] maintains across decisions.
#[derive(Default)]
struct ExploitState {
    /// Per-player aggregates keyed by `player_id`, rebuilt on each ingest.
    registry: StatsRegistry,
    /// Latest `seat → player_id` mapping, so snapshot seats carry the id the
    /// decider looks up in `registry`.
    seat_ids: HashMap<u8, Uuid>,
    /// Completed-hand count at the last successful ingest; the `GetSessionInfo`
    /// throttle skips re-export while this is unchanged.
    last_hand_count: u32,
}

impl ExploitPuller {
    /// Refreshes [`ExploitState`] from the service, but only when a new hand has
    /// completed. Best-effort: any transport, auth, or parse failure leaves the
    /// existing stats in place (the decider then adjusts on slightly stale — or,
    /// on the very first hand, empty — reads rather than failing the decision).
    async fn refresh(&self) {
        let count = {
            let mut client = self.client.lock().await;
            match client.get_session_info(GetSessionInfoRequest {}).await {
                Ok(resp) => resp.into_inner().hand_count,
                Err(_) => return,
            }
        };
        if count == 0 || count == self.state.lock().await.last_hand_count {
            return;
        }

        let mut request = tonic::Request::new(ExportSessionRequest {
            format: RecordFormat::Json as i32,
            drain: false,
        });
        if let Ok(value) = self.spectator_token.parse() {
            request
                .metadata_mut()
                .insert(PLAYER_TOKEN_METADATA_KEY, value);
        }
        let payload = {
            let mut client = self.client.lock().await;
            match client.export_session(request).await {
                Ok(resp) => resp.into_inner().payload,
                Err(_) => return,
            }
        };
        let Ok(collection) = serde_json::from_str::<HandCollection>(&payload) else {
            return;
        };

        let mut state = self.state.lock().await;
        state.registry = build_registry(&collection);
        state.seat_ids = seat_ids_from_collection(&collection);
        state.last_hand_count = count;
        eprintln!(
            "[exploit] ingested {count} hands → {} players tracked",
            state.registry.len()
        );
    }
}

/// Builds a fresh [`StatsRegistry`] by ingesting every completed hand in
/// `collection`. Rebuilt wholesale (rather than incrementally) each refresh:
/// `ingest_collection` is idempotent over a full collection and keeps the map
/// exactly consistent with what the service holds.
fn build_registry(collection: &HandCollection) -> StatsRegistry {
    let mut registry = StatsRegistry::new();
    registry.ingest_collection(collection);
    registry
}

/// Derives the current `seat → player_id` map from a hand collection: the last
/// recorded hand that seats a player wins, so the map reflects who currently
/// occupies each seat. Delegates the latest-wins fold to [`latest_seat_ids`].
fn seat_ids_from_collection(collection: &HandCollection) -> HashMap<u8, Uuid> {
    latest_seat_ids(collection.hands().iter().flat_map(|hand| {
        hand.players
            .iter()
            .map(|player| (player.seat, player.player_id))
    }))
}

/// Folds `(seat, player_id)` pairs into a `seat → id` map, keeping the last id
/// seen per seat and skipping seats whose `player_id` is `None` (legacy hands).
/// Iteration order is the collection's hand order, so "last wins" == "most
/// recent hand wins".
fn latest_seat_ids(entries: impl Iterator<Item = (u8, Option<Uuid>)>) -> HashMap<u8, Uuid> {
    let mut map = HashMap::new();
    for (seat, id) in entries {
        if let Some(id) = id {
            map.insert(seat, id);
        }
    }
    map
}

/// Returns the logical position (0-based index) of `seat` within the sorted
/// list of occupied `seats`, or `None` when `seat` is not occupied.
///
/// pkcore's [`TableSnapshot::position`] expresses `dealer_button` and
/// `logical_seat` as indices into the sorted occupied-seat list (`0` = earliest
/// occupied seat), *not* as raw seat numbers, so raw seat numbers must be
/// projected through this before they are stored on the snapshot.
fn logical_index(sorted_seats: &[u8], seat: u8) -> Option<u8> {
    sorted_seats
        .iter()
        .position(|&s| s == seat)
        .and_then(|i| u8::try_from(i).ok())
}

/// Converts a [`HandState`] into a pkcore [`TableSnapshot`].
///
/// `current_bet` is approximated as `to_call` and `min_raise` as `big_blind`;
/// the runner's floor-raise correction handles any undersized raise amounts.
///
/// Each seat's `bet` and `is_active` flags are carried straight through from the
/// [`HandState`]. `is_active` matters for the equity path: pkcore counts *live*
/// opponents (folded / busted seats excluded) when it estimates multi-way
/// equity, so passing the real flag keeps `--equity fast|exact` from
/// systematically understating the hero's equity.
///
/// `dealer_button` and `logical_seat` are derived from `state.button_seat` and
/// `state.seat` by projecting both raw seat numbers into logical positions over
/// the sorted occupied-seat list (see [`logical_index`]). This makes
/// `TableSnapshot::position` resolve, which is what `ranges: position_aware`
/// consults. When `button_seat` is `None` (no button known), `dealer_button`
/// stays `None` and the decider falls back to the flat range.
///
/// `hole_cards` and `board` are parsed from their space-separated index
/// notation (e.g. `"Ah Kd"`) so that [`RuleBasedDecider`] runs its
/// hand-strength *equity* path. When a card field is empty (e.g. `board`
/// before the flop) or contains an unparseable token, [`Cards::forgiving_from_str`]
/// yields what it can — an empty [`Cards`] in the empty case — and the decider
/// falls back to its aggression-factor path for that decision.
///
/// `opponent_stats` is threaded straight onto the snapshot for the decider's
/// `exploit` path; pass `None` (see [`hand_state_to_snapshot`]) to preserve the
/// historical no-stats behavior. `seat_ids` supplies each seat's pkcore
/// `player_id` so `SeatInfo::id` matches the registry keys the decider looks up;
/// a seat absent from the map gets a fresh random id (which never matches a
/// registry entry, so it simply contributes no opponent read).
fn snapshot_with_stats<'a>(
    state: &HandState,
    opponent_stats: Option<&'a StatsRegistry>,
    seat_ids: &HashMap<u8, Uuid>,
) -> TableSnapshot<'a> {
    let stacks: Vec<SeatInfo> = state
        .stacks
        .iter()
        .map(|s| SeatInfo {
            id: seat_ids.get(&s.seat).copied().unwrap_or_else(Uuid::new_v4),
            seat: s.seat,
            name: s.name.clone(),
            #[allow(clippy::cast_possible_truncation)]
            chips: s.chips as usize,
            #[allow(clippy::cast_possible_truncation)]
            bet: s.bet as usize,
            is_active: s.is_active,
        })
        .collect();

    let phase = match state.street.as_str() {
        "flop" => GamePhase::BettingFlop,
        "turn" => GamePhase::BettingTurn,
        "river" => GamePhase::BettingRiver,
        _ => GamePhase::BettingPreFlop,
    };

    let seat_count = u8::try_from(stacks.len()).unwrap_or(u8::MAX);

    // Logical positions index into the sorted list of occupied seats, which is
    // what `TableSnapshot::position()` expects — not raw seat numbers.
    let mut occupied: Vec<u8> = state.stacks.iter().map(|s| s.seat).collect();
    occupied.sort_unstable();
    let logical_seat = logical_index(&occupied, state.seat);
    let dealer_button = state
        .button_seat
        .and_then(|button| logical_index(&occupied, button));

    #[allow(clippy::cast_possible_truncation)]
    TableSnapshot {
        seat: state.seat,
        phase,
        board: Cards::forgiving_from_str(&state.board),
        hole_cards: Cards::forgiving_from_str(&state.hole_cards),
        pot: state.pot as usize,
        // `HandState` does not carry the Rule 54-B pot (the real pot plus any
        // blind money owed but never posted), so the real pot stands in. It is
        // correct from the flop onward, where the two are equal by definition,
        // and can only understate the pre-flop pot-limit ceiling — an
        // over-large raise would be rejected by the table anyway.
        pot_limit_pot: state.pot as usize,
        to_call: state.to_call as usize,
        current_bet: state.to_call as usize,
        min_raise: state.big_blind as usize,
        // `HandState` carries no per-street raise count, so this is synthesized
        // like `min_raise` above. pkcore uses it only for the Fixed-Limit raise
        // cap; reporting 0 means `raise_bounds()` never sees the cap as full, so
        // a capped-out raise can still be proposed and then rejected by the
        // table. No effect on No-Limit or Pot-Limit. Carrying the real count on
        // the proto is tracked separately.
        raises_this_street: 0,
        // Rule 47-A's gate needs per-seat action history that the proto does
        // not carry. Reporting `false` means a gated seat can still propose a
        // raise, which the table then rejects — the same fail-open choice as
        // `raises_this_street` above.
        reopen_gated: false,
        my_chips: state.my_chips as usize,
        stacks,
        big_blind: state.big_blind as usize,
        betting_structure: BettingStructure::default(),
        bet_tier: BetTier::default(),
        checked_this_street: false,
        dealer_button,
        seat_count,
        logical_seat,
        opponent_stats,
    }
}

/// Converts a [`HandState`] into a pkcore [`TableSnapshot`] with no opponent
/// stats attached — the historical behavior used by every non-`exploit` bot and
/// by the unit tests. Delegates to [`snapshot_with_stats`] with `None` and an
/// empty seat-id map (so `SeatInfo::id`s are fresh randoms, as before).
fn hand_state_to_snapshot(state: &HandState) -> TableSnapshot<'static> {
    snapshot_with_stats(state, None, &HashMap::new())
}

/// Converts a pkcore [`PkcoreAction`] into an agent-core [`Decision`].
///
/// Bet and raise amounts are cast from `usize` to `u32`. In practice they
/// are bounded by the chip counts that originated as `u32` values, so no
/// truncation occurs.
#[allow(clippy::cast_possible_truncation)]
fn pkcore_action_to_decision(action: PkcoreAction) -> Decision {
    match action {
        PkcoreAction::Fold => Decision::Fold,
        PkcoreAction::Check => Decision::Check,
        PkcoreAction::Call => Decision::Call,
        PkcoreAction::Bet(n) => Decision::Bet(n as u32),
        PkcoreAction::Raise(n) => Decision::Raise(n as u32),
        PkcoreAction::AllIn => Decision::AllIn,
    }
}

/// Resolves a [`BotProfile`] from a short name or a YAML file path.
///
/// Recognized short names: `gto`, `tight_passive` / `tp`, `loose_aggressive` /
/// `lag`, `tight_aggressive` / `tag`, `loose_passive` / `lp`, `maniac`, `abc`,
/// `short_stack_ninja` / `ssn`, `joker`, `strong_all_on` / `strong`,
/// `weak_all_off` / `weak`. Any other value is treated as a path to a YAML file.
///
/// The `strong_all_on` / `weak_all_off` reference profiles are embedded into the
/// binary via [`include_str!`], so they resolve regardless of the working
/// directory (unlike a bare file path).
///
/// # Errors
///
/// Returns an error when the spec is not a known name and the file cannot be
/// read or parsed, or when an embedded reference profile fails to deserialize.
///
/// # Examples
///
/// ```
/// let p = pkdealer_agent_rules_load_profile("gto").unwrap();
/// ```
fn load_profile(spec: &str) -> Result<BotProfile, Box<dyn std::error::Error>> {
    let built_in = match spec {
        "gto" => Some(BotProfile::gto()),
        "tight_passive" | "tp" => Some(BotProfile::tight_passive()),
        "loose_aggressive" | "lag" => Some(BotProfile::loose_aggressive()),
        "tight_aggressive" | "tag" => Some(BotProfile::tight_aggressive()),
        "loose_passive" | "lp" => Some(BotProfile::loose_passive()),
        "maniac" => Some(BotProfile::maniac()),
        "abc" => Some(BotProfile::abc()),
        "short_stack_ninja" | "ssn" => Some(BotProfile::short_stack_ninja()),
        "joker" => Some(BotProfile::joker()),
        "strong_all_on" | "strong" => Some(BotProfile::from_yaml_str(include_str!(
            "../../../data/bots/strong_all_on.yaml"
        ))?),
        "weak_all_off" | "weak" => Some(BotProfile::from_yaml_str(include_str!(
            "../../../data/bots/weak_all_off.yaml"
        ))?),
        _ => None,
    };
    if let Some(p) = built_in {
        return Ok(p);
    }
    Ok(BotProfile::from_file(spec)?)
}

/// Builds an [`ExploitPuller`] when the profile enables the `exploit` knob,
/// opening a dedicated `ExportSession` connection to `endpoint`.
///
/// Returns `None` — leaving the `exploit` path inert and adding no extra
/// connection — in the common case (`exploit: off`) and, best-effort, when the
/// connection cannot be established (the agent still plays without opponent
/// stats rather than failing to start).
async fn connect_exploit_puller(
    profile: &BotProfile,
    endpoint: String,
    spectator_token: String,
    name: &str,
) -> Option<ExploitPuller> {
    if matches!(profile.decision.exploit, ExploitMode::Off) {
        return None;
    }
    match DealerServiceClient::connect(endpoint).await {
        Ok(client) => {
            eprintln!("[{name}] exploit enabled: pulling opponent stats via ExportSession");
            Some(ExploitPuller {
                client: Mutex::new(client),
                spectator_token,
                state: Mutex::new(ExploitState::default()),
            })
        }
        Err(e) => {
            eprintln!(
                "[{name}] exploit requested but the ExportSession connection failed ({e}); \
                 playing without opponent stats"
            );
            None
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let mut profile = match load_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to load profile {:?}: {e}", args.profile);
            process::exit(1);
        }
    };
    if apply_decision_overrides(&mut profile, &args) {
        eprintln!(
            "[{}] decision overrides applied: {:?}",
            args.name, profile.decision
        );
    }
    eprintln!("[{}] profile: {profile}", args.name);

    let exploit = connect_exploit_puller(
        &profile,
        args.endpoint.clone(),
        args.spectator_token.clone(),
        &args.name,
    )
    .await;

    // EPIC-70: resolve collusion before `args` fields are moved into the config.
    // Configuration errors are fatal (a colluder that cannot leak is a broken
    // experiment); absent flags leave `collusion` None and the agent honest.
    #[cfg(feature = "collusion")]
    let collusion = match validate_collusion(&args) {
        Ok(None) => None,
        Ok(Some(config)) => match connect_partner_source(&args, &config).await {
            Ok(source) => {
                eprintln!(
                    "[{}] COLLUSION ACTIVE: partner={} channel={:?} style={:?}",
                    args.name, config.partner, config.channel, config.style
                );
                Some(Colluder { config, source })
            }
            Err(e) => {
                eprintln!("[{}] collusion requested but {e}", args.name);
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("[{}] invalid collusion flags: {e}", args.name);
            process::exit(1);
        }
    };

    let config = AgentConfig {
        endpoint: args.endpoint,
        name: args.name,
        seat: args.seat,
        chips: args.chips,
        client_secret: args.client_secret,
    };

    #[cfg(feature = "collusion")]
    let mut agent = RulesAgent::new(profile, exploit);
    #[cfg(not(feature = "collusion"))]
    let agent = RulesAgent::new(profile, exploit);
    #[cfg(feature = "collusion")]
    {
        agent.collusion = collusion;
    }
    if let Err(e) = run_agent(agent, config).await {
        eprintln!("Agent error: {e}");
        process::exit(1);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use pkdealer_agent_core::SeatSnapshot;

    fn sample_state() -> HandState {
        HandState {
            seat: 0,
            hole_cards: "Ah Kd".to_string(),
            board: String::new(),
            pot: 300,
            to_call: 100,
            my_chips: 9_900,
            stacks: vec![
                SeatSnapshot {
                    seat: 0,
                    name: "alice".to_string(),
                    chips: 9_900,
                    bet: 100,
                    is_active: true,
                    player_id: None,
                },
                SeatSnapshot {
                    seat: 1,
                    name: "bob".to_string(),
                    chips: 10_000,
                    bet: 0,
                    is_active: true,
                    player_id: None,
                },
            ],
            big_blind: 100,
            street: "preflop".to_string(),
            action_history: vec![],
            button_seat: Some(0),
            hand_no: 0,
        }
    }

    #[test]
    fn hand_state_to_snapshot_numeric_fields() {
        let state = sample_state();
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.seat, 0);
        assert_eq!(snap.pot, 300);
        assert_eq!(snap.to_call, 100);
        assert_eq!(snap.my_chips, 9_900);
        assert_eq!(snap.big_blind, 100);
        assert_eq!(snap.min_raise, 100);
    }

    #[test]
    fn latest_seat_ids_keeps_most_recent_per_seat() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        // Seat 0 appears twice (hand order): the later id `c` wins. Seat 2 has
        // no `player_id` (legacy hand) and is skipped entirely.
        let map = latest_seat_ids(
            [(0u8, Some(a)), (1u8, Some(b)), (2u8, None), (0u8, Some(c))].into_iter(),
        );
        assert_eq!(map.get(&0), Some(&c));
        assert_eq!(map.get(&1), Some(&b));
        assert_eq!(map.get(&2), None);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn snapshot_with_stats_sets_seat_ids_and_attaches_registry() {
        let state = sample_state();
        let registry = StatsRegistry::new();
        let known = Uuid::new_v4();
        let mut seat_ids = HashMap::new();
        seat_ids.insert(1u8, known); // seat 0 deliberately absent → random fallback
        let snap = snapshot_with_stats(&state, Some(&registry), &seat_ids);
        let seat1 = snap
            .stacks
            .iter()
            .find(|s| s.seat == 1)
            .expect("seat 1 present");
        let seat0 = snap
            .stacks
            .iter()
            .find(|s| s.seat == 0)
            .expect("seat 0 present");
        assert_eq!(
            seat1.id, known,
            "mapped seat takes its player_id from the map"
        );
        assert_ne!(seat0.id, known, "unmapped seat gets a fresh random id");
        assert!(
            snap.opponent_stats.is_some(),
            "registry is threaded onto the snapshot"
        );
    }

    #[test]
    fn hand_state_to_snapshot_has_no_opponent_stats() {
        // The no-stats wrapper preserves the historical (pre-exploit) behavior.
        let snap = hand_state_to_snapshot(&sample_state());
        assert!(snap.opponent_stats.is_none());
    }

    #[test]
    fn build_registry_from_empty_collection_is_empty() {
        assert!(build_registry(&HandCollection::new()).is_empty());
    }

    #[test]
    fn logical_index_found_and_missing() {
        let occupied = [2u8, 4, 5, 7];
        assert_eq!(logical_index(&occupied, 2), Some(0));
        assert_eq!(logical_index(&occupied, 5), Some(2));
        assert_eq!(logical_index(&occupied, 7), Some(3));
        assert_eq!(logical_index(&occupied, 3), None);
    }

    #[test]
    fn hand_state_to_snapshot_threads_bet_and_active() {
        // Seat 1 has folded; seat 0 has 100 in the pot this street.
        let state = HandState {
            stacks: vec![
                SeatSnapshot {
                    seat: 0,
                    name: "alice".to_string(),
                    chips: 9_900,
                    bet: 100,
                    is_active: true,
                    player_id: None,
                },
                SeatSnapshot {
                    seat: 1,
                    name: "bob".to_string(),
                    chips: 10_000,
                    bet: 0,
                    is_active: false,
                    player_id: None,
                },
            ],
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.stacks[0].bet, 100);
        assert!(snap.stacks[0].is_active);
        assert_eq!(snap.stacks[1].bet, 0);
        assert!(!snap.stacks[1].is_active, "folded seat must not be active");
    }

    #[test]
    fn hand_state_to_snapshot_derives_logical_positions() {
        // Non-contiguous occupied seats: 3 (hero) and 6 (button).
        let state = HandState {
            seat: 3,
            button_seat: Some(6),
            stacks: vec![
                SeatSnapshot {
                    seat: 3,
                    name: "hero".to_string(),
                    chips: 9_000,
                    bet: 0,
                    is_active: true,
                    player_id: None,
                },
                SeatSnapshot {
                    seat: 6,
                    name: "villain".to_string(),
                    chips: 9_000,
                    bet: 0,
                    is_active: true,
                    player_id: None,
                },
            ],
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        // Sorted occupied = [3, 6]: hero is logical 0, button is logical 1.
        assert_eq!(snap.logical_seat, Some(0));
        assert_eq!(snap.dealer_button, Some(1));
        // With both set, position() resolves (enables position_aware ranges).
        assert!(snap.position().is_some());
    }

    #[test]
    fn hand_state_to_snapshot_no_button_leaves_position_unresolved() {
        let state = HandState {
            button_seat: None,
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.dealer_button, None);
        assert!(snap.position().is_none());
    }

    #[test]
    fn hand_state_to_snapshot_stacks() {
        let state = sample_state();
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.stacks.len(), 2);
        assert_eq!(snap.stacks[0].name, "alice");
        assert_eq!(snap.stacks[1].chips, 10_000);
    }

    #[test]
    fn hand_state_to_snapshot_phase_preflop() {
        let snap = hand_state_to_snapshot(&sample_state());
        assert_eq!(snap.phase, GamePhase::BettingPreFlop);
    }

    #[test]
    fn hand_state_to_snapshot_phase_flop() {
        let state = HandState {
            street: "flop".to_string(),
            ..sample_state()
        };
        assert_eq!(hand_state_to_snapshot(&state).phase, GamePhase::BettingFlop);
    }

    #[test]
    fn hand_state_to_snapshot_phase_turn() {
        let state = HandState {
            street: "turn".to_string(),
            ..sample_state()
        };
        assert_eq!(hand_state_to_snapshot(&state).phase, GamePhase::BettingTurn);
    }

    #[test]
    fn hand_state_to_snapshot_phase_river() {
        let state = HandState {
            street: "river".to_string(),
            ..sample_state()
        };
        assert_eq!(
            hand_state_to_snapshot(&state).phase,
            GamePhase::BettingRiver
        );
    }

    #[test]
    fn hand_state_to_snapshot_empty_board_is_default_cards() {
        let snap = hand_state_to_snapshot(&sample_state());
        assert!(snap.board.is_empty());
    }

    #[test]
    fn hand_state_to_snapshot_parses_hole_cards() {
        // sample_state() has hole_cards "Ah Kd"; the snapshot must carry both
        // cards so RuleBasedDecider runs its equity path rather than the
        // card-blind fallback path.
        let snap = hand_state_to_snapshot(&sample_state());
        assert_eq!(snap.hole_cards.len(), 2);
    }

    #[test]
    fn hand_state_to_snapshot_parses_board() {
        let state = HandState {
            board: "Ts 9s 8c".to_string(),
            street: "flop".to_string(),
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.board.len(), 3);
    }

    #[test]
    fn hand_state_to_snapshot_empty_board_stays_empty() {
        let state = HandState {
            board: String::new(),
            street: "preflop".to_string(),
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert!(snap.board.is_empty());
    }

    #[test]
    fn pkcore_action_to_decision_fold() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Fold),
            Decision::Fold
        );
    }

    #[test]
    fn pkcore_action_to_decision_check() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Check),
            Decision::Check
        );
    }

    #[test]
    fn pkcore_action_to_decision_call() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Call),
            Decision::Call
        );
    }

    #[test]
    fn pkcore_action_to_decision_bet() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Bet(200)),
            Decision::Bet(200)
        );
    }

    #[test]
    fn pkcore_action_to_decision_raise() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Raise(400)),
            Decision::Raise(400)
        );
    }

    #[test]
    fn pkcore_action_to_decision_all_in() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::AllIn),
            Decision::AllIn
        );
    }

    #[test]
    fn load_profile_builtin_gto() {
        let p = load_profile("gto").expect("gto should load");
        assert_eq!(p.name, "gto");
    }

    #[test]
    fn load_profile_builtin_abbreviations() {
        assert_eq!(load_profile("tp").expect("tp").name, "tight_passive");
        assert_eq!(load_profile("lag").expect("lag").name, "loose_aggressive");
        assert_eq!(load_profile("tag").expect("tag").name, "tight_aggressive");
        assert_eq!(load_profile("lp").expect("lp").name, "loose_passive");
        assert_eq!(load_profile("ssn").expect("ssn").name, "short_stack_ninja");
    }

    #[test]
    fn load_profile_all_builtins() {
        for name in [
            "gto",
            "tight_passive",
            "loose_aggressive",
            "tight_aggressive",
            "loose_passive",
            "maniac",
            "abc",
            "short_stack_ninja",
            "joker",
            "strong_all_on",
            "weak_all_off",
        ] {
            let p = load_profile(name).unwrap_or_else(|e| panic!("failed to load {name}: {e}"));
            assert!(!p.name.is_empty(), "{name} profile has empty name");
        }
    }

    #[test]
    fn load_profile_unknown_path_returns_error() {
        let result = load_profile("nonexistent_profile_xyz.yaml");
        assert!(result.is_err());
    }

    #[test]
    fn load_profile_strong_all_on_has_decision_knobs_on() {
        let p = load_profile("strong_all_on").expect("strong_all_on should load");
        assert_eq!(p.name, "strong_all_on");
        assert_eq!(p.decision.ranges, RangeMode::PositionAware);
        assert!(matches!(p.decision.equity, EquityMode::Fast { .. }));
        assert!(!p.decision.is_default());
    }

    #[test]
    fn load_profile_weak_all_off_has_default_decision() {
        let p = load_profile("weak_all_off").expect("weak_all_off should load");
        assert_eq!(p.name, "weak_all_off");
        assert_eq!(p.decision.equity, EquityMode::Off);
        assert_eq!(p.decision.ranges, RangeMode::Flat);
        assert!((p.decision.pot_odds.discipline - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn load_profile_strong_weak_aliases() {
        assert_eq!(
            load_profile("strong").expect("strong").name,
            "strong_all_on"
        );
        assert_eq!(load_profile("weak").expect("weak").name, "weak_all_off");
    }

    /// Builds an `Args` from a bare arg list; panics on parse failure.
    fn args_from(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("args should parse")
    }

    #[test]
    fn apply_decision_overrides_none_leaves_profile_unchanged() {
        let mut profile = BotProfile::gto();
        let before = profile.decision.clone();
        let args = args_from(["pkdealer_agent_rules"].as_slice());
        assert!(!apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision, before);
    }

    #[test]
    fn apply_decision_overrides_equity_fast_uses_samples() {
        let mut profile = BotProfile::gto();
        let args = args_from(&[
            "pkdealer_agent_rules",
            "--equity",
            "fast",
            "--equity-samples",
            "4000",
        ]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.equity, EquityMode::Fast { samples: 4_000 });
    }

    #[test]
    fn apply_decision_overrides_ranges_position_aware() {
        let mut profile = BotProfile::gto();
        let args = args_from(&["pkdealer_agent_rules", "--ranges", "position-aware"]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.ranges, RangeMode::PositionAware);
    }

    #[test]
    fn apply_decision_overrides_pot_odds_clamped() {
        let mut profile = BotProfile::gto();
        let args = args_from(&["pkdealer_agent_rules", "--pot-odds-discipline", "5.0"]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert!((profile.decision.pot_odds.discipline - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_decision_overrides_all_knobs() {
        let mut profile = BotProfile::gto();
        let args = args_from(&[
            "pkdealer_agent_rules",
            "--equity",
            "exact",
            "--ranges",
            "position-aware",
            "--pot-odds-discipline",
            "0.25",
            "--outs",
            "on",
            "--exploit",
            "heavy",
            "--preflop-charts",
            "hup",
        ]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.equity, EquityMode::Exact);
        assert_eq!(profile.decision.ranges, RangeMode::PositionAware);
        assert!((profile.decision.pot_odds.discipline - 0.25).abs() < f64::EPSILON);
        assert_eq!(profile.decision.outs, Toggle::On);
        assert_eq!(profile.decision.exploit, ExploitMode::Heavy);
        assert_eq!(profile.decision.preflop_charts, PreflopCharts::Hup);
    }

    #[test]
    fn apply_decision_overrides_can_disable_profile_knobs() {
        // strong_all_on ships with position-aware ranges + Monte-Carlo equity;
        // overrides must be able to force them back off.
        let mut profile = load_profile("strong_all_on").expect("strong_all_on");
        let args = args_from(&[
            "pkdealer_agent_rules",
            "--equity",
            "off",
            "--ranges",
            "flat",
        ]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.equity, EquityMode::Off);
        assert_eq!(profile.decision.ranges, RangeMode::Flat);
    }

    #[tokio::test]
    async fn rules_agent_legal_action_with_to_call() {
        let agent = RulesAgent::new(BotProfile::gto(), None);
        let state = sample_state();
        let decision = agent.decide(&state).await;
        assert!(
            matches!(
                decision,
                Decision::Fold | Decision::Call | Decision::Raise(_) | Decision::AllIn
            ),
            "unexpected decision when to_call > 0: {decision:?}"
        );
    }

    #[tokio::test]
    async fn rules_agent_legal_action_without_to_call() {
        let agent = RulesAgent::new(BotProfile::gto(), None);
        let state = HandState {
            to_call: 0,
            ..sample_state()
        };
        let decision = agent.decide(&state).await;
        assert!(
            matches!(decision, Decision::Check | Decision::Bet(_)),
            "unexpected decision when to_call == 0: {decision:?}"
        );
    }

    #[tokio::test]
    async fn rules_agent_zero_chips_checks() {
        let agent = RulesAgent::new(BotProfile::gto(), None);
        let state = HandState {
            my_chips: 0,
            to_call: 0,
            ..sample_state()
        };
        let decision = agent.decide(&state).await;
        assert_eq!(decision, Decision::Check);
    }

    #[tokio::test]
    async fn rules_agent_all_profiles_produce_actions() {
        for profile in BotProfile::default_profiles() {
            let agent = RulesAgent::new(profile, None);
            let _ = agent.decide(&sample_state()).await;
        }
    }

    #[cfg(feature = "collusion")]
    #[tokio::test]
    async fn rules_agent_without_collusion_behaves_honest() {
        // The wrapper is strictly additive: no config ⇒ the exact honest path.
        let agent = RulesAgent::new(BotProfile::gto(), None);
        let decision = agent.decide(&sample_state()).await;
        assert!(matches!(
            decision,
            Decision::Fold | Decision::Call | Decision::Raise(_) | Decision::AllIn
        ));
    }

    #[test]
    fn args_defaults() {
        let args =
            Args::try_parse_from(["pkdealer_agent_rules"]).expect("default args should parse");
        assert_eq!(args.endpoint, "http://127.0.0.1:50051");
        assert_eq!(args.name, "rules");
        assert!(args.seat.is_none());
        assert_eq!(args.chips, 10_000);
        assert_eq!(args.profile, "gto");
        assert!(args.client_secret.is_empty());
        // EPIC-36 override knobs default to "leave the profile alone".
        assert!(args.equity.is_none());
        assert_eq!(args.equity_samples, 2_000);
        assert!(args.ranges.is_none());
        assert!(args.pot_odds_discipline.is_none());
        assert!(args.outs.is_none());
        assert!(args.exploit.is_none());
        assert!(args.preflop_charts.is_none());
    }

    #[test]
    fn args_parse_decision_knobs() {
        let args = Args::try_parse_from([
            "pkdealer_agent_rules",
            "--equity",
            "fast",
            "--ranges",
            "position-aware",
            "--exploit",
            "light",
            "--preflop-charts",
            "solver",
        ])
        .expect("decision-knob args should parse");
        assert_eq!(args.equity, Some(EquityArg::Fast));
        assert_eq!(args.ranges, Some(RangesArg::PositionAware));
        assert_eq!(args.exploit, Some(ExploitArg::Light));
        assert_eq!(args.preflop_charts, Some(PreflopChartsArg::Solver));
    }

    #[test]
    fn args_with_profile_name() {
        let args = Args::try_parse_from([
            "pkdealer_agent_rules",
            "--name",
            "gto-bot",
            "--profile",
            "tight_passive",
        ])
        .expect("named args should parse");
        assert_eq!(args.name, "gto-bot");
        assert_eq!(args.profile, "tight_passive");
    }

    #[test]
    fn args_with_seat_and_chips() {
        let args = Args::try_parse_from(["pkdealer_agent_rules", "--seat", "2", "--chips", "5000"])
            .expect("seat/chips args should parse");
        assert_eq!(args.seat, Some(2));
        assert_eq!(args.chips, 5_000);
    }

    /// End-to-end wiring of [`Colluder`] through [`RulesAgent::choose`] with a
    /// stub [`collude::PartnerCardSource`] — the identity resolution and the
    /// graceful degradation when a seat carries no `player_id`.
    #[cfg(feature = "collusion")]
    mod colluder_wiring {
        use super::super::*;
        use pkdealer_agent_core::SeatSnapshot;

        /// A source that always leaks, so the test isolates `choose`'s wiring
        /// rather than any channel's behavior.
        struct AlwaysLeaks(Cards);

        #[async_trait]
        impl collude::PartnerCardSource for AlwaysLeaks {
            async fn partner_hole(
                &self,
                _hand_no: u32,
                _my_seat: u8,
                _my_id: Uuid,
                _my_cards: &Cards,
                _partner_id: Uuid,
            ) -> Option<Cards> {
                Some(self.0.clone())
            }
        }

        /// Hero (seat 0) holds KK on a full board facing the partner's bet;
        /// the partner (seat 1) is committed with the stronger AA, so `Dump`
        /// folds — provided both seats carry identities.
        fn dump_state(identities: bool) -> HandState {
            let id = |n: u128| {
                if identities {
                    Some(Uuid::from_u128(n))
                } else {
                    None
                }
            };
            HandState {
                seat: 0,
                hole_cards: "Kh Kd".to_string(),
                board: "2d 7c 9s Jd 3h".to_string(),
                pot: 600,
                to_call: 400,
                my_chips: 10_000,
                stacks: vec![
                    SeatSnapshot {
                        seat: 0,
                        name: "mallory_1".to_string(),
                        chips: 10_000,
                        bet: 0,
                        is_active: true,
                        player_id: id(1),
                    },
                    SeatSnapshot {
                        seat: 1,
                        name: "trudy_1".to_string(),
                        chips: 9_000,
                        bet: 400,
                        is_active: true,
                        player_id: id(2),
                    },
                ],
                big_blind: 100,
                street: "river".to_string(),
                action_history: vec![],
                button_seat: Some(0),
                hand_no: 7,
            }
        }

        /// A `Dump` colluder over the given channel, backed by the same
        /// channel-agnostic `AlwaysLeaks` source regardless of `channel`. Taking
        /// `channel` as a parameter lets a caller assert that the label alone
        /// does not move the decision (A/B equivalence).
        fn colluding_agent(channel: CollusionChannel) -> RulesAgent {
            let mut agent = RulesAgent::new(BotProfile::gto(), None);
            agent.collusion = Some(Colluder {
                config: CollusionConfig {
                    partner: "trudy_1".to_string(),
                    channel,
                    style: CollusionStyle::Dump,
                },
                source: Box::new(AlwaysLeaks(Cards::forgiving_from_str("As Ah"))),
            });
            agent
        }

        #[tokio::test]
        async fn colluder_applies_style_when_both_identities_are_known() {
            assert_eq!(
                colluding_agent(CollusionChannel::Peer)
                    .decide(&dump_state(true))
                    .await,
                Decision::Fold
            );
        }

        /// A/B equivalence, pinned behaviorally: the *only* difference between
        /// these two agents is `config.channel`, and the source is identical and
        /// channel-agnostic. `choose` reads partner cards through the
        /// `PartnerCardSource` trait object and must never branch on the channel
        /// label, so both variants must reach the same decision. This is the
        /// guard the type signature alone cannot give — if someone later wrote
        /// `if config.channel == Peer { … }` inside `choose`, this test fails
        /// while every other collusion test (all hardcoded to `Peer`) stays green.
        #[tokio::test]
        async fn channel_label_does_not_change_the_decision() {
            let spectator = colluding_agent(CollusionChannel::Spectator)
                .decide(&dump_state(true))
                .await;
            let peer = colluding_agent(CollusionChannel::Peer)
                .decide(&dump_state(true))
                .await;
            assert_eq!(spectator, peer);
            // Non-vacuity: both actually collude (Dump folds KK to the partner's
            // committed AA), so the equality above is over the *colluding* line,
            // not two identical honest fallbacks.
            assert_eq!(spectator, Decision::Fold);
        }

        #[tokio::test]
        async fn colluder_decides_honestly_without_seat_identities() {
            // Same table, same always-leaking source — but no `player_id` on
            // the wire means no peer exchange is possible, so the agent falls
            // back to the honest decision (never a fabricated identity).
            let honest = RulesAgent::new(BotProfile::gto(), None)
                .decide(&dump_state(false))
                .await;
            assert_eq!(
                colluding_agent(CollusionChannel::Peer)
                    .decide(&dump_state(false))
                    .await,
                honest
            );
            // Non-vacuity: the honest line is *not* the colluding one, so the
            // two assertions above genuinely discriminate.
            assert_ne!(honest, Decision::Fold);
        }
    }

    #[cfg(feature = "collusion")]
    mod collusion_args {
        use super::super::*;

        #[test]
        fn args_without_collude_with_yield_no_config() {
            let args = Args::try_parse_from(["pkdealer_agent_rules"]).expect("parse");
            assert!(validate_collusion(&args).expect("valid").is_none());
        }

        #[test]
        fn args_parse_collusion_flags() {
            let args = Args::try_parse_from([
                "pkdealer_agent_rules",
                "--name",
                "mallory_1",
                "--collude-with",
                "trudy_1",
                "--collusion-style",
                "dump",
            ])
            .expect("parse");
            let config = validate_collusion(&args).expect("valid").expect("config");
            assert_eq!(config.partner, "trudy_1");
            assert_eq!(config.channel, CollusionChannel::Spectator);
            assert_eq!(config.style, CollusionStyle::Dump);
        }

        #[test]
        fn peer_channel_resolves_to_the_backchannel() {
            // Phase 3: `peer` is accepted; `main` then dials the broker.
            let args = Args::try_parse_from([
                "pkdealer_agent_rules",
                "--collude-with",
                "trudy_1",
                "--collusion-channel",
                "peer",
                "--backchannel",
                "127.0.0.1:9099",
            ])
            .expect("parse");
            let config = validate_collusion(&args).expect("valid").expect("config");
            assert_eq!(config.channel, CollusionChannel::Peer);
            assert_eq!(config.partner, "trudy_1");
        }

        #[test]
        fn peer_channel_without_broker_address_is_rejected() {
            let args = Args::try_parse_from([
                "pkdealer_agent_rules",
                "--collude-with",
                "trudy_1",
                "--collusion-channel",
                "peer",
                "--backchannel",
                "",
            ])
            .expect("parse");
            assert!(validate_collusion(&args).is_err());
        }

        #[test]
        fn spectator_channel_ignores_a_missing_broker_address() {
            // Vector A never touches the broker, so an empty --backchannel is
            // not an error for it.
            let args = Args::try_parse_from([
                "pkdealer_agent_rules",
                "--collude-with",
                "trudy_1",
                "--backchannel",
                "",
            ])
            .expect("parse");
            let config = validate_collusion(&args).expect("valid").expect("config");
            assert_eq!(config.channel, CollusionChannel::Spectator);
        }

        #[test]
        fn colluding_with_yourself_is_rejected() {
            let args = Args::try_parse_from([
                "pkdealer_agent_rules",
                "--name",
                "x",
                "--collude-with",
                "x",
            ])
            .expect("parse");
            assert!(validate_collusion(&args).is_err());
        }
    }
}
