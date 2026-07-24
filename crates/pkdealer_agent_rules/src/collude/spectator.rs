//! Vector A (`SpectatorLeak`): reads the partner's live hole cards through
//! the spectator token, on a dedicated second connection — the same
//! connection + token-injection pattern as `ExploitPuller`, but reading
//! *live* card state on every decision instead of completed-hand history.
//!
//! **Honor filter (load-bearing for A/B equivalence):** the spectator view
//! exposes *every* seat's cards; [`extract_partner_cards`] discards all but
//! the partner's at ingest, collapsing Vector A's information position to
//! Vector B's. See EPIC-70 → Scope.

use pkcore::Forgiving;
use pkcore::cards::Cards;
use pkdealer_proto::dealer::dealer_service_client::DealerServiceClient;
use pkdealer_proto::dealer::{GetStatusRequest, TableStatus};
use tokio::sync::Mutex;
use tonic::transport::Channel;

use crate::PLAYER_TOKEN_METADATA_KEY;

/// Live partner-card reader over the spectator token (Vector A).
pub struct SpectatorLeak {
    /// Dedicated connection, separate from the play connection.
    client: Mutex<DealerServiceClient<Channel>>,
    /// Spectator token injected into request metadata.
    token: String,
    /// Partner's arena-composed display name.
    partner: String,
}

impl SpectatorLeak {
    /// Opens the dedicated spectator connection.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message when the endpoint is unreachable —
    /// the caller exits: a colluder that cannot leak is a broken experiment.
    pub async fn connect(endpoint: String, token: String, partner: String) -> Result<Self, String> {
        match DealerServiceClient::connect(endpoint).await {
            Ok(client) => Ok(Self {
                client: Mutex::new(client),
                token,
                partner,
            }),
            Err(e) => Err(format!("spectator-leak connection failed: {e}")),
        }
    }

    /// Reads the partner's current hole cards, or `None` when unavailable
    /// (between hands, transport error, redacted view). Best-effort per
    /// decision — a missed read means the agent decides honestly this turn.
    ///
    /// The inherent name is distinct from the
    /// [`PartnerCardSource::partner_hole`](crate::collude::PartnerCardSource::partner_hole)
    /// trait method that wraps it, because Vector A needs none of that
    /// method's arguments: it reads the partner's seat live rather than
    /// trading cards with it.
    pub async fn read_partner_live(&self) -> Option<Cards> {
        let mut request = tonic::Request::new(GetStatusRequest {});
        request
            .metadata_mut()
            .insert(PLAYER_TOKEN_METADATA_KEY, self.token.parse().ok()?);
        let status = {
            let mut client = self.client.lock().await;
            client.get_status(request).await.ok()?.into_inner().status?
        };
        extract_partner_cards(&status, &self.partner).map(|s| Cards::forgiving_from_str(&s))
    }
}

#[async_trait::async_trait]
impl crate::collude::PartnerCardSource for SpectatorLeak {
    /// Vector A ignores every argument: the partner's cards are read live off
    /// the spectator view, so there is nothing to publish and no hand to match
    /// against — the view is already current.
    async fn partner_hole(
        &self,
        _hand_no: u32,
        _my_seat: u8,
        _my_id: uuid::Uuid,
        _my_cards: &Cards,
        _partner_id: uuid::Uuid,
    ) -> Option<Cards> {
        self.read_partner_live().await
    }
}

/// The honor filter: pulls **only** the named partner's cards out of a
/// spectator-visible status, discarding every other seat's at ingest.
///
/// Returns `None` when the partner is not seated or carries no cards (between
/// hands, or a redacted view) — never a fabricated read.
pub(crate) fn extract_partner_cards(status: &TableStatus, partner: &str) -> Option<String> {
    status
        .seats
        .iter()
        .find(|s| s.player_name == partner)
        .map(|s| s.cards.clone())
        .filter(|cards| !cards.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkdealer_proto::dealer::{SeatInfo, TableStatus};

    fn seat(name: &str, cards: &str) -> SeatInfo {
        SeatInfo {
            seat_number: 0,
            player_name: name.to_string(),
            cards: cards.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn extracts_partner_cards_only() {
        let status = TableStatus {
            seats: vec![
                seat("mallory_1", "Ah Kd"),
                seat("trudy_1", "Qs Qc"),
                seat("gto_1", "7d 2c"),
            ],
            ..Default::default()
        };
        // Honor filter: only the partner's cards come out, ever.
        assert_eq!(
            extract_partner_cards(&status, "trudy_1").as_deref(),
            Some("Qs Qc")
        );
    }

    #[test]
    fn absent_partner_yields_none() {
        let status = TableStatus {
            seats: vec![seat("gto_1", "7d 2c")],
            ..Default::default()
        };
        assert!(extract_partner_cards(&status, "trudy_1").is_none());
    }

    #[test]
    fn empty_cards_yield_none() {
        // Between hands (or if the token was rejected and cards were redacted)
        // the partner's seat carries no cards — never fabricate a read.
        let status = TableStatus {
            seats: vec![seat("trudy_1", "")],
            ..Default::default()
        };
        assert!(extract_partner_cards(&status, "trudy_1").is_none());
    }
}
