//! Tournament blind schedule for the demo arenas.
//!
//! Blinds escalate one level every `hands_per_level` hands, following the
//! 12-level [`BLIND_LEVELS`] table (identical to the `pkarena0-web`
//! schedule). The top level is **not** terminal: once it plays out its
//! `hands_per_level` hands the cycle wraps back to level 0, and the wrap is
//! signalled by [`BlindUpdate::reset_stacks`] so the caller can cap
//! over-large stacks back to the starting amount.
//!
//! This module is pure — it performs no I/O and holds no state. The caller
//! owns the `hands_completed` counter.

/// `(small_blind, big_blind)` for each level. Values match the
/// `pkarena0-web` `BLIND_LEVELS` array.
pub const BLIND_LEVELS: [(usize, usize); 12] = [
    (50, 100),
    (100, 200),
    (150, 300),
    (200, 400),
    (300, 600),
    (400, 800),
    (500, 1000),
    (750, 1500),
    (1000, 2000),
    (1500, 3000),
    (2000, 4000),
    (3000, 6000),
];

/// The blind decision for the hand that is about to start.
///
/// Produced by [`blind_update_for`]. `small_blind` / `big_blind` are the
/// blinds to post for the upcoming hand; `level` is its 0-based level index
/// into [`BLIND_LEVELS`]; `reset_stacks` is true exactly on the hand that
/// begins a fresh cycle (every player over the starting stack should be
/// capped back down before this hand is dealt).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlindUpdate {
    pub small_blind: usize,
    pub big_blind: usize,
    pub level: usize,
    pub reset_stacks: bool,
}

/// Decides the blinds for the upcoming hand from the number of hands already
/// completed.
///
/// `hands_completed` is the count of hands that have already finished, so the
/// hand about to start is hand number `hands_completed + 1`. Level 0 therefore
/// covers completed-counts `0..hands_per_level`.
///
/// A `hands_per_level` of 0 is treated as 1 so the function never divides by
/// zero.
///
/// # Examples
///
/// ```
/// use pkdealer_service::blind_schedule::blind_update_for;
///
/// // First hand of the tournament: level 0, 50/100, no reset.
/// let upd = blind_update_for(0, 20);
/// assert_eq!(upd.small_blind, 50);
/// assert_eq!(upd.big_blind, 100);
/// assert_eq!(upd.level, 0);
/// assert!(!upd.reset_stacks);
///
/// // Hand 241 (240 completed) wraps the cycle back to level 0 and resets.
/// let wrap = blind_update_for(240, 20);
/// assert_eq!(wrap.level, 0);
/// assert!(wrap.reset_stacks);
/// ```
#[must_use]
pub fn blind_update_for(hands_completed: u64, hands_per_level: usize) -> BlindUpdate {
    let per = hands_per_level.max(1);
    let cycle = BLIND_LEVELS.len() * per;
    let pos = (hands_completed % cycle as u64) as usize;
    let level = pos / per;
    let (small_blind, big_blind) = BLIND_LEVELS[level];
    BlindUpdate {
        small_blind,
        big_blind,
        level,
        reset_stacks: hands_completed > 0 && pos == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blind_update_for_first_hand() {
        let upd = blind_update_for(0, 20);
        assert_eq!(upd.level, 0);
        assert_eq!((upd.small_blind, upd.big_blind), (50, 100));
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_mid_level_no_reset() {
        let upd = blind_update_for(15, 20);
        assert_eq!(upd.level, 0);
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_level_boundary() {
        let upd = blind_update_for(20, 20);
        assert_eq!(upd.level, 1);
        assert_eq!((upd.small_blind, upd.big_blind), (100, 200));
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_last_level() {
        // 11 * 20 = 220 completed → level 11 (top), 3000/6000.
        let upd = blind_update_for(220, 20);
        assert_eq!(upd.level, 11);
        assert_eq!((upd.small_blind, upd.big_blind), (3000, 6000));
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_top_level_internal_hand() {
        // Still inside the top level (no wrap yet).
        let upd = blind_update_for(239, 20);
        assert_eq!(upd.level, 11);
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_cycle_wrap_resets() {
        let upd = blind_update_for(240, 20);
        assert_eq!(upd.level, 0);
        assert_eq!((upd.small_blind, upd.big_blind), (50, 100));
        assert!(upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_second_cycle_boundary() {
        // 260 completed = 240 (one full cycle) + 20 → level 1, no reset.
        let upd = blind_update_for(260, 20);
        assert_eq!(upd.level, 1);
        assert!(!upd.reset_stacks);
    }

    #[test]
    fn blind_update_for_zero_per_level_does_not_panic() {
        // per normalises to 1: cycle = 12, pos = 5 % 12 = 5, level 5.
        let upd = blind_update_for(5, 0);
        assert_eq!(upd.level, 5);
        assert!(!upd.reset_stacks);
    }
}
