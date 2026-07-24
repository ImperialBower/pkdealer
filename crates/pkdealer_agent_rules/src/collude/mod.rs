//! EPIC-70 collusion machinery (feature `collusion`): configuration, the two
//! card channels behind the [`PartnerCardSource`] trait — Vector A's spectator
//! leak and Vector B's peer backchannel — and the decision-adjusting
//! strategies. Cheating is strictly additive: with no [`CollusionConfig`], the
//! agent is byte-for-byte the honest bot.
//!

pub mod backchannel_source;
pub mod spectator;
pub mod strategy;

pub use strategy::CollusionStyle;

use pkcore::cards::Cards;
use uuid::Uuid;

/// A source of a colluding partner's hole cards for the current hand.
///
/// Both Vector A ([`spectator::SpectatorLeak`]) and Vector B
/// ([`backchannel_source::PeerSource`]) implement it, so the decide path is
/// byte-identical across channels — the Boss catches the behavior, not the
/// channel. Every implementation is best-effort: `None` means "no cards this
/// turn", and the caller decides honestly.
///
/// The caller passes its own identity (`my_seat`, `my_id`) and cards on every
/// call rather than binding them at construction: Vector A ignores them
/// entirely, and Vector B needs them only to stamp the share it publishes, so
/// neither implementation needs lazy state or interior mutability.
///
/// # Examples
///
/// ```text
/// // Any source is interchangeable behind the trait object:
/// let source: Box<dyn PartnerCardSource> = Box::new(peer_source);
/// let hole = source.partner_hole(hand_no, my_seat, my_id, &my_cards, partner_id).await;
/// ```
#[async_trait::async_trait]
pub trait PartnerCardSource: Send + Sync {
    /// The partner's hole cards this hand, or `None` (decide honestly).
    ///
    /// `hand_no` scopes the exchange to one dealer hand (no cross-hand
    /// contamination); `my_seat` / `my_id` / `my_cards` describe this agent's
    /// own position for channels that must publish to be read.
    async fn partner_hole(
        &self,
        hand_no: u32,
        my_seat: u8,
        my_id: Uuid,
        my_cards: &Cards,
        partner_id: Uuid,
    ) -> Option<Cards>;
}

/// How partner hole cards reach this agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollusionChannel {
    /// Vector A: read the partner's cards live via the spectator token.
    Spectator,
    /// Vector B: peer backchannel — cards are exchanged over a broker the
    /// dealer never sees (EPIC-70 Phase 3).
    Peer,
}

