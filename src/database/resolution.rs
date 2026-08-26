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
    /// Byte offset of the site. Two dispatches can share a line — `a->f()`
    /// and `b->f()`, or a chain resolved twice — and without this they are
    /// one answer whose registration count is the sum of both.
    pub site_byte_start: u64,
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
        /// One of the places it is installed, and how many there are. A
        /// function in a widely used slot is installed hundreds of times,
        /// and the site is the same site whichever of them the reader looks
        /// at.
        registration_file: String,
        registration_line: u32,
        registration_count: usize,
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
            site_byte_start: site.byte_start,
            member: site.member.clone(),
            site_kind: site.kind.as_str().to_string(),
            evidence: Evidence::StatedAtSite,
        });
    }

    // A site reached through a registration is one answer, however many
    // places install the function there: seq_read sits in file_operations
    // ::read in 430 of them, and listing fs/read_write.c once per
    // registration buries the three call sites that matter.
    type SiteKey = (String, String, u32, u64, String, String, String, bool);
    let mut by_site: HashMap<SiteKey, (&Registration, usize)> = HashMap::new();

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

            let key = (
                site.caller_name.clone(),
                site.file_path.clone(),
                site.line,
                site.byte_start,
                site.member.clone(),
                site.kind.as_str().to_string(),
                registration.container_type.clone(),
                type_matched,
            );

            by_site
                .entry(key)
                .and_modify(|(exemplar, count)| {
                    *count += 1;
                    // Whichever comes first in the tree, so the answer does
                    // not change with the order rows come back in.
                    if (&registration.file_path, registration.line)
                        < (&exemplar.file_path, exemplar.line)
                    {
                        *exemplar = registration;
                    }
                })
                .or_insert((registration, 1));
        }
    }

    for (
        (
            caller_name,
            site_file,
            site_line,
            site_byte_start,
            member,
            site_kind,
            container_type,
            type_matched,
        ),
        (exemplar, count),
    ) in by_site
    {
        found.push(IndirectCaller {
            caller_name,
            site_file,
            site_line,
            site_byte_start,
            member,
            site_kind,
            evidence: Evidence::Registered {
                container_type,
                registration_file: exemplar.file_path.clone(),
                registration_line: exemplar.line,
                registration_count: count,
                type_matched,
            },
        });
    }

    // Sort on everything that distinguishes an answer, not on the three
    // fields a reader sees first: dedup only drops neighbours, so two equal
    // rows with a third between them both survive a partial sort.
    found.sort_by(|a, b| {
        (
            &a.caller_name,
            &a.site_file,
            a.site_line,
            a.site_byte_start,
            &a.member,
            &a.site_kind,
        )
            .cmp(&(
                &b.caller_name,
                &b.site_file,
                b.site_line,
                b.site_byte_start,
                &b.member,
                &b.site_kind,
            ))
    });
    found.dedup();

    found
}

/// The struct or union a field is declared as, with the noise a declaration
/// carries stripped: `struct file_operations *` is `file_operations`.
///
/// A field declared as a plain type, a function pointer, or an array of
/// something yields nothing: only an aggregate can hold the members a
/// dispatch goes through.
pub fn aggregate_of(declared: &str) -> Option<String> {
    // A function-pointer member declares a signature, not an aggregate:
    // `int (*read)(...)` has nothing to dispatch through.
    let declared = declared
        .split('(')
        .next()
        .unwrap_or(declared)
        .replace(['*', '['], " ");

    let mut words = declared.split_whitespace().peekable();
    let mut named = None;
    while let Some(word) = words.next() {
        match word {
            "struct" | "union" => {
                named = words.peek().copied();
                break;
            }
            "const" | "volatile" | "restrict" | "_Atomic" => continue,
            // A bare name is either a typedef or a builtin. A typedef of a
            // struct is a container something can be registered under —
            // `aggregate_type_name` files registrations under the typedef
            // name, so refusing it here leaves the two halves of the join
            // unable to meet. A builtin names no aggregate and is dropped
            // below, along with anything else that is not one identifier.
            other => {
                named = Some(other);
                break;
            }
        }
    }

    named
        .filter(|name| {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_numeric())
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !is_builtin_type(name)
        })
        .map(|name| name.to_string())
}

/// A type C provides, which no struct is declared as and nothing is
/// registered under.
fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "void"
            | "char"
            | "short"
            | "int"
            | "long"
            | "float"
            | "double"
            | "signed"
            | "unsigned"
            | "bool"
            | "_Bool"
            | "size_t"
            | "ssize_t"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "s8"
            | "s16"
            | "s32"
            | "s64"
    )
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
/// The content each path holds at a revision, as the working tree shows it.
///
/// A revision is not a filter a query may apply and may skip: it decides
/// which rows are answers at all. Giving it a type means a lookup cannot be
/// written without one, and means how the paths are obtained — a whole-tree
/// walk, a lookup per path, or the index itself — is a property of this value
/// rather than of every caller.
#[derive(Debug, Default, Clone)]
pub struct RevisionPaths {
    paths: HashMap<String, String>,
}

