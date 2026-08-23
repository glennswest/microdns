//! Turning a stored name into a wire name.
//!
//! Names are stored in presentation format — the escaped form DNS tools print
//! and accept — because that is what round-trips: `epson\040et-5170\040series`
//! is a printer whose name contains spaces.
//!
//! [`Name::from_str`] cannot be used to read those back. It decodes the escapes
//! correctly and then rejects the result, because its parser only admits the
//! conventional host character set. A label may hold any byte at all (RFC 2181
//! §11) and DNS-SD relies on it: service instance names are UTF-8 text with
//! spaces and punctuation. Refusing them means the record is silently dropped
//! on the way out and the name simply does not resolve.
//!
//! So the escapes are decoded here and the labels built from raw bytes, which
//! is the same path the wire parser takes.

use hickory_proto::rr::domain::Label;
use hickory_proto::rr::Name;

/// Parse a stored name in presentation format into a wire name.
///
/// Accepts the escapes DNS presentation format defines: `\NNN` for a byte in
/// octal, and `\X` for a literal character. Returns `None` only for genuinely
/// unusable input — a dangling escape, or a label over 63 bytes.
pub fn to_dns_name(presentation: &str) -> Option<Name> {
    let trimmed = presentation.trim_end_matches('.');
    if trimmed.is_empty() {
        return Some(Name::root());
    }

    let mut labels = Vec::new();
    for raw in split_labels(trimmed)? {
        if raw.is_empty() {
            return None;
        }
        labels.push(Label::from_raw_bytes(&raw).ok()?);
    }

    let mut name = Name::from_labels(labels).ok()?;
    name.set_fqdn(true);
    Some(name)
}

/// Split on unescaped dots and decode each label to its bytes.
fn split_labels(name: &str) -> Option<Vec<Vec<u8>>> {
    let mut labels = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut chars = name.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '.' => labels.push(std::mem::take(&mut current)),
            '\\' => {
                // `\NNN` is one byte written in octal; anything else escapes a
                // single character.
                let digits: String = chars
                    .clone()
                    .take(3)
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if digits.len() == 3 {
                    let value = u16::from_str_radix(&digits, 8).ok()?;
                    if value > 255 {
                        return None;
                    }
                    current.push(value as u8);
                    for _ in 0..3 {
                        chars.next();
                    }
                } else {
                    let escaped = chars.next()?;
                    let mut buf = [0u8; 4];
                    current.extend_from_slice(escaped.encode_utf8(&mut buf).as_bytes());
                }
            }
            _ => {
                let mut buf = [0u8; 4];
                current.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    labels.push(current);
    Some(labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_are_unchanged() {
        let name = to_dns_name("boot.gw.lo.").unwrap();
        assert_eq!(name.to_string(), "boot.gw.lo.");
        assert!(name.is_fqdn());
        // A missing trailing dot is still a fully qualified name here: every
        // name in the database is absolute.
        assert_eq!(to_dns_name("boot.gw.lo").unwrap().to_string(), "boot.gw.lo.");
    }

    #[test]
    fn dns_sd_instance_names_survive_the_round_trip() {
        // The exact forms that were being dropped: spaces, punctuation and
        // UTF-8 in a service instance name.
        for original in [
            "epson\\040et-5170\\040series._printer._tcp.mdns.lo.",
            "02424c526151\\@gwest-mac._raop._tcp.mdns.lo.",
            "glenn\\342\\200\\231s\\040mac\\040mini._ssh._tcp.mdns.lo.",
            "55\\\"\\040the\\040frame._airplay._tcp.mdns.lo.",
        ] {
            let name = to_dns_name(original)
                .unwrap_or_else(|| panic!("{original} should parse"));
            assert_eq!(
                name.to_string(),
                original,
                "presentation form must survive the round trip"
            );
        }
    }

    #[test]
    fn an_escaped_dot_stays_inside_its_label() {
        let name = to_dns_name("host\\.name.gw.lo.").unwrap();
        assert_eq!(name.num_labels(), 3, "the escaped dot must not split");
    }

    #[test]
    fn unusable_input_is_refused_rather_than_guessed_at() {
        assert!(to_dns_name("trailing\\").is_none());
        assert!(to_dns_name("a..b").is_none(), "an empty label is not a name");
        let too_long = format!("{}.gw.lo", "a".repeat(64));
        assert!(to_dns_name(&too_long).is_none());
    }

    #[test]
    fn the_root_is_a_name() {
        assert_eq!(to_dns_name(".").unwrap(), Name::root());
    }
}
