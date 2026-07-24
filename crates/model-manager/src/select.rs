//! Language- and hardware-aware model selection (Architecture §8).
//!
//! Given a catalog of [`ModelManifest`]s, the user's language, and the probed
//! [`HardwareTier`], pick the best model to **download on demand** per role. A
//! language-specialized variant (e.g. Whisper `*.en`, `LanguageSupport::Only`) is
//! preferred over a multilingual one *when it covers the user's language* because
//! it is smaller/faster/more accurate there; otherwise the best multilingual model
//! that fits the machine is chosen. This is how "download the model for *his*
//! language" works: the app knows the user's language (OS locale or an explicit
//! setting), asks here for the right pack, and the registry fetches it.

use crate::manifest::{LanguageSupport, ModelEngine, ModelManifest};
use crate::tier::HardwareTier;

/// The user's language preference driving model selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguagePreference {
    /// Primary BCP-47 language tag, e.g. `"en"`, `"fr"`, `"hi-IN"`.
    pub primary: String,
    /// Keep a multilingual STT even when a specialized model exists, so speech in
    /// other languages still transcribes (Whisper auto-detect). Defaults to `true`.
    pub auto_detect: bool,
}

impl LanguagePreference {
    /// A preference for `primary`, with multilingual auto-detect kept on.
    #[must_use]
    pub fn new(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            auto_detect: true,
        }
    }
}

/// Coarse rank for tier-floor comparison without relying on the enum's ordering.
const fn tier_rank(t: HardwareTier) -> u8 {
    match t {
        HardwareTier::Tier1 => 1,
        HardwareTier::Tier2 => 2,
        HardwareTier::Tier3 => 3,
    }
}

/// Does `m` run acceptably on a machine at `tier` (respecting its `min_tier` floor)?
fn fits_tier(m: &ModelManifest, tier: HardwareTier) -> bool {
    match m.min_hardware.min_tier {
        Some(min) => tier_rank(tier) >= tier_rank(min),
        None => true,
    }
}

/// Pick the best model of `engine` for `lang` on a `tier` machine, or `None` if the
/// catalog has nothing suitable. When `prefer_specialized` is true a language-only
/// model that covers `lang` wins over a multilingual one; either way the largest
/// artifact that fits the tier is chosen (best quality within the hardware budget).
#[must_use]
pub fn select_for_language<'a>(
    catalog: &'a [ModelManifest],
    engine: ModelEngine,
    tier: HardwareTier,
    lang: &str,
    prefer_specialized: bool,
) -> Option<&'a ModelManifest> {
    catalog
        .iter()
        .filter(|m| m.engine == engine && fits_tier(m, tier) && m.languages.supports(lang))
        .max_by_key(|m| {
            // Preference class first, then artifact size as a within-budget quality
            // proxy. `Only` (specialized) scores 1 when we prefer it, else 0.
            let specialized = matches!(m.languages, LanguageSupport::Only(_));
            let class = u8::from(prefer_specialized == specialized);
            (class, m.size_bytes)
        })
}

/// The recommended download pack for a user: an STT model, an LLM, and an embedder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection<'a> {
    /// Speech-to-text model, if the catalog offers a fit.
    pub stt: Option<&'a ModelManifest>,
    /// Language model.
    pub llm: Option<&'a ModelManifest>,
    /// Text embedder for semantic search.
    pub embedder: Option<&'a ModelManifest>,
}

