//! Vector B (`BackchannelClient`): shares this agent's hole cards with its
//! colluding partner over a broker the dealer never sees, and reads the
//! partner's, matched by `hand_no`. Best-effort — a missing/late partner share
//! yields `None`, and the agent decides honestly that turn (same graceful
//! degradation as Vector A).

use std::collections::HashMap;
use std::sync::Arc;

use pkcore::Forgiving;
use pkcore::cards::Cards;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::Mutex;
use uuid::Uuid;

/// One colluder's hole cards for one hand.
///
/// Wire-identical to `pkdealer_backchannel::CardShare` by contract, but
/// re-declared here rather than imported: `agent_core` must not depend on
/// the broker binary crate, which would invert the dependency direction and
/// drag the broker into every agent build.
///
/// # Examples
///
/// ```
/// use pkdealer_agent_core::backchannel::CardShare;
/// use uuid::Uuid;
///
/// let share = CardShare {
///     hand_no: 7,
///     seat: 2,
///     player_id: Uuid::from_u128(0xA1),
///     hole_cards: "Ah Kd".to_string(),
/// };
///
/// let json = serde_json::to_string(&share).unwrap();
/// let round_tripped: CardShare = serde_json::from_str(&json).unwrap();
/// assert_eq!(share, round_tripped);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardShare {
    /// Dealer hand number the cards belong to.
    pub hand_no: u32,
    /// Sharer's seat.
    pub seat: u8,
    /// Sharer's stable player UUID.
    pub player_id: Uuid,
    /// Hole cards in index notation, e.g. `"Ah Kd"`.
    pub hole_cards: String,
}

type Buffer = Arc<Mutex<HashMap<(Uuid, u32), String>>>;

/// A colluder's connection to the backchannel broker.
///
/// Dials the broker once via [`BackchannelClient::connect`], then publishes
/// this agent's hole cards each hand with [`BackchannelClient::publish`] and
/// reads its partner's with [`BackchannelClient::partner_cards`]. A
/// background task drains the socket and buffers every incoming share by
/// `(player_id, hand_no)`, so lookups never block on network I/O.
pub struct BackchannelClient {
    write: Mutex<OwnedWriteHalf>,
    buffer: Buffer,
}

impl BackchannelClient {
    /// Dials the broker and spawns a background reader that buffers incoming
    /// shares by `(player_id, hand_no)`.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message when the broker is unreachable.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), String> {
    /// use pkdealer_agent_core::backchannel::BackchannelClient;
    ///
    /// let client = BackchannelClient::connect("127.0.0.1:9999").await?;
    /// # let _ = client;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(addr: &str) -> Result<Self, String> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("backchannel connect {addr} failed: {e}"))?;
        let (read, write) = stream.into_split();
        let buffer: Buffer = Arc::new(Mutex::new(HashMap::new()));
        let reader_buffer = Arc::clone(&buffer);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(share) = serde_json::from_str::<CardShare>(&line) {
                    reader_buffer
                        .lock()
                        .await
                        .insert((share.player_id, share.hand_no), share.hole_cards);
                }
            }
        });
        Ok(Self {
            write: Mutex::new(write),
            buffer,
        })
    }

    /// Publishes this agent's cards for the current hand. Best-effort: write
    /// errors (e.g. a dropped broker connection) are swallowed rather than
    /// surfaced, since a failed share should not disrupt honest play.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), String> {
    /// use pkdealer_agent_core::backchannel::{BackchannelClient, CardShare};
    /// use uuid::Uuid;
    ///
    /// let client = BackchannelClient::connect("127.0.0.1:9999").await?;
    /// client
    ///     .publish(CardShare {
    ///         hand_no: 1,
    ///         seat: 0,
    ///         player_id: Uuid::from_u128(1),
    ///         hole_cards: "Ah Kd".to_string(),
    ///     })
    ///     .await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn publish(&self, share: CardShare) {
        if let Ok(mut line) = serde_json::to_string(&share) {
            line.push('\n');
            let _ = self.write.lock().await.write_all(line.as_bytes()).await;
        }
    }

    /// The partner's cards for `hand_no`, or `None` if not yet received.
    ///
    /// Best-effort: a missing or late partner share yields `None` rather
    /// than blocking, so the caller can fall back to deciding honestly.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), String> {
    /// use pkdealer_agent_core::backchannel::BackchannelClient;
    /// use uuid::Uuid;
    ///
    /// let client = BackchannelClient::connect("127.0.0.1:9999").await?;
    /// let cards = client.partner_cards(Uuid::from_u128(1), 1).await;
    /// assert!(cards.is_none());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn partner_cards(&self, partner_id: Uuid, hand_no: u32) -> Option<Cards> {
        self.buffer
            .lock()
            .await
            .get(&(partner_id, hand_no))
            .map(|s| Cards::forgiving_from_str(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkcore::Forgiving;

    async fn broker() -> String {
        // Minimal inline broadcast relay mirroring pkdealer_backchannel::Broker,
        // so agent_core needs no dependency on the broker binary crate.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (tx, _) = tokio::sync::broadcast::channel::<(u64, String)>(256);
            let mut id = 0u64;
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let (me, txc, mut rx) = (id, tx.clone(), tx.subscribe());
                id += 1;
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                    let (r, mut w) = sock.into_split();
                    let mut lines = BufReader::new(r).lines();
                    loop {
                        tokio::select! {
                            l = lines.next_line() => match l { Ok(Some(l)) => { let _ = txc.send((me, l)); }, _ => break },
                            b = rx.recv() => if let Ok((from, l)) = b { if from != me { let _ = w.write_all(format!("{l}\n").as_bytes()).await; } },
                        }
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn backchannel_matches_shares_by_hand_no() {
        let addr = broker().await;
        let trudy_id = uuid::Uuid::from_u128(0xA2);
        let mallory = BackchannelClient::connect(&addr).await.unwrap();
        let trudy = BackchannelClient::connect(&addr).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        trudy
            .publish(CardShare {
                hand_no: 7,
                seat: 1,
                player_id: trudy_id,
                hole_cards: "Qs Qc".into(),
            })
            .await;
        // Let the share traverse the broker + reader task.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            mallory.partner_cards(trudy_id, 7).await,
            Some(pkcore::cards::Cards::forgiving_from_str("Qs Qc"))
        );
        // Wrong hand → None (no cross-hand contamination).
        assert!(mallory.partner_cards(trudy_id, 8).await.is_none());
    }
}
