//! Hardware-tier recommendation (Architecture §8, PRD P4 zero-config first launch).
//!
//! On first launch a hardware probe auto-selects an LLM tier from installed RAM
//! (Architecture §8.319: "Tier 1 (4B) / Tier 2 (8B) / Tier 3 (14B)"):
//!
//! | Installed RAM | Tier   | LLM param range |
//! |---------------|--------|-----------------|
//! | ~8 GB         | Tier 1 | 3-4B            |
//! | ~16 GB        | Tier 2 | 7-8B            |
//! | 24-32 GB      | Tier 3 | 12-14B          |
//!
//! The auto choice is always overridable by the user (a manual-override hook —
//! [`TierRecommendation::with_override`]), because capability honesty (CLAUDE.md)
//! means the user, not the app, has final say on the memory/quality trade-off.
//!
//! The RAM *probe* itself is an OS concern and lives in an adapter; this module is
//! pure arithmetic over a supplied byte count so it stays trivially testable and
//! network-free.

use serde::{Deserialize, Serialize};

/// Bytes per GiB, for readable thresholds.
const GIB: u64 = 1024 * 1024 * 1024;

/// Threshold (inclusive floor) at or above which Tier 2 is chosen. Sits below the
/// nominal 16 GiB so machines that report slightly-less-than-16 (reserved/UMA RAM)
/// still land on Tier 2 as intended.
const TIER2_FLOOR_BYTES: u64 = 12 * GIB;

/// Threshold (inclusive floor) at or above which Tier 3 is chosen. Sits below the
/// nominal 24 GiB for the same margin reason.
const TIER3_FLOOR_BYTES: u64 = 22 * GIB;

/// Coarse capability bucket driving default model selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareTier {
    /// ~8 GB class → 3-4B LLM.
    Tier1,
    /// ~16 GB class → 7-8B LLM.
    Tier2,
    /// 24-32 GB class → 12-14B LLM.
    Tier3,
}

impl HardwareTier {
    /// Human-facing LLM parameter range for this tier.
    #[must_use]
    pub const fn llm_param_range(self) -> &'static str {
        match self {
            Self::Tier1 => "3-4B",
            Self::Tier2 => "7-8B",
            Self::Tier3 => "12-14B",
        }
    }

    /// A short, stable label (used in the health panel / settings — Architecture §11).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tier1 => "Tier 1 (compact)",
            Self::Tier2 => "Tier 2 (standard)",
            Self::Tier3 => "Tier 3 (large)",
        }
    }
}

/// The static profile a tier resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierProfile {
    /// The selected tier.
    pub tier: HardwareTier,
    /// LLM parameter range, e.g. `"7-8B"`.
    pub llm_param_range: &'static str,
    /// Display label.
    pub label: &'static str,
}

impl From<HardwareTier> for TierProfile {
    fn from(tier: HardwareTier) -> Self {
        Self {
            tier,
            llm_param_range: tier.llm_param_range(),
            label: tier.label(),
        }
    }
}

/// Map installed system RAM to the recommended [`HardwareTier`].
///
/// Monotonic: more RAM never selects a smaller tier. Machines below the Tier 1
/// nominal still receive Tier 1 (the smallest supported profile) — there is no
/// "unsupported" bucket; the smallest models must run everywhere.
#[must_use]
pub fn tier_for_ram(total_ram_bytes: u64) -> HardwareTier {
    if total_ram_bytes >= TIER3_FLOOR_BYTES {
        HardwareTier::Tier3
    } else if total_ram_bytes >= TIER2_FLOOR_BYTES {
        HardwareTier::Tier2
    } else {
        HardwareTier::Tier1
    }
}

/// An auto recommendation plus an optional user override (the manual-override hook).
///
/// [`effective`](Self::effective) resolves override-if-present, else the auto pick.
/// Persisted (as the tier enum) to the settings `selected model tiers` field
/// (Data Model §9.6 settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierRecommendation {
    /// Total installed RAM the auto pick was computed from.
    pub probed_ram_bytes: u64,
    /// The hardware-probe auto selection.
    pub auto: HardwareTier,
    /// User's explicit override, if any. When set it wins over `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_override: Option<HardwareTier>,
}

impl TierRecommendation {
    /// Build an auto recommendation from a probed RAM figure (no override yet).
    #[must_use]
    pub fn from_ram(total_ram_bytes: u64) -> Self {
        Self {
            probed_ram_bytes: total_ram_bytes,
            auto: tier_for_ram(total_ram_bytes),
            manual_override: None,
        }
    }

    /// Attach (or replace) a manual override — the user's explicit tier choice.
    #[must_use]
    pub fn with_override(mut self, tier: HardwareTier) -> Self {
        self.manual_override = Some(tier);
        self
    }

    /// Clear any manual override, reverting to the auto pick.
    pub fn clear_override(&mut self) {
        self.manual_override = None;
    }

    /// The tier actually in effect: override if set, else the auto pick.
    #[must_use]
    pub fn effective(&self) -> HardwareTier {
        self.manual_override.unwrap_or(self.auto)
    }

    /// The effective tier's static profile.
    #[must_use]
    pub fn profile(&self) -> TierProfile {
        self.effective().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gib(n: u64) -> u64 {
        n * GIB
    }

    #[test]
    fn eight_gb_selects_tier1() {
        assert_eq!(tier_for_ram(gib(8)), HardwareTier::Tier1);
        assert_eq!(tier_for_ram(gib(4)), HardwareTier::Tier1);
        assert_eq!(tier_for_ram(gib(11)), HardwareTier::Tier1);
    }

    #[test]
    fn sixteen_gb_selects_tier2() {
        assert_eq!(tier_for_ram(gib(16)), HardwareTier::Tier2);
        assert_eq!(tier_for_ram(gib(12)), HardwareTier::Tier2);
        assert_eq!(tier_for_ram(gib(21)), HardwareTier::Tier2);
    }

    #[test]
    fn twentyfour_to_thirtytwo_selects_tier3() {
        assert_eq!(tier_for_ram(gib(24)), HardwareTier::Tier3);
        assert_eq!(tier_for_ram(gib(32)), HardwareTier::Tier3);
        assert_eq!(tier_for_ram(gib(64)), HardwareTier::Tier3);
    }

    #[test]
    fn selection_is_monotonic() {
        let mut last = HardwareTier::Tier1;
        for g in 1..=128 {
            let t = tier_for_ram(gib(g));
            assert!(t >= last, "tier decreased at {g} GiB");
            last = t;
        }
    }

    #[test]
    fn param_ranges_map_per_spec() {
        assert_eq!(HardwareTier::Tier1.llm_param_range(), "3-4B");
        assert_eq!(HardwareTier::Tier2.llm_param_range(), "7-8B");
        assert_eq!(HardwareTier::Tier3.llm_param_range(), "12-14B");
    }

    #[test]
    fn override_wins_and_clears() {
        let mut rec = TierRecommendation::from_ram(gib(8));
        assert_eq!(rec.effective(), HardwareTier::Tier1);
        rec = rec.with_override(HardwareTier::Tier3);
        assert_eq!(rec.effective(), HardwareTier::Tier3);
        assert_eq!(rec.profile().llm_param_range, "12-14B");
        rec.clear_override();
        assert_eq!(rec.effective(), HardwareTier::Tier1);
    }

    #[test]
    fn recommendation_serde_round_trip() {
        let rec = TierRecommendation::from_ram(gib(16)).with_override(HardwareTier::Tier1);
        let json = serde_json::to_string(&rec).unwrap();
        let back: TierRecommendation = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
        assert_eq!(back.effective(), HardwareTier::Tier1);
    }
}
