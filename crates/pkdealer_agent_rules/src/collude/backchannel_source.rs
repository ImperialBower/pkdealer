//! Vector B adapter: makes `BackchannelClient` a
//! [`PartnerCardSource`](crate::collude::PartnerCardSource) by publishing this
//! agent's cards and reading the partner's, matched by hand.
//!
//! Unlike Vector A, nothing here touches the dealer: the cards travel over a
//! broker the service never sees. The decide path downstream is identical
//! either way, which is the whole point of the trait.

use pkcore::cards::Cards;
use pkdealer_agent_core::backchannel::{BackchannelClient, CardShare};
use uuid::Uuid;

/// Vector-B partner-card source: a live broker connection over which the
/// colluding pair swap hole cards each hand.
///
/// Carries no identity of its own — the agent's seat and UUID arrive per call
/// on [`PartnerCardSource::partner_hole`](crate::collude::PartnerCardSource::partner_hole),
/// resolved from the live table snapshot.
pub struct PeerSource {
    /// Connection to the backchannel broker.
    pub client: BackchannelClient,
}

#[async_trait::async_trait]
impl crate::collude::PartnerCardSource for PeerSource {
    /// Publishes this agent's cards for `hand_no`, then returns whatever the
    /// partner has published for the same hand — or `None` when the partner's
    /// share has not arrived yet (best-effort; the agent then decides
    /// honestly, exactly as a failed Vector-A read does).
    async fn partner_hole(
        &self,
        hand_no: u32,
        my_seat: u8,
        my_id: Uuid,
        my_cards: &Cards,
        partner_id: Uuid,
    ) -> Option<Cards> {
        self.client
            .publish(CardShare {
                hand_no,
                seat: my_seat,
                player_id: my_id,
                hole_cards: my_cards.to_string(),
            })
            .await;
        self.client.partner_cards(partner_id, hand_no).await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::collude::PartnerCardSource;
    use pkcore::Forgiving;

    /// Minimal line-relay standing in for `pkdealer_backchannel::Broker`, so
    /// the rules crate needs no dependency on the broker binary crate.
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
    async fn peer_source_publishes_own_cards_and_reads_the_partners() {
        let addr = broker().await;
        let (mallory_id, trudy_id) = (Uuid::from_u128(0xA1), Uuid::from_u128(0xA2));
        let mallory = PeerSource {
            client: BackchannelClient::connect(&addr).await.expect("connect"),
        };
        let trudy = PeerSource {
            client: BackchannelClient::connect(&addr).await.expect("connect"),
        };
        settle().await;

        // Trudy shares first; her partner (mallory) has not shared yet, so she
        // decides honestly this turn.
        let trudy_hole = Cards::forgiving_from_str("Qs Qc");
        assert!(
            trudy
                .partner_hole(7, 1, trudy_id, &trudy_hole, mallory_id)
                .await
                .is_none()
        );
        settle().await;

        // Mallory's call publishes hers and picks up trudy's — proving the
        // share was stamped with the *sharer's* id and this hand's number.
        let mallory_hole = Cards::forgiving_from_str("Ah Kd");
        assert_eq!(
            mallory
                .partner_hole(7, 0, mallory_id, &mallory_hole, trudy_id)
                .await,
            Some(trudy_hole.clone())
        );
        // A different hand never sees hand 7's share.
        assert!(
            mallory
                .partner_hole(8, 0, mallory_id, &mallory_hole, trudy_id)
                .await
                .is_none()
        );
        settle().await;

        // And the exchange is symmetric: trudy now reads mallory's hand-7 share.
        assert_eq!(
            trudy
                .partner_hole(7, 1, trudy_id, &trudy_hole, mallory_id)
                .await,
            Some(Cards::forgiving_from_str("Ah Kd"))
        );
    }
}
