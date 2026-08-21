// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Joining dispatch sites to the functions installed in the members they
// dispatch through. Neither side knows the answer alone: a site names a
// member, a registration names a target, and the join is what turns them
// into "this call can reach that function".
use std::collections::HashMap;

use crate::types::{DispatchSite, Registration};

/// A call site that can reach a function without naming it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndirectCaller {
    /// Function containing the call, empty when the site is not in one.
    pub caller_name: String,
    pub site_file: String,
    pub site_line: u32,
    pub member: String,
    /// How the site was written: a member call, a call through a pointer, a
    /// candidate an indirect-call macro named.
    pub site_kind: String,
    /// Why this site is believed to reach the target.
    pub evidence: Evidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// The site itself names the target: a macro's declared candidate, or a
    /// local pointer's initializer. Nothing had to be joined.
    StatedAtSite,
    /// The target is installed in a member this site dispatches through.
    /// `container_type` is the type it was installed in, which the site may
    /// not state — see `type_matched`.
    Registered {
        container_type: String,
        registration_file: String,
        registration_line: u32,
        /// True when the site's receiver type is known and matches the type
        /// the function was installed in. False means the member names match
        /// and nothing contradicts it, which is weaker.
        type_matched: bool,
    },
}

impl Evidence {
    pub fn is_type_matched(&self) -> bool {
        match self {
            Evidence::StatedAtSite => true,
            Evidence::Registered { type_matched, .. } => *type_matched,
        }
    }
}

/// Match dispatch sites against the places a function is installed.
///
/// Both inputs are expected to be filtered to the revision being queried
/// already: this decides what reaches what, not what exists.
pub fn indirect_callers(
    registrations: &[Registration],
    sites_by_member: &HashMap<String, Vec<DispatchSite>>,
    sites_naming_target: &[DispatchSite],
) -> Vec<IndirectCaller> {
    let mut found = Vec::new();

    for site in sites_naming_target {
        found.push(IndirectCaller {
            caller_name: site.caller_name.clone(),
            site_file: site.file_path.clone(),
            site_line: site.line,
            member: site.member.clone(),
            site_kind: site.kind.as_str().to_string(),
            evidence: Evidence::StatedAtSite,
        });
    }

    for registration in registrations {
        let Some(sites) = sites_by_member.get(&registration.member) else {
            continue;
        };

        for site in sites {
            // A site that states its receiver type only matches the type the
            // function was installed in; one that does not is a member-name
            // match, which is weaker but is what the reader asked for.
            let type_matched = match site.receiver_type.as_deref() {
                Some(receiver_type) => {
                    if receiver_type != registration.container_type {
                        continue;
                    }
                    true
                }
                None => false,
            };

            found.push(IndirectCaller {
                caller_name: site.caller_name.clone(),
                site_file: site.file_path.clone(),
                site_line: site.line,
                member: site.member.clone(),
                site_kind: site.kind.as_str().to_string(),
                evidence: Evidence::Registered {
                    container_type: registration.container_type.clone(),
                    registration_file: registration.file_path.clone(),
                    registration_line: registration.line,
                    type_matched,
                },
            });
        }
    }

    found.sort_by(|a, b| {
        (&a.caller_name, &a.site_file, a.site_line).cmp(&(
            &b.caller_name,
            &b.site_file,
            b.site_line,
        ))
    });
    found.dedup();

    found
}

/// Sites are grouped by the member they dispatch through, which is the key a
/// registration joins on.
pub fn group_by_member(sites: Vec<DispatchSite>) -> HashMap<String, Vec<DispatchSite>> {
    let mut grouped: HashMap<String, Vec<DispatchSite>> = HashMap::new();
    for site in sites {
        if site.member.is_empty() {
            continue;
        }
        grouped.entry(site.member.clone()).or_default().push(site);
    }

    grouped
}

/// Keep only rows whose file is at the revision being queried, the same way
/// function lookups do.
pub fn at_revision<T, F>(rows: Vec<T>, manifest: &HashMap<String, String>, key: F) -> Vec<T>
where
    F: Fn(&T) -> (&str, &str),
{
    rows.into_iter()
        .filter(|row| {
            let (file, hash) = key(row);
            manifest.get(file).map(|h| h == hash).unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DispatchKind, RegistrationKind};

    fn site(member: &str, caller: &str, receiver_type: Option<&str>) -> DispatchSite {
        DispatchSite {
            caller_name: caller.to_string(),
            file_path: "user.c".to_string(),
            git_file_hash: "hash-user".to_string(),
            byte_start: 10,
            line: 42,
            member: member.to_string(),
            receiver_expr: Some("ops".to_string()),
            receiver_type: receiver_type.map(|t| t.to_string()),
            receiver_base_type: None,
            receiver_field: None,
            kind: DispatchKind::MemberArrow,
            target: None,
        }
    }

    fn registration(container: &str, member: &str, target: &str) -> Registration {
        Registration {
            container_type: container.to_string(),
            member: member.to_string(),
            target: target.to_string(),
            file_path: "driver.c".to_string(),
            git_file_hash: "hash-driver".to_string(),
            byte_start: 20,
            line: 7,
            enclosing_function: String::new(),
            kind: RegistrationKind::DesignatedInit,
        }
    }

    #[test]
    fn a_member_call_reaches_what_is_installed_in_that_member() {
        let sites = group_by_member(vec![site("read", "vfs_read", None)]);
        let found = indirect_callers(
            &[registration("file_operations", "read", "my_read")],
            &sites,
            &[],
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].caller_name, "vfs_read");
        match &found[0].evidence {
            Evidence::Registered {
                container_type,
                type_matched,
                ..
            } => {
                assert_eq!(container_type, "file_operations");
                // The site did not say what type its receiver had.
                assert!(!type_matched);
            }
            other => panic!("unexpected evidence: {other:?}"),
        }
    }

    #[test]
    fn a_typed_site_does_not_match_another_types_member() {
        let sites = group_by_member(vec![site("read", "vfs_read", Some("file_operations"))]);
        let found = indirect_callers(
            &[registration("proto_ops", "read", "sock_read")],
            &sites,
            &[],
        );

        assert!(found.is_empty(), "matched across types: {found:?}");
    }

    #[test]
    fn a_site_that_names_the_target_needs_no_registration() {
        let mut stated = site("handler", "ip_protocol_deliver_rcu", None);
        stated.kind = DispatchKind::MacroDeclared;
        stated.target = Some("tcp_v4_rcv".to_string());

        let found = indirect_callers(&[], &HashMap::new(), &[stated]);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].evidence, Evidence::StatedAtSite);
        assert!(found[0].evidence.is_type_matched());
    }
}
