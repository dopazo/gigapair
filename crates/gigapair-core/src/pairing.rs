//! Pairing codes and the 4-word fingerprint (RF-01).
//!
//! A pairing code carries the creator's endpoint id, how to reach it (its
//! relay URL, or direct addresses when relays are off) and a one-time
//! secret. The device that pastes the code connects and proves it holds
//! the secret. Both devices then show a fingerprint derived from both ids
//! and the secret; if the words match, nobody swapped the code in transit.

use std::net::SocketAddr;

use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};

/// Codes expire after this long (spec: 10 minutes).
pub const CODE_TTL_SECS: i64 = 10 * 60;

const CODE_PREFIX: &str = "gp1";
const CODE_VERSION: u8 = 1;
const TAG_RELAY: u8 = 0;
const TAG_IP: u8 = 1;

pub type PairSecret = [u8; 16];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub addr: EndpointAddr,
    pub secret: PairSecret,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodeError {
    #[error(
        "this is not a Gigapair pairing code; copy the whole code (it starts with \"{CODE_PREFIX}-\")"
    )]
    Format,
    #[error(
        "this pairing code was made by a newer version of Gigapair; update Gigapair on this device"
    )]
    Version,
}

impl PairingCode {
    pub fn new(addr: EndpointAddr, secret: PairSecret) -> Self {
        Self { addr, secret }
    }

    /// Text form: `gp1-` followed by lowercase base32, in groups of 4 for
    /// readability.
    pub fn encode(&self) -> String {
        let mut bytes = vec![CODE_VERSION];
        bytes.extend_from_slice(self.addr.id.as_bytes());
        bytes.extend_from_slice(&self.secret);
        for addr in &self.addr.addrs {
            match addr {
                TransportAddr::Relay(url) => {
                    let s = url.to_string();
                    if s.len() <= u8::MAX as usize {
                        bytes.push(TAG_RELAY);
                        bytes.push(s.len() as u8);
                        bytes.extend_from_slice(s.as_bytes());
                    }
                }
                TransportAddr::Ip(sa) => {
                    let s = sa.to_string();
                    bytes.push(TAG_IP);
                    bytes.push(s.len() as u8);
                    bytes.extend_from_slice(s.as_bytes());
                }
                _ => {}
            }
        }
        let body = data_encoding::BASE32_NOPAD
            .encode(&bytes)
            .to_ascii_lowercase();
        let grouped: Vec<&str> = body
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect();
        format!("{CODE_PREFIX}-{}", grouped.join("-"))
    }

    /// Parses a code, tolerating whitespace, case and missing dashes.
    pub fn decode(text: &str) -> Result<Self, CodeError> {
        let cleaned: String = text
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect::<String>()
            .to_ascii_uppercase();
        let body = cleaned
            .strip_prefix(&CODE_PREFIX.to_ascii_uppercase())
            .ok_or(CodeError::Format)?;
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.as_bytes())
            .map_err(|_| CodeError::Format)?;
        if bytes.len() < 49 {
            return Err(CodeError::Format);
        }
        let (&version, rest) = bytes.split_first().ok_or(CodeError::Format)?;
        if version != CODE_VERSION {
            return Err(CodeError::Version);
        }
        let id_bytes: [u8; 32] = rest[..32].try_into().map_err(|_| CodeError::Format)?;
        let id = EndpointId::from_bytes(&id_bytes).map_err(|_| CodeError::Format)?;
        let secret: PairSecret = rest[32..48].try_into().map_err(|_| CodeError::Format)?;
        let mut addrs = Vec::new();
        let mut cur = &rest[48..];
        while !cur.is_empty() {
            if cur.len() < 2 {
                return Err(CodeError::Format);
            }
            let (tag, len) = (cur[0], cur[1] as usize);
            let value = cur.get(2..2 + len).ok_or(CodeError::Format)?;
            let s = std::str::from_utf8(value).map_err(|_| CodeError::Format)?;
            match tag {
                TAG_RELAY => addrs.push(TransportAddr::Relay(
                    s.parse::<RelayUrl>().map_err(|_| CodeError::Format)?,
                )),
                TAG_IP => addrs.push(TransportAddr::Ip(
                    s.parse::<SocketAddr>().map_err(|_| CodeError::Format)?,
                )),
                _ => {} // Unknown hints from newer versions are ignored.
            }
            cur = &cur[2 + len..];
        }
        Ok(Self {
            addr: EndpointAddr::from_parts(id, addrs),
            secret,
        })
    }
}