/// A resolved, validated collusion assignment for this agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollusionConfig {
    /// Partner's arena-composed display name (unique, e.g. `trudy_1`).
    pub partner: String,
    /// Card-leak channel.
    pub channel: CollusionChannel,
    /// Decision-adjustment strategy.
    pub style: CollusionStyle,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod ab_equivalence {
    use super::*;
    use crate::collude::backchannel_source::PeerSource;
    use crate::hand_state_to_snapshot;
    use pkcore::Forgiving;
    use pkcore::bot::player_action::PlayerAction;
    use pkcore::cards::Cards;
    use pkdealer_agent_core::backchannel::BackchannelClient;
    use pkdealer_agent_core::{HandState, SeatSnapshot};
    use uuid::Uuid;

    /// A [`PartnerCardSource`] that always yields the same cards, standing in
    /// for either channel — the point being that the decide path cannot tell
    /// them apart.
    struct Fixed(Cards);

    #[async_trait::async_trait]
    impl PartnerCardSource for Fixed {
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

    #[tokio::test]
    async fn two_sources_same_partner_hole_are_interchangeable() {
        // Any two PartnerCardSources returning the same cards feed apply_style
        // identically — the decision path is channel-agnostic.
        let cards = Cards::forgiving_from_str("As Ac");
        let a = Fixed(cards.clone());
        let b = Fixed(cards.clone());
        let id = Uuid::from_u128(1);
        let ha = a.partner_hole(7, 0, id, &cards, Uuid::from_u128(2)).await;
        let hb = b.partner_hole(7, 0, id, &cards, Uuid::from_u128(2)).await;
        assert_eq!(ha, hb);
    }

    /// Minimal line-relay standing in for `pkdealer_backchannel::Broker`,
    /// mirroring `backchannel_source::tests::broker` — duplicated here (rather
    /// than shared) because that helper lives in a private `#[cfg(test)]`
    /// module and this test wants a *real* Vector-B channel, not a stub.
    async fn broker() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr").to_string();
        tokio::spawn(async move {
            let (tx, _) = tokio::sync::broadcast::channel::<(u64, String)>(64);
            let mut next_id = 0u64;
            while let Ok((sock, _)) = listener.accept().await {
                let (me, sender, mut inbox) = (next_id, tx.clone(), tx.subscribe());
                next_id += 1;
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                    let (read, mut write) = sock.into_split();
                    let mut lines = BufReader::new(read).lines();
                    loop {
                        tokio::select! {
                            line = lines.next_line() => match line {
                                Ok(Some(line)) => { let _ = sender.send((me, line)); }
                                _ => break,
                            },
                            relayed = inbox.recv() => if let Ok((from, line)) = relayed {
                                if from != me {
                                    let _ = write.write_all(format!("{line}\n").as_bytes()).await;
                                }
                            },
                        }
                    }
                });
            }
        });
        addr
    }

    async fn settle() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn vector_a_and_b_same_signature() {
        // `Dump` is the only style whose `chip_dump` actually reads
        // `partner_hole` (see strategy.rs) — Soft and Whipsaw never look at
        // it, so only Dump can prove the two channels are truly
        // interchangeable rather than passing by construction.
        //
        // Scenario: hero KK on a full board, partner committed. A stronger
        // committed partner (AA) forces a fold; this scenario is shared with
        // `strategy::tests::chip_dump_folds_strong_to_partner`.
        let state = HandState {
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
                    player_id: Some(Uuid::from_u128(1)),
                },
                SeatSnapshot {
                    seat: 1,
                    name: "trudy_1".to_string(),
                    chips: 9_000,
                    bet: 400,
                    is_active: true,
                    player_id: Some(Uuid::from_u128(2)),
                },
            ],
            big_blind: 100,
            street: "river".to_string(),
            action_history: vec![],
            button_seat: Some(0),
            hand_no: 7,
        };
        let snapshot = hand_state_to_snapshot(&state);
        let mine = Cards::forgiving_from_str(&state.hole_cards);
        let (mallory_id, trudy_id) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let strong_partner_hole = Cards::forgiving_from_str("As Ah");

        // Vector B, for real: a `PeerSource` talking to a real loopback
        // broker (same pattern as `backchannel_source::tests`) — not a stub
        // standing in for the channel, an actual second channel.
        let addr = broker().await;
        let trudy_peer = PeerSource {
            client: BackchannelClient::connect(&addr).await.expect("trudy connect"),
        };
        let mallory_peer = PeerSource {
            client: BackchannelClient::connect(&addr)
                .await
                .expect("mallory connect"),
        };
        settle().await;
        // Trudy publishes her (strong) cards over the wire first.
        trudy_peer
            .partner_hole(
                state.hand_no,
                1,
                trudy_id,
                &strong_partner_hole,
                mallory_id,
            )
            .await;
        settle().await;
        let via_peer = mallory_peer
            .partner_hole(state.hand_no, state.seat, mallory_id, &mine, trudy_id)
            .await
            .expect("peer relay delivered trudy's cards");
        assert_eq!(via_peer, strong_partner_hole);

        // The stub source, delivering the identical cards.
        let via_stub = Fixed(strong_partner_hole.clone())
            .partner_hole(state.hand_no, state.seat, mallory_id, &mine, trudy_id)
            .await
            .expect("fixed source always leaks");
        assert_eq!(via_stub, strong_partner_hole);

        // Same cards, two genuinely different channels: the `Dump` decision
        // must agree, because `chip_dump` cannot tell which one delivered
        // `partner_hole`.
        let action_peer = strategy::apply_style(
            CollusionStyle::Dump,
            PlayerAction::Call,
            &snapshot,
            1,
            &via_peer,
        );
        let action_stub = strategy::apply_style(
            CollusionStyle::Dump,
            PlayerAction::Call,
            &snapshot,
            1,
            &via_stub,
        );
        assert_eq!(action_peer, action_stub);
        // And the adjustment actually fired (a vacuous pass would be worthless).
        assert_eq!(action_peer, PlayerAction::Fold);

        // Discrimination guard: a source delivering *different* partner
        // cards must yield a *different* action, or this test could regress
        // to vacuous again without anyone noticing. A partner hand that
        // loses to hero's pair of kings leaves the honest base action alone.
        let weak_partner_hole = Cards::forgiving_from_str("4d 5c");
        let action_weak = strategy::apply_style(
            CollusionStyle::Dump,
            PlayerAction::Call,
            &snapshot,
            1,
            &weak_partner_hole,
        );
        assert_ne!(action_weak, action_peer);
        assert_eq!(action_weak, PlayerAction::Call);
    }
}
