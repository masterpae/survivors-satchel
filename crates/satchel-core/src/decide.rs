//! How it decides, with no guessing: three fingerprints.
//! L = this PC's live save, B = this PC's last sync, N = the NAS's newest version.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The same save everywhere. `adopt` names the NAS version when only this PC's memory
    /// (B) needs to catch up, for example after an in-game Import of that version.
    InSync { adopt: Option<u64> },
    /// This PC moved on and the NAS didn't: send.
    Send,
    /// The NAS moved on and this PC didn't: bring version `n` in.
    BringIn(u64),
    /// Both moved on since this PC last synced, or this PC never synced and they differ:
    /// the player picks, and both are kept.
    Split { nas: u64 },
}

pub fn decide(local: Option<&str>, base: Option<&str>, nas: Option<(&str, u64)>) -> Decision {
    let Some(local) = local else {
        // No progress on this PC yet: nothing here to lose.
        return match nas {
            Some((_, n)) => Decision::BringIn(n),
            None => Decision::InSync { adopt: None },
        };
    };
    let Some((nas_sha, n)) = nas else {
        // An empty NAS (a new folder, or one that was cleared): this PC's progress seeds it.
        return Decision::Send;
    };
    if local == nas_sha {
        return Decision::InSync { adopt: (base != Some(nas_sha)).then_some(n) };
    }
    if base == Some(nas_sha) {
        return Decision::Send;
    }
    if base == Some(local) {
        return Decision::BringIn(n);
    }
    Decision::Split { nas: n }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Decision::*;

    #[test]
    fn the_four_cases_of_the_proposal() {
        // N = B, L = B: nothing to do.
        assert_eq!(decide(Some("a"), Some("a"), Some(("a", 1))), InSync { adopt: None });
        // N = B, L ≠ B: send.
        assert_eq!(decide(Some("b"), Some("a"), Some(("a", 1))), Send);
        // N ≠ B, L = B: bring it in.
        assert_eq!(decide(Some("a"), Some("a"), Some(("c", 2))), BringIn(2));
        // N ≠ B, L ≠ B: ask, both kept.
        assert_eq!(decide(Some("b"), Some("a"), Some(("c", 2))), Split { nas: 2 });
    }

    #[test]
    fn identical_saves_only_update_memory() {
        assert_eq!(decide(Some("c"), Some("a"), Some(("c", 2))), InSync { adopt: Some(2) });
        assert_eq!(decide(Some("c"), None, Some(("c", 2))), InSync { adopt: Some(2) });
    }

    #[test]
    fn first_start_and_empty_places() {
        // Empty NAS: this PC's progress becomes version 1.
        assert_eq!(decide(Some("a"), None, None), Send);
        // This PC never synced and differs from the NAS: the player picks.
        assert_eq!(decide(Some("a"), None, Some(("c", 4))), Split { nas: 4 });
        // No progress on this PC yet.
        assert_eq!(decide(None, None, Some(("c", 4))), BringIn(4));
        assert_eq!(decide(None, None, None), InSync { adopt: None });
        // The NAS was emptied, or the folder changed: this PC re-seeds it.
        assert_eq!(decide(Some("a"), Some("a"), None), Send);
        assert_eq!(decide(Some("b"), Some("a"), None), Send);
    }
}
