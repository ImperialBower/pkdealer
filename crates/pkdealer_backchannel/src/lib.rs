#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
// unwrap/expect are the idiomatic failure report in tests; the ban above is
// for shipping code only (see CLAUDE.md → Error Handling).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! EPIC-70 Vector-B collusion backchannel: a broker that relays `CardShare`
//! lines between colluding agent processes. It is a dumb fan-out relay — it
//! broadcasts each received line to every *other* connected client and keeps
//! no state; clients filter for their partner. The dealer service never sees
//! these messages.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use uuid::Uuid;

/// One colluder's hole cards for one hand, as shared over the backchannel.
///
/// # Examples
///
/// ```
/// use pkdealer_backchannel::CardShare;
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

/// A fan-out relay for `CardShare` lines.
pub struct Broker {
    tx: broadcast::Sender<(u64, String)>,
}

impl Default for Broker {
    fn default() -> Self {
        Self::new()
    }
}

impl Broker {
    /// Creates an idle broker.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_backchannel::Broker;
    ///
    /// let _broker = Broker::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    /// Accepts connections until the listener errors, broadcasting each client's
    /// lines to every other client. Each connection is tagged with a unique id
    /// so the sender is excluded from its own broadcast.
    ///
    /// # Errors
    ///
    /// Returns the first fatal `accept` error.
    pub async fn serve(self, listener: TcpListener) -> std::io::Result<()> {
        let mut next_id: u64 = 0;
        loop {
            let (socket, _) = listener.accept().await?;
            let id = next_id;
            next_id += 1;
            let tx = self.tx.clone();
            let mut rx = self.tx.subscribe();
            tokio::spawn(async move {
                let (read_half, mut write_half) = socket.into_split();
                let mut lines = BufReader::new(read_half).lines();
                loop {
                    tokio::select! {
                        incoming = lines.next_line() => match incoming {
                            Ok(Some(line)) => { let _ = tx.send((id, line)); }
                            _ => break, // EOF or read error: drop this client
                        },
                        broadcasted = rx.recv() => match broadcasted {
                            Ok((from, line)) if from != id => {
                                if write_half.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {} // own message or lag: skip
                            Err(broadcast::error::RecvError::Closed) => break,
                        },
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    fn share(seat: u8, id: u128, cards: &str) -> CardShare {
        CardShare {
            hand_no: 7,
            seat,
            player_id: Uuid::from_u128(id),
            hole_cards: cards.to_string(),
        }
    }

    #[tokio::test]
    async fn broker_broadcasts_to_others_not_sender() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { Broker::new().serve(listener).await.unwrap() });

        let mut a = TcpStream::connect(addr).await.unwrap();
        let b = TcpStream::connect(addr).await.unwrap();
        let mut b = BufReader::new(b);
        // Give the broker a moment to register both clients.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let line = serde_json::to_string(&share(0, 0xA1, "Ah Kd")).unwrap();
        a.write_all(format!("{line}\n").as_bytes()).await.unwrap();

        // B receives A's share.
        let mut got = String::new();
        b.read_line(&mut got).await.unwrap();
        let parsed: CardShare = serde_json::from_str(got.trim()).unwrap();
        assert_eq!(parsed.hole_cards, "Ah Kd");
        assert_eq!(parsed.player_id, Uuid::from_u128(0xA1));

        // A does NOT receive its own share back (nothing to read within a beat).
        let mut a = BufReader::new(a);
        let mut echo = String::new();
        let r = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            a.read_line(&mut echo),
        )
        .await;
        assert!(
            r.is_err() || echo.is_empty(),
            "sender must not receive its own share"
        );
    }
}