impl RevisionPaths {
    /// Paths already resolved, with the working tree's answer preferred.
    pub fn from_map(paths: HashMap<String, String>) -> Self {
        Self { paths }
    }

    /// The content this path holds, or None when the revision does not have
    /// it. None is an answer: the row that named this path is not in this
    /// tree.
    pub fn hash_of(&self, path: &str) -> Option<&str> {
        self.paths.get(path).map(String::as_str)
    }

    /// Whether nothing at all resolved, which says the revision could not be
    /// established rather than that the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// The paths as a map, for the callers that still hand one to git.
    pub fn as_map(&self) -> &HashMap<String, String> {
        &self.paths
    }
}

pub fn at_revision<T, F>(rows: Vec<T>, paths: &RevisionPaths, key: F) -> Vec<T>
where
    F: Fn(&T) -> (&str, &str),
{
    rows.into_iter()
        .filter(|row| {
            let (file, hash) = key(row);
            paths.hash_of(file).map(|h| h == hash).unwrap_or(false)
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
            container_base_type: None,
            container_field: None,
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

    #[test]
    fn a_field_declaration_yields_the_struct_it_names() {
        assert_eq!(
            aggregate_of("struct file_operations *"),
            Some("file_operations".to_string())
        );
        assert_eq!(
            aggregate_of("const struct proto_ops *"),
            Some("proto_ops".to_string())
        );
        assert_eq!(aggregate_of("union u *"), Some("u".to_string()));
    }

    #[test]
    fn a_field_that_is_not_an_aggregate_yields_nothing() {
        // Nothing is registered under a builtin, and a function pointer
        // declares a signature rather than something to dispatch through.
        assert_eq!(aggregate_of("int"), None);
        assert_eq!(aggregate_of("unsigned long"), None);
        assert_eq!(aggregate_of("void *"), None);
        assert_eq!(aggregate_of("int (*)(void)"), None);
    }

    #[test]
    fn a_typedef_name_is_a_container_like_any_other() {
        // `aggregate_type_name` files a registration under the typedef it
        // sees, so resolution has to arrive at the same key or the two halves
        // never meet.
        assert_eq!(aggregate_of("atomic_t"), Some("atomic_t".to_string()));
        assert_eq!(
            aggregate_of("const nvkm_ior_func_bl *"),
            Some("nvkm_ior_func_bl".to_string())
        );
    }

    #[test]
    fn a_site_is_one_answer_however_many_places_install_the_target() {
        // seq_read sits in file_operations::read hundreds of times. The call
        // in vfs_read is still one call.
        let sites = group_by_member(vec![site("read", "vfs_read", Some("file_operations"))]);
        let installs: Vec<Registration> = (0..3)
            .map(|i| {
                let mut registration = registration("file_operations", "read", "seq_read");
                registration.file_path = format!("fs/{i}.c");
                registration.line = 10 + i;
                registration
            })
            .collect();

        let found = indirect_callers(&installs, &sites, &[]);

        assert_eq!(found.len(), 1, "one site, one answer: {found:?}");
        match &found[0].evidence {
            Evidence::Registered {
                registration_count,
                registration_file,
                ..
            } => {
                assert_eq!(*registration_count, 3);
                // The exemplar is the same one whichever order they arrive in.
                assert_eq!(registration_file, "fs/0.c");
            }
            other => panic!("unexpected evidence: {other:?}"),
        }
    }

    #[test]
    fn two_sites_stay_two_answers() {
        let sites = group_by_member(vec![
            site("read", "vfs_read", Some("file_operations")),
            site("read", "loop_rw_iter", Some("file_operations")),
        ]);

        let found = indirect_callers(
            &[registration("file_operations", "read", "seq_read")],
            &sites,
            &[],
        );

        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn two_dispatches_on_one_line_are_two_answers() {
        // `a->run(); b->run();` is two calls, and the answer for each has to
        // count its own installations rather than both.
        let mut first = site("run", "caller", Some("ops"));
        let mut second = site("run", "caller", Some("ops"));
        first.byte_start = 100;
        second.byte_start = 140;

        let found = indirect_callers(
            &[registration("ops", "run", "impl")],
            &group_by_member(vec![first, second]),
            &[],
        );

        assert_eq!(found.len(), 2, "one line swallowed both: {found:?}");
        for caller in &found {
            match &caller.evidence {
                Evidence::Registered {
                    registration_count, ..
                } => assert_eq!(*registration_count, 1, "count doubled: {caller:?}"),
                other => panic!("unexpected evidence: {other:?}"),
            }
        }
    }
}
