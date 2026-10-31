//! Signed updates. From 0.7.0 every release of the Satchel carries `SurvivorsSatchel.exe.minisig`:
//! a minisign signature over the exe, made on the maintainer's own PC with a key that never
//! touches GitHub or the machines that build and publish releases. GitHub's checksum proves a file
//! is the one GitHub stores; the signature proves the maintainer released it. An update is offered
//! and installed only when its signature verifies with a key compiled in here and its trusted
//! comment names the release's version.

use minisign_verify::{Error, PublicKey, Signature};

/// A key the Satchel's releases are signed with.
pub struct ReleaseKey {
    /// Its key id, as minisign prints it; the log names it when it verifies an update.
    pub id: &'static str,
    /// The public key: the second line of its `.pub` file.
    pub public: &'static str,
}

/// The keys this Satchel trusts for its own updates: one, or the old and the new one while a key
/// is being replaced. A Satchel reads only `SIGNATURE_NAME`, so that file must be signed with a key
/// every Satchel that should update by itself trusts: the release that adds the new key is signed
/// with the old one, and so are the releases after it until the Satchels that trust only the old
/// key have moved on. A Satchel that skipped all of them is updated by hand.
pub const RELEASE_KEYS: &[ReleaseKey] =
    &[ReleaseKey { id: "2E9B003F0BC653FC", public: "RWT8U8YLPwCbLp80L/d7d2g3uRL8SXo+SSH5qaoxx5KZE1bmnLfIvRKw" }];

/// The signature's file name: a release asset beside the exe.
pub const SIGNATURE_NAME: &str = "SurvivorsSatchel.exe.minisig";

/// A minisign signature is about 300 bytes; a bigger file isn't one.
pub const SIGNATURE_MAX: u64 = 4096;

/// Checks `signature`, the text of a `.minisig` file, over `exe`, the bytes of release `version`'s
/// exe: the id of the key that verified it, or why it was refused.
pub fn verify(exe: &[u8], signature: &str, version: &str) -> Result<&'static str, String> {
    verify_with(RELEASE_KEYS, exe, signature, version)
}