/// Four words both devices show during pairing. Symmetric: the order of
/// the two ids doesn't matter.
pub fn fingerprint(a: &EndpointId, b: &EndpointId, secret: &PairSecret) -> String {
    let (lo, hi) = if a.as_bytes() <= b.as_bytes() {
        (a, b)
    } else {
        (b, a)
    };
    let mut h = blake3::Hasher::new_derive_key("gigapair 2026 pairing fingerprint v1");
    h.update(lo.as_bytes());
    h.update(hi.as_bytes());
    h.update(secret);
    let out = h.finalize();
    out.as_bytes()[..4]
        .iter()
        .map(|&b| WORDS[b as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// 256 short, distinct English words: one per byte.
const WORDS: [&str; 256] = [
    "acid", "acorn", "actor", "adult", "agent", "alarm", "album", "alley", "amber", "angle",
    "ankle", "apple", "apron", "arena", "arrow", "aspen", "atlas", "attic", "audio", "award",
    "bacon", "badge", "bagel", "baker", "bamboo", "banjo", "barn", "basil", "basket", "beach",
    "beard", "bee", "bell", "bench", "berry", "bike", "bird", "blade", "blanket", "blossom",
    "boat", "bone", "book", "boot", "bottle", "box", "branch", "bread", "brick", "bridge", "broom",
    "brush", "bucket", "bugle", "butter", "cabin", "cable", "cactus", "camel", "camera", "canal",
    "candle", "canoe", "canyon", "carpet", "carrot", "castle", "cedar", "chair", "chalk", "cherry",
    "chess", "chimney", "cider", "circle", "cliff", "clock", "cloud", "clover", "coast", "cobra",
    "cocoa", "comet", "copper", "coral", "cotton", "cradle", "crane", "crayon", "cricket", "crown",
    "cup", "daisy", "delta", "desert", "diamond", "dinner", "dock", "dolphin", "donkey", "door",
    "dragon", "drum", "duck", "eagle", "earth", "easel", "echo", "elbow", "elephant", "elm",
    "ember", "engine", "fabric", "falcon", "feather", "fence", "fern", "ferry", "fiddle", "field",
    "fig", "finch", "flag", "flame", "flute", "forest", "fork", "fossil", "fox", "frog", "garden",
    "garlic", "gecko", "giant", "ginger", "glacier", "globe", "glove", "goat", "gold", "grape",
    "gravel", "guitar", "hammer", "harbor", "harp", "hawk", "hazel", "helmet", "heron", "hill",
    "honey", "horse", "igloo", "island", "ivory", "jacket", "jaguar", "jelly", "jewel", "juice",
    "kayak", "kettle", "kiwi", "koala", "ladder", "lagoon", "lake", "lamp", "lantern", "lemon",
    "lily", "lion", "lizard", "llama", "lobster", "magnet", "mango", "maple", "marble", "meadow",
    "melon", "mint", "mirror", "moon", "moose", "mountain", "mouse", "mug", "needle", "nest",
    "noodle", "oak", "oasis", "ocean", "olive", "onion", "orange", "orbit", "otter", "owl",
    "paddle", "palm", "panda", "paper", "parrot", "peach", "pearl", "pebble", "pepper", "piano",
    "pillow", "pine", "planet", "plum", "pony", "potato", "puzzle", "quartz", "quilt", "rabbit",
    "radio", "rain", "raven", "reef", "ribbon", "river", "robin", "rocket", "rose", "saddle",
    "salmon", "sand", "scarf", "shell", "silver", "sky", "sled", "snail", "spider", "spoon",
    "squid", "star", "stone", "sugar", "sun", "swan", "table", "tiger", "tomato", "tulip",
    "turtle", "violin", "walnut", "whale",
];

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn id() -> EndpointId {
        SecretKey::generate().public()
    }

    #[test]
    fn words_are_unique() {
        let set: std::collections::HashSet<_> = WORDS.iter().collect();
        assert_eq!(set.len(), WORDS.len());
    }

    #[test]
    fn code_roundtrip_relay_and_ip() {
        let relay: RelayUrl = "https://relay.example.com".parse().unwrap();
        let addr = EndpointAddr::new(id())
            .with_relay_url(relay)
            .with_ip_addr("192.168.1.5:4433".parse().unwrap());
        let code = PairingCode::new(addr, [7; 16]);
        let text = code.encode();
        assert!(text.starts_with("gp1-"));
        assert_eq!(PairingCode::decode(&text).unwrap(), code);
        // Tolerant of case, spaces and dashes.
        let messy = format!("  {} ", text.to_uppercase().replace('-', " "));
        assert_eq!(PairingCode::decode(&messy).unwrap(), code);
    }

    #[test]
    fn bad_codes() {
        assert_eq!(PairingCode::decode("hello"), Err(CodeError::Format));
        assert_eq!(PairingCode::decode("gp1-aaaa"), Err(CodeError::Format));
        let mut bytes = vec![9u8];
        bytes.extend_from_slice(&[0; 48]);
        let text = format!(
            "gp1-{}",
            data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
        );
        assert_eq!(PairingCode::decode(&text), Err(CodeError::Version));
    }

    #[test]
    fn fingerprint_is_symmetric_and_secret_bound() {
        let (a, b) = (id(), id());
        let f1 = fingerprint(&a, &b, &[1; 16]);
        assert_eq!(f1, fingerprint(&b, &a, &[1; 16]));
        assert_eq!(f1.split(' ').count(), 4);
        assert_ne!(f1, fingerprint(&a, &b, &[2; 16]));
    }
}
