//! Whether someone set this node up, and the credential it was given.
//!
//! A node enrolls and sends only after someone set it up to: with a token of its own, or with an
//! enrollment token it trades for one. The API's address alone does neither, so a binary on a
//! device nobody set up makes no call at all. A node that was turned off on the device makes none
//! either, whatever credential it still holds.

/// A credential. It is printed as its first characters only, so a log line, a panic or a debug
/// print never carries one. Nothing serializes it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// The credential itself, for the `Authorization` header and for the file it is kept in.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The first characters, enough to tell credentials apart in a list: `rvrs_Zm9y`.
    pub fn prefix(&self) -> String {
        self.0.chars().take(9).collect()
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}\u{2026}", self.prefix())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Secret({self})")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// The node's own token: it sends the node's runs and claims the runs asked of it.
    Token(Secret),
    /// An enrollment token, which the node trades for a token of its own and does nothing else
    /// with.
    Enrollment(Secret),
}

/// What was found on the device and in the environment when the node started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// The node was turned off on this device.
    pub off: bool,
    /// The token the node keeps from an earlier enroll.
    pub kept: Option<String>,
    /// A token someone gave the node (`RVRS_TOKEN`).
    pub token: Option<String>,
    /// An enrollment token someone gave the node.
    pub enrollment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// Nobody set this node up. It makes no call.
    NotSetUp,
    /// Someone on the device turned the node off. It makes no call until it is turned on again.
    Off,
    Ready(Credential),
}

impl Setup {
    /// A token someone gave comes before the one the node kept, and a token of its own before
    /// an enrollment token: a node that has enrolled never enrolls again while its token works.
    pub fn of(found: Found) -> Setup {
        if found.off {
            return Setup::Off;
        }
        let given = |value: Option<String>| {
            value
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(Secret::new)
        };
        if let Some(token) = given(found.token).or_else(|| given(found.kept)) {
            return Setup::Ready(Credential::Token(token));
        }
        match given(found.enrollment) {
            Some(token) => Setup::Ready(Credential::Enrollment(token)),
            None => Setup::NotSetUp,
        }
    }

    /// The credential to call the API with. None means the node makes no call: the client that
    /// talks to the API is built from this and from nothing else.
    pub fn credential(&self) -> Option<&Credential> {
        match self {
            Setup::Ready(credential) => Some(credential),
            Setup::NotSetUp | Setup::Off => None,
        }
    }

    /// What `rvrs node` says when it has nothing to call with. None when it is ready.
    pub fn notice(&self) -> Option<&'static str> {
        match self {
            Setup::Ready(_) => None,
            Setup::NotSetUp => Some(
                "This node isn't set up. It needs a token or an enrollment token, and sends nothing without one.",
            ),
            Setup::Off => Some("This node is turned off on this device. It sends nothing."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_nobody_set_up_has_nothing_to_call_with() {
        // The API's address is not among what a node is set up with, so knowing it changes
        // nothing: no token and no enrollment token means no credential and no call.
        let setup = Setup::of(Found::default());
        assert_eq!(setup, Setup::NotSetUp);
        assert_eq!(setup.credential(), None);
        assert!(setup.notice().is_some());

        let blank = Setup::of(Found {
            token: Some("  ".into()),
            enrollment: Some(String::new()),
            ..Found::default()
        });
        assert_eq!(blank, Setup::NotSetUp);
    }

    #[test]
    fn a_node_that_was_turned_off_makes_no_call_whatever_it_holds() {
        let setup = Setup::of(Found {
            off: true,
            kept: Some("rvrs_Zm9yIHRoZSBub2Rl".into()),
            token: Some("rvrs_Zm9yIHRoZSBub2Rl".into()),
            enrollment: Some("rvrs_enroll_c2V0IHVw".into()),
        });
        assert_eq!(setup, Setup::Off);
        assert_eq!(setup.credential(), None);
    }

    #[test]
    fn a_token_of_its_own_comes_before_an_enrollment_token() {
        let enrolled = Setup::of(Found {
            kept: Some("rvrs_a2VwdA".into()),
            enrollment: Some("rvrs_enroll_c2V0IHVw".into()),
            ..Found::default()
        });
        assert_eq!(
            enrolled.credential(),
            Some(&Credential::Token(Secret::new("rvrs_a2VwdA")))
        );
        let given = Setup::of(Found {
            kept: Some("rvrs_a2VwdA".into()),
            token: Some(" rvrs_Z2l2ZW4\n".into()),
            ..Found::default()
        });
        assert_eq!(
            given.credential(),
            Some(&Credential::Token(Secret::new("rvrs_Z2l2ZW4")))
        );
        let fresh = Setup::of(Found {
            enrollment: Some("rvrs_enroll_c2V0IHVw".into()),
            ..Found::default()
        });
        assert_eq!(
            fresh.credential(),
            Some(&Credential::Enrollment(Secret::new("rvrs_enroll_c2V0IHVw")))
        );
        assert_eq!(fresh.notice(), None);
    }

    #[test]
    fn a_credential_never_prints_whole() {
        let secret = Secret::new("rvrs_Zm9yIHRoZSBub2RlIG9ubHk");
        assert_eq!(secret.to_string(), "rvrs_Zm9y\u{2026}");
        assert_eq!(format!("{secret:?}"), "Secret(rvrs_Zm9y\u{2026})");
        let setup = Setup::Ready(Credential::Token(secret.clone()));
        let printed = format!("{setup:?} {setup:#?}");
        assert!(!printed.contains("IHRoZSBub2Rl"), "{printed}");
        assert_eq!(secret.expose(), "rvrs_Zm9yIHRoZSBub2RlIG9ubHk");
    }
}
