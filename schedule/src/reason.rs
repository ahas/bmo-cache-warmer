//! A ping's reason cut to a length (docs/warming.md, 「What an answer leaves behind」).

/// The first `units` of a text, by the UTF-16 units a script counts; a surrogate pair the cut splits ends
/// as the replacement character.
pub fn first_units(text: &str, units: usize) -> String {
  let all: Vec<u16> = text.encode_utf16().collect();

  String::from_utf16_lossy(&all[..units.min(all.len())])
}
