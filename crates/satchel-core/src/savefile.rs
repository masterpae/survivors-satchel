//! What a save the game wrote looks like, so an empty or damaged one never travels.
//! `characters.txt` and the game's own `.sav` files are `RSE1:` followed by padded base64: every
//! save in the operator's shared folder on 29 Oct 2026 (34 files, 2.2 to 8 KB) has that shape.

/// Smaller than any save the game writes (the smallest seen is 2.2 KB).
const MIN_BYTES: usize = 32;
/// Far larger than any save the game writes (the largest seen is 8 KB).
const MAX_BYTES: usize = 16 * 1024 * 1024;

const NOT_A_SAVE: &str = "doesn't start like a save (RSE1:)";

/// Why `bytes` can't be a save the game wrote, or `None` when they look like one. The answer
/// completes "this PC's save …".
pub fn problem(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some("is empty".into());
    }
    if bytes.len() < MIN_BYTES || bytes.len() > MAX_BYTES {
        return Some(format!("is {} bytes, not a save's size", bytes.len()));
    }
    let text = bytes.trim_ascii_end();
    let Some(rest) = text.strip_prefix(b"RSE") else { return Some(NOT_A_SAVE.into()) };
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || rest.get(digits) != Some(&b':') {
        return Some(NOT_A_SAVE.into());
    }
    let body = &rest[digits + 1..];
    if &rest[..digits] == b"1" {
        // The format the game writes today: base64 with its padding, so a save cut short
        // shows in its length.
        let data = body.strip_suffix(b"==").or_else(|| body.strip_suffix(b"=")).unwrap_or(body);
        if data.is_empty() || !data.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/') {
            return Some("has characters a save never has".into());
        }
        if body.len() % 4 != 0 {
            return Some("is cut short".into());
        }
    } else if body.contains(&0) {
        // A newer format the Satchel doesn't know yet: only plainly broken bytes (zeros, as in
        // a file cut off while it was written) are refused, so a game update never stops the sync.
        return Some("has characters a save never has".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save in the game's shape: `RSE1:` + padded base64 of `data`.
    fn sealed(data: &[u8]) -> Vec<u8> {
        const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = b"RSE1:".to_vec();
        for chunk in data.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
            for i in 0..4 {
                out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] } else { b'=' });
            }
        }
        out
    }

    #[test]
    fn saves_in_the_games_shape_pass() {
        for len in [40, 41, 42, 5000, 5999] {
            let data: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(problem(&sealed(&data)), None, "{len} bytes of data");
        }
        let mut with_newline = sealed(&[9; 60]);
        with_newline.extend_from_slice(b"\r\n");
        assert_eq!(problem(&with_newline), None);
    }

    #[test]
    fn empty_foreign_and_torn_saves_are_refused() {
        assert_eq!(problem(b"").as_deref(), Some("is empty"));
        assert_eq!(problem(b"RSE1:QUJD").as_deref(), Some("is 9 bytes, not a save's size"));
        assert_eq!(problem(b"{\"heroes\": [], \"gold\": 1000000000}").as_deref(), Some(NOT_A_SAVE));
        assert_eq!(problem(b"RSE:QUJDQUJDQUJDQUJDQUJDQUJDQUJDQUJD").as_deref(), Some(NOT_A_SAVE));
        let save = sealed(&[1; 60]);
        assert_eq!(problem(&save[..save.len() - 1]).as_deref(), Some("is cut short"));
        let mut odd = save.clone();
        odd[20] = b'*';
        assert_eq!(problem(&odd).as_deref(), Some("has characters a save never has"));
        let mut zeroed = save;
        zeroed[30..40].fill(0);
        assert_eq!(problem(&zeroed).as_deref(), Some("has characters a save never has"));
    }

    #[test]
    fn padding_only_at_the_end() {
        let mut save = sealed(&[2; 61]);
        assert!(save.ends_with(b"=="));
        save[10] = b'=';
        assert!(problem(&save).is_some());
    }

    #[test]
    fn a_newer_format_is_let_through_unless_plainly_broken() {
        assert_eq!(problem(b"RSE2:{\"anything\":\"the game may write one day\"}"), None);
        assert!(problem(b"RSE2:\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0").is_some());
    }
}