/// Choose the full pack for `pref` on a `tier` machine. For STT with `auto_detect`
/// on, a multilingual model is preferred so any spoken language transcribes;
/// otherwise a language-specialized STT (e.g. Whisper `*.en`) is preferred. LLMs
/// and embedders are chosen to cover the user's language (multilingual wins unless
/// a specialized model covers exactly that language).
#[must_use]
pub fn select_pack<'a>(
    catalog: &'a [ModelManifest],
    tier: HardwareTier,
    pref: &LanguagePreference,
) -> Selection<'a> {
    let lang = &pref.primary;
    Selection {
        stt: select_for_language(catalog, ModelEngine::Stt, tier, lang, !pref.auto_detect),
        llm: select_for_language(catalog, ModelEngine::Llm, tier, lang, true),
        embedder: select_for_language(catalog, ModelEngine::Embedder, tier, lang, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{HardwareRequirements, ModelManifest};
    use app_domain::ModelId;

    fn m(
        id: &str,
        engine: ModelEngine,
        size: u64,
        langs: LanguageSupport,
        min: Option<HardwareTier>,
    ) -> ModelManifest {
        ModelManifest {
            id: ModelId(id.to_string()),
            engine,
            family: "whisper".into(),
            variant: "x".into(),
            version: "1".into(),
            quantization: None,
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            size_bytes: size,
            url: "file:///x".into(),
            min_hardware: HardwareRequirements {
                min_ram_bytes: 0,
                min_tier: min,
                disk_overhead_bytes: 0,
            },
            languages: langs,
            signature: "sig".into(),
        }
    }

    fn catalog() -> Vec<ModelManifest> {
        vec![
            m(
                "whisper-small",
                ModelEngine::Stt,
                500,
                LanguageSupport::Multilingual,
                None,
            ),
            m(
                "whisper-small-en",
                ModelEngine::Stt,
                480,
                LanguageSupport::Only(vec!["en".into()]),
                None,
            ),
            m(
                "whisper-medium",
                ModelEngine::Stt,
                1500,
                LanguageSupport::Multilingual,
                Some(HardwareTier::Tier3),
            ),
            m(
                "qwen3-8b",
                ModelEngine::Llm,
                4800,
                LanguageSupport::Multilingual,
                Some(HardwareTier::Tier2),
            ),
            m(
                "qwen3-4b",
                ModelEngine::Llm,
                2400,
                LanguageSupport::Multilingual,
                Some(HardwareTier::Tier1),
            ),
            m(
                "bge-m3",
                ModelEngine::Embedder,
                300,
                LanguageSupport::Multilingual,
                None,
            ),
        ]
    }

    #[test]
    fn english_stt_prefers_specialized_when_not_auto_detecting() {
        let cat = catalog();
        let got = select_for_language(&cat, ModelEngine::Stt, HardwareTier::Tier2, "en-US", true);
        assert_eq!(got.unwrap().id.0, "whisper-small-en");
    }

    #[test]
    fn other_language_falls_back_to_multilingual() {
        let cat = catalog();
        // No French-specialized STT → the multilingual model that fits Tier2.
        let got = select_for_language(&cat, ModelEngine::Stt, HardwareTier::Tier2, "fr", true);
        assert_eq!(got.unwrap().id.0, "whisper-small");
    }

    #[test]
    fn tier_floor_is_respected() {
        let cat = catalog();
        // whisper-medium needs Tier3; on Tier1 the multilingual pick is small.
        let got = select_for_language(&cat, ModelEngine::Stt, HardwareTier::Tier1, "fr", true);
        assert_eq!(got.unwrap().id.0, "whisper-small");
        // LLM on Tier1 cannot use the 8B (Tier2 floor) → the 4B.
        let llm = select_for_language(&cat, ModelEngine::Llm, HardwareTier::Tier1, "de", true);
        assert_eq!(llm.unwrap().id.0, "qwen3-4b");
    }

    #[test]
    fn auto_detect_pack_keeps_multilingual_stt() {
        let cat = catalog();
        let pref = LanguagePreference::new("en"); // auto_detect = true
        let pack = select_pack(&cat, HardwareTier::Tier2, &pref);
        // auto-detect on → multilingual STT even for English, so other languages transcribe.
        assert_eq!(pack.stt.unwrap().id.0, "whisper-small");
        assert_eq!(pack.llm.unwrap().id.0, "qwen3-8b");
        assert_eq!(pack.embedder.unwrap().id.0, "bge-m3");
    }

    #[test]
    fn english_only_user_gets_specialized_stt() {
        let cat = catalog();
        let pref = LanguagePreference {
            primary: "en".into(),
            auto_detect: false,
        };
        let pack = select_pack(&cat, HardwareTier::Tier2, &pref);
        assert_eq!(pack.stt.unwrap().id.0, "whisper-small-en");
    }

    #[test]
    fn unsupported_role_is_none() {
        let cat = catalog();
        assert!(
            select_for_language(&cat, ModelEngine::Reranker, HardwareTier::Tier3, "en", true)
                .is_none()
        );
    }
}
