//! Musical keys: 0-11 = C..B major, 12-23 = C..B minor, shown in Camelot notation.

pub const PITCH: [&str; 12] = ["C", "D♭", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Camelot {
    pub num: u8,
    pub minor: bool,
}

impl Camelot {
    pub fn letter(&self) -> char { if self.minor { 'A' } else { 'B' } }
    pub fn label(&self) -> String { format!("{}{}", self.num, self.letter()) }
    /// sort order used by the library's Key column
    pub fn sort(&self) -> i32 { self.num as i32 * 2 + if self.minor { 0 } else { 1 } }
    /// display colour: hsl((num*30+150) % 360, 70%, 64/74%)
    pub fn color(&self) -> [u8; 3] {
        let h = ((self.num as f32 * 30.0 + 150.0) % 360.0) / 360.0;
        hsl(h, 0.70, if self.minor { 0.64 } else { 0.74 })
    }
}

pub fn camelot(k: i32) -> Option<Camelot> {
    if !(0..24).contains(&k) { return None; }
    let minor = k >= 12;
    let pc = k % 12;
    let rel = if minor { (pc + 3) % 12 } else { pc }; // relative major
    let num = ((rel * 7) % 12 + 7) % 12 + 1; // C major = 8B, G = 9B ...
    Some(Camelot { num: num as u8, minor })
}

pub fn key_name(k: i32) -> String {
    if !(0..24).contains(&k) { return String::new(); }
    format!("{}{}", PITCH[(k % 12) as usize], if k >= 12 { " minor" } else { " major" })
}

pub fn shift_key(k: i32, semis: i32) -> i32 {
    if k < 0 { return k; }
    (if k >= 12 { 12 } else { 0 }) + (((k % 12) + semis) % 12 + 12) % 12
}

/// harmonic mixing: same number, one step round the wheel, or the relative major/minor
pub fn keys_match(a: Option<Camelot>, b: Option<Camelot>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            if a.num == b.num { return true; }
            let d = (a.num as i32 - b.num as i32).abs();
            a.minor == b.minor && (d == 1 || d == 11)
        }
        _ => false,
    }
}

/// "Am", "F#m", "8A", "Db" ... -> key index (rekordbox XML / export.pdb spellings)
pub fn key_index(s: &str) -> i32 {
    let s = s.trim();
    if s.is_empty() { return -1; }
    // Camelot / Open Key style "8A"
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() < s.len() {
        let num: i32 = digits.parse().unwrap_or(0);
        let l = s[digits.len()..].chars().next().unwrap_or(' ').to_ascii_uppercase();
        if (1..=12).contains(&num) && (l == 'A' || l == 'B') {
            for k in 0..24 {
                let c = camelot(k).unwrap();
                if c.num as i32 == num && c.minor == (l == 'A') { return k; }
            }
        }
        return -1;
    }
    let mut it = s.chars();
    let letter = it.next().unwrap().to_ascii_uppercase();
    let base = match letter { 'C' => 0, 'D' => 2, 'E' => 4, 'F' => 5, 'G' => 7, 'A' => 9, 'B' => 11, _ => return -1 };
    let rest: String = it.collect();
    let mut pc: i32 = base;
    let mut tail = rest.as_str();
    if let Some(t) = tail.strip_prefix('#').or_else(|| tail.strip_prefix('♯')) { pc += 1; tail = t; }
    else if let Some(t) = tail.strip_prefix('b').or_else(|| tail.strip_prefix('♭')) { pc -= 1; tail = t; }
    let t = tail.trim().to_ascii_lowercase();
    let minor = t == "m" || t.starts_with("min") || t == "minor";
    if !(t.is_empty() || minor || t.starts_with("maj")) { return -1; }
    pc = (pc + 12) % 12;
    (if minor { 12 } else { 0 }) + pc
}

/// rekordbox XML Tonality spelling: "C", "Db", ..., "Am"
pub fn key_xml(k: i32) -> String {
    const N: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
    if !(0..24).contains(&k) { return String::new(); }
    format!("{}{}", N[(k % 12) as usize], if k >= 12 { "m" } else { "" })
}

pub fn hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 { t += 1.0; }
        if t > 1.0 { t -= 1.0; }
        let v = if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t } else if t < 0.5 { q } else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 } else { p };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camelot_wheel() {
        assert_eq!(camelot(0).unwrap().label(), "8B"); // C major
        assert_eq!(camelot(7).unwrap().label(), "9B"); // G major
        assert_eq!(camelot(21).unwrap().label(), "8A"); // A minor
        assert_eq!(camelot(17).unwrap().label(), "4A"); // F minor
        assert_eq!(camelot(14).unwrap().label(), "7A"); // D minor
        assert_eq!(camelot(3).unwrap().label(), "5B"); // E-flat major
    }
    #[test]
    fn parse_keys() {
        assert_eq!(key_index("Am"), 21);
        assert_eq!(key_index("Fm"), 17);
        assert_eq!(key_index("8A"), 21);
        assert_eq!(key_index("Db"), 1);
        assert_eq!(key_index("F#m"), 18);
        assert_eq!(key_index(""), -1);
        assert_eq!(key_xml(17), "Fm");
        for k in 0..24 { assert_eq!(key_index(&key_xml(k)), k); }
    }
    #[test]
    fn matching() {
        assert!(keys_match(camelot(21), camelot(0))); // 8A / 8B
        assert!(keys_match(camelot(21), camelot(16))); // 8A / 9A (E minor)
        assert!(!keys_match(camelot(21), camelot(7))); // 8A / 9B
    }
}
