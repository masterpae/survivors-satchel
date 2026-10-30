//! An addon's `.toc`, read the way the game reads it: `##` at the start of a line opens a
//! directive, keys match case-insensitively as prefixes, the last value of a scalar wins and lists
//! append, split on commas or spaces.

/// The only `## Interface` the game loads unless "Load out of date AddOns" is on.
pub const GAME_INTERFACE: u32 = 11200;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Toc {
    /// 0 when the line is missing; `11200, 11300` reads as 11200, as in the game.
    pub interface: u32,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    /// `## Dependencies`, `## RequiredDeps` and anything else starting `Dep`/`RequiredDep`.
    pub required: Vec<String>,
    pub optional: Vec<String>,
    /// `## DefaultState: disabled`: off until the player turns it on.
    pub default_disabled: bool,
    pub load_on_demand: bool,
    /// `## X-Website`.
    pub website: Option<String>,
}

impl Toc {
    pub fn parse(bytes: &[u8]) -> Toc {
        let text = decode(bytes);
        let mut toc = Toc::default();
        for line in text.split(['\r', '\n']) {
            let Some(directive) = line.strip_prefix("##") else { continue };
            let Some((key, value)) = directive.split_once(':') else { continue };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            let scalar = || (!value.is_empty()).then(|| value.to_string());
            let list = || value.split([',', ' ']).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
            // The game's own order of prefixes.
            if key == "interface" {
                toc.interface = value.split([',', ' ']).next().and_then(|n| n.trim().parse().ok()).unwrap_or(0);
            } else if key == "title" {
                toc.title = scalar().map(|t| plain(&t));
            } else if key == "notes" {
                toc.notes = scalar().map(|t| plain(&t));
            } else if key == "author" {
                toc.author = scalar();
            } else if key == "version" {
                toc.version = scalar();
            } else if key == "x-website" {
                toc.website = scalar();
            } else if key.starts_with("optionaldep") {
                toc.optional.extend(list());
            } else if key.starts_with("requireddep") || key.starts_with("dep") {
                toc.required.extend(list());
            } else if key == "defaultstate" {
                toc.default_disabled = value.eq_ignore_ascii_case("disabled");
            } else if key == "loadondemand" {
                toc.load_on_demand = value == "1";
            }
        }
        toc
    }

    /// Whether the game loads it without "Load out of date AddOns".
    pub fn current(&self) -> bool {
        self.interface == GAME_INTERFACE
    }
}

/// UTF-8 (a BOM dropped), else Windows-1252 read as Latin-1, as the game does.
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// A title without the game's colour codes (`|cff33ff99Name|r`).
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('|') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        rest = match after.chars().next() {
            Some('c' | 'C') if after.len() >= 9 && after.is_char_boundary(9) => &after[9..],
            Some('r' | 'R') => &after[1..],
            _ => {
                out.push('|');
                after
            }
        };
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_manager_needs() {
        let toc = Toc::parse(
            b"\xEF\xBB\xBF## Interface: 11200\r\n## Title: |cff33ff99Lucky|rOdds\r\n## Notes: Shows the odds\r\n\
              ## Author: cadoky\r\n## Version: 0.7.0\r\n## Dependencies: SurvivorUI, Lib\r\n## OptionalDeps: A B\r\n\
              ## SavedVariables: LuckyOddsDB\r\n# a comment\r\nLuckyOdds.lua\r\n",
        );
        assert_eq!(toc.interface, 11200);
        assert!(toc.current());
        assert_eq!(toc.title.as_deref(), Some("LuckyOdds"));
        assert_eq!(toc.version.as_deref(), Some("0.7.0"));
        assert_eq!(toc.author.as_deref(), Some("cadoky"));
        assert_eq!(toc.required, ["SurvivorUI", "Lib"]);
        assert_eq!(toc.optional, ["A", "B"]);
        assert!(!toc.default_disabled);
    }

    #[test]
    fn follows_the_games_quirks() {
        // The first value counts, keys are case-insensitive, the last scalar wins.
        let toc = Toc::parse(b"## interface: 11200, 11300\n## Version: 1\n## VERSION: 2\n## RequiredDeps: X\n## DefaultState: Disabled\n");
        assert_eq!(toc.interface, 11200);
        assert_eq!(toc.version.as_deref(), Some("2"));
        assert_eq!(toc.required, ["X"]);
        assert!(toc.default_disabled);
        // A missing line is 0; an indented ## is a file name, not a directive.
        let toc = Toc::parse(b"  ## Interface: 11200\n## Title: Old\n## Interface: 30300\n");
        assert_eq!(toc.interface, 30300);
        assert!(!toc.current());
        assert_eq!(Toc::parse(b"## Title: x").interface, 0);
        // Windows-1252 text.
        assert_eq!(Toc::parse(b"## Author: Ula\xfe").author.as_deref(), Some("Ula\u{fe}"));
    }
}
