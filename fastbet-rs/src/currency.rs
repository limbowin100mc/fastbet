//! Currency artwork shared by the casinos and the UI: each casino's own coin and fiat icons, taken
//! from its site (assets/currencies/<casino>.json). Stake's set (coins plus ~150 country flags) is
//! the fallback for every casino. The SVGs were checked element by element when they were taken:
//! shapes, gradients, masks and filters only, no scripts, styles or outside links.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::casino::CasinoId;

#[derive(Deserialize, Default)]
struct Icons {
    coins: HashMap<String, String>,
    /// Fiat icons: Stake draws flags, Shuffle currency signs.
    #[serde(alias = "flags")]
    fiats: HashMap<String, String>,
}

impl Icons {
    fn get(&self, code: &str) -> Option<&str> {
        self.coins.get(code).or_else(|| self.fiats.get(code)).map(String::as_str)
    }
}

fn stake() -> &'static Icons {
    static ICONS: OnceLock<Icons> = OnceLock::new();
    ICONS.get_or_init(|| serde_json::from_str(include_str!("../assets/currencies/stake.json")).unwrap_or_default())
}

fn shuffle() -> &'static Icons {
    static ICONS: OnceLock<Icons> = OnceLock::new();
    ICONS.get_or_init(|| serde_json::from_str(include_str!("../assets/currencies/shuffle.json")).unwrap_or_default())
}

/// Coins the sites draw from a picture (JPEG or WebP) rather than a vector icon.
fn picture(casino: CasinoId, code: &str) -> Option<&'static [u8]> {
    Some(match (casino, code) {
        (CasinoId::Shuffle, "bonk") => include_bytes!("../assets/currencies/shuffle/bonk.jpg"),
        (CasinoId::Shuffle, "pump") => include_bytes!("../assets/currencies/shuffle/pump.jpg"),
        (CasinoId::Shuffle, "wif") => include_bytes!("../assets/currencies/shuffle/wif.jpg"),
        (CasinoId::Shuffle, "shib") => include_bytes!("../assets/currencies/shuffle/shib.webp"),
        (CasinoId::Shuffle, "trump") => include_bytes!("../assets/currencies/shuffle/trump.webp"),
        (_, "trump") => include_bytes!("../assets/currencies/stake/trump.webp"),
        _ => return None,
    })
}

pub enum Art {
    Svg(&'static str),
    /// JPEG or WebP, drawn as a circle.
    Picture(&'static [u8]),
}

/// The icon for a currency code ("btc", "EUR") as `casino` draws it, else as Stake does.
pub fn art(casino: CasinoId, code: &str) -> Option<Art> {
    let code = code.to_ascii_lowercase();
    if let Some(p) = picture(casino, &code) {
        return Some(Art::Picture(p));
    }
    let own = match casino {
        CasinoId::Shuffle => shuffle().get(&code),
        _ => None,
    };
    own.or_else(|| stake().get(&code)).map(Art::Svg)
}

/// Stake's order for currency lists (from its own JS): the big coins first, then the common fiats.
const ORDER: &[&str] = &[
    "btc", "eth", "ltc", "usdt", "sol", "doge", "bch", "xrp", "trx", "eos", "bnb", "usdc", "ape", "busd", "cro", "dai", "link",
    "sand", "shib", "uni", "pol", "trump", "gold", "sweeps", "usd", "eur", "jpy", "cad", "cny", "idr", "inr", "krw", "php", "rub",
    "dkk", "mxn", "pln", "try", "vnd", "ars", "pen", "clp", "ngn", "aed", "bhd", "crc", "ils", "jod", "kwd", "mad", "myr", "nok",
    "omr", "qar", "sar", "sgd", "tnd", "twd", "ghs", "kes", "bob", "xof",
];

/// Where a currency comes in Stake's lists; unranked ones share the last place.
pub fn rank(code: &str) -> usize {
    let code = code.to_ascii_lowercase();
    ORDER.iter().position(|o| *o == code).unwrap_or(ORDER.len())
}

/// A fiat currency: one some casino draws as a fiat (Stake has a flag for every fiat it knows).
pub fn is_fiat(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    stake().fiats.contains_key(&code) || shuffle().fiats.contains_key(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_load() {
        assert!(stake().coins.len() >= 20, "Stake coin icons missing");
        assert!(stake().fiats.len() >= 100, "Stake flags missing");
        assert!(shuffle().coins.len() >= 15 && shuffle().fiats.len() >= 18, "Shuffle icons missing");
        for code in ["btc", "ETH", "usdt", "sol", "trump", "usd", "eur", "jpy", "brl"] {
            assert!(art(CasinoId::Stake, code).is_some(), "no Stake icon for {code}");
        }
        for code in ["shfl", "ton", "bonk", "wif", "pump", "cash", "matic", "avax", "pln", "vnd"] {
            assert!(art(CasinoId::Shuffle, code).is_some(), "no Shuffle icon for {code}");
        }
        // Shuffle's own art wins on Shuffle; other casinos fall back to Stake's
        assert!(matches!((art(CasinoId::Shuffle, "eur"), art(CasinoId::Stake, "eur")), (Some(Art::Svg(a)), Some(Art::Svg(b))) if a != b));
        assert!(art(CasinoId::ChipsGg, "btc").is_some());
        assert!(is_fiat("eur") && is_fiat("USD") && !is_fiat("btc") && !is_fiat("usdt") && !is_fiat("shfl"));
        assert!(art(CasinoId::Stake, "nope").is_none());
        assert!(rank("btc") < rank("eth") && rank("usd") < rank("eur") && rank("eur") < rank("nope"));
    }
}