fn verify_with(keys: &'static [ReleaseKey], exe: &[u8], signature: &str, version: &str) -> Result<&'static str, String> {
    let signature = Signature::decode(signature).map_err(|_| "its signature file isn't a minisign signature".to_string())?;
    for key in keys {
        let public = PublicKey::from_base64(key.public).map_err(|_| format!("the release key {} doesn't read", key.id))?;
        match public.verify(exe, &signature, false) {
            // Signed with another key: the next one may be it.
            Err(Error::UnexpectedKeyId) => continue,
            Err(Error::UnexpectedAlgorithm) => return Err("its signature is old-style (legacy), which isn't accepted".into()),
            Err(_) => return Err("its signature doesn't match the file".into()),
            Ok(()) => {
                // The trusted comment is part of what was signed, so it can be believed only now.
                let expected = format!("SurvivorsSatchel {version}");
                let comment = signature.trusted_comment();
                if comment != expected {
                    return Err(format!("it was signed as \"{comment}\", not as \"{expected}\""));
                }
                return Ok(key.id);
            }
        }
    }
    Err("it isn't signed with the Satchel's release key".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A key pair made for these tests with the official minisign 0.12 binary on 31 Oct 2026 and
    // thrown away after signing PAYLOAD: not a release key. Each signature below is that tool's
    // own output: LEGACY with `-l`, NO_COMMENT without `-t` (the tool's default comment).
    const TEST_KEY: ReleaseKey = ReleaseKey { id: "02F5A4767FB1F8D8", public: "RWTY+LF/dqT1AjHk8uzVcNVFDxDoNMQfDkkwS9muAc3v8ibsI+uTw3GI" };
    const TEST_KEYS: &[ReleaseKey] = &[TEST_KEY];
    const PAYLOAD: &[u8] = b"Survivors Satchel test payload\n";
    const GOOD: &str = "untrusted comment: signature from minisign secret key\n\
        RUTY+LF/dqT1Anl/cRjL7NesxAbPsvKL2pc6j7zI4DlbU3kxpRcUXalF40GdQUWhBvpv+HuTHfWUuDUqxyV2Qtg33IV61by61Q8=\n\
        trusted comment: SurvivorsSatchel 0.7.0\n\
        QoKIAD2eXc2jHRGtrukYgHhybGxTqOYoL4f5na0iP86O/5EIErXfWPIZmy5p3qZ4/7g2kDZvmBLIykePQGjvCQ==\n";
    const LEGACY: &str = "untrusted comment: signature from minisign secret key\n\
        RWTY+LF/dqT1AmgMeD0EIBdys4+bN5i2jxsHyu2pIsjmpdyV0As9vRtwo62vhnr1YyJIMN04cco2piFK09pPj+1MrjZKpCjB2gM=\n\
        trusted comment: SurvivorsSatchel 0.7.0\n\
        Ymmr5/WTQo2dA9ku4H+Yp2r7VkoZzAA8gGitio8AC23kRXbODcLiS29nSmEm5WamCxUbWik2SfaAkWVEuEBWAg==\n";
    const OTHER_VERSION: &str = "untrusted comment: signature from minisign secret key\n\
        RUTY+LF/dqT1Anl/cRjL7NesxAbPsvKL2pc6j7zI4DlbU3kxpRcUXalF40GdQUWhBvpv+HuTHfWUuDUqxyV2Qtg33IV61by61Q8=\n\
        trusted comment: SurvivorsSatchel 0.6.9\n\
        Ji2OV650Vg5xlzAbhEV7S/FDKeuBrFpXgQit5YX2V5P8NRMMiFvfG3igJY0rUDkFRie0UndSDfUfAUzwioniCQ==\n";
    const NO_COMMENT: &str = "untrusted comment: signature from minisign secret key\n\
        RUTY+LF/dqT1Anl/cRjL7NesxAbPsvKL2pc6j7zI4DlbU3kxpRcUXalF40GdQUWhBvpv+HuTHfWUuDUqxyV2Qtg33IV61by61Q8=\n\
        trusted comment: timestamp:1793443039\tfile:payload\thashed\n\
        P6xfxmyCJf/r8jumszxsPzpb1GjrpSjrAg62BJLjiqgt6GnLndTQLMEmAUaiTf61mYxt5ZQx5ClzfFw60VXaCw==\n";
    const OTHER_KEY: &str = "untrusted comment: signature from minisign secret key\n\
        RURNncDFuadCQMHN+XadUp58pDEyA+0VLHyt1V+CKcpnW0JX6qiq7YrYFnaGD647IEWFNfNUQk3piMoi5AGjXAhTKginEdkINwI=\n\
        trusted comment: SurvivorsSatchel 0.7.0\n\
        UHBo5NC3Mm7C3XZBo07ly33zpoPCbRwmdr9i5Dq/BfqlqEECpsXEljsQIrm68Eie3HBe9wZaH1k6e2seUYzfAQ==\n";

    fn refused(signature: &str, payload: &[u8], version: &str) -> String {
        verify_with(TEST_KEYS, payload, signature, version).expect_err("should be refused")
    }

    #[test]
    fn a_good_signature_passes_and_names_its_key() {
        assert_eq!(verify_with(TEST_KEYS, PAYLOAD, GOOD, "0.7.0"), Ok("02F5A4767FB1F8D8"));
    }

    #[test]
    fn windows_line_endings_pass() {
        assert_eq!(verify_with(TEST_KEYS, PAYLOAD, &GOOD.replace('\n', "\r\n"), "0.7.0"), Ok("02F5A4767FB1F8D8"));
    }

    #[test]
    fn one_changed_byte_is_refused() {
        let mut exe = PAYLOAD.to_vec();
        exe[0] ^= 1;
        assert!(refused(GOOD, &exe, "0.7.0").contains("doesn't match"));
    }

    #[test]
    fn another_key_is_refused() {
        assert!(refused(OTHER_KEY, PAYLOAD, "0.7.0").contains("release key"));
    }

    #[test]
    fn a_legacy_signature_is_refused() {
        assert!(refused(LEGACY, PAYLOAD, "0.7.0").contains("legacy"));
    }

    #[test]
    fn a_signature_for_another_version_is_refused() {
        assert!(refused(OTHER_VERSION, PAYLOAD, "0.7.0").contains("\"SurvivorsSatchel 0.6.9\""));
        assert!(refused(GOOD, PAYLOAD, "0.7.1").contains("not as \"SurvivorsSatchel 0.7.1\""));
    }

    #[test]
    fn the_tools_default_comment_is_refused() {
        assert!(refused(NO_COMMENT, PAYLOAD, "0.7.0").contains("signed as"));
    }

    #[test]
    fn a_changed_comment_is_refused() {
        // The comment is covered by the second signature: changing it breaks that one.
        let changed = GOOD.replace("SurvivorsSatchel 0.7.0", "SurvivorsSatchel 0.7.1");
        assert!(refused(&changed, PAYLOAD, "0.7.1").contains("doesn't match"));
    }

    #[test]
    fn what_isnt_a_signature_is_refused() {
        assert!(refused("", PAYLOAD, "0.7.0").contains("isn't a minisign signature"));
        assert!(refused("untrusted comment: x\nnot base64\n", PAYLOAD, "0.7.0").contains("isn't a minisign signature"));
    }

    #[test]
    fn during_a_rotation_the_signing_key_verifies_in_either_slot() {
        const RELEASE: ReleaseKey =
            ReleaseKey { id: "2E9B003F0BC653FC", public: "RWT8U8YLPwCbLp80L/d7d2g3uRL8SXo+SSH5qaoxx5KZE1bmnLfIvRKw" };
        const SECOND: &[ReleaseKey] = &[RELEASE, TEST_KEY];
        const FIRST: &[ReleaseKey] = &[TEST_KEY, RELEASE];
        assert_eq!(verify_with(SECOND, PAYLOAD, GOOD, "0.7.0"), Ok("02F5A4767FB1F8D8"));
        assert_eq!(verify_with(FIRST, PAYLOAD, GOOD, "0.7.0"), Ok("02F5A4767FB1F8D8"));
    }

    #[test]
    fn the_release_keys_read() {
        assert!(!RELEASE_KEYS.is_empty());
        for key in RELEASE_KEYS {
            assert!(PublicKey::from_base64(key.public).is_ok(), "{}", key.id);
            assert_eq!(key.id.len(), 16, "{}", key.id);
        }
    }
}
