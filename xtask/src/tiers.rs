//! The check of the crate tiers, `cargo xtask ci`'s first step
//! (`docs/design/logic-foundations.md`, "The tier check in `cargo xtask
//! ci`"): every workspace member is classed in one tier, and what each
//! tier's crates may reach is read from the dependency graph, so the
//! boundaries are facts of the graph rather than conventions.
//!
//! The graph is `cargo metadata`'s, read once with no platform filter, over
//! normal and build edges, development edges left out: an edge gated on
//! `cfg(windows)` is read on every host, so the check gives the same answer
//! on Linux as on Windows.
//!
//! The rules:
//!
//! - Every workspace member is classed in exactly one of [`TIERS`]'
//!   lists, and every crate a list names is a member, because the old
//!   check's hole was a crate it did not know.
//! - A platform-neutral crate reaches only platform-neutral workspace
//!   crates, since that tier may depend on no GPL-tier crate.
//! - A logic crate reaches only platform-neutral and logic workspace
//!   crates.
//! - Neither reaches [`WINDOWS_BINDINGS`] by any path, nor a raw Windows
//!   binding ([`is_raw_windows_binding`]) directly from a workspace crate
//!   other than [`WORKSPACE_HACK`]: third-party crates with target-gated
//!   bindings are cross-platform code.
//!
//! The graph is read with every feature on, so an optional dependency
//! behind a feature no member enables is checked too.
//! - A role's crates ([`ROLES`]) reach no other role's, the rule of
//!   one-way dependencies.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// A workspace member's tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Original code only, which may depend on no GPL-tier crate and not
    /// on the Windows bindings (`CLAUDE.md`, "NVDA provenance").
    PlatformNeutral,
    /// GPL tier, may hold ported behaviour, and has no Windows dependency.
    Logic,
    /// GPL tier, Windows-specific.
    Windows,
    /// The workspace's tools and test tools, which no rule constrains.
    Tool,
}

/// Every workspace member, by tier. Adding a crate means classing it here.
pub const TIERS: &[(Tier, &[&str])] = &[
    (
        Tier::PlatformNeutral,
        &[
            "verbatim-model",
            "verbatim-core",
            "verbatim-speech",
            "verbatim-config",
            "verbatim-i18n",
            "verbatim-input",
            "verbatim-audio",
            "verbatim-text",
            // An extension's SDK never carries the GPL tier (Dickson,
            // 2026-10-31).
            "verbatim-ext-api",
            // Every crate depends on it; its `windows-sys` is covered by
            // the exception for it.
            WORKSPACE_HACK,
        ],
    ),
    (
        Tier::Logic,
        &[
            "verbatim-reader-rules",
            "verbatim-outpost-logic",
            "verbatim-listener-logic",
        ],
    ),
    (
        Tier::Windows,
        &[
            "verbatim-app",
            "verbatim-audio-wasapi",
            "verbatim-control",
            "verbatim-ext",
            "verbatim-gui",
            "verbatim-ia2",
            "verbatim-input-windows",
            "verbatim-listener",
            "verbatim-outpost",
            "verbatim-process",
            "verbatim-supervisor",
            "verbatim-synth-capture",
            "verbatim-synth-espeak",
            "verbatim-synth-host",
            "verbatim-synth-hosted",
            "verbatim-synth-onecore",
            "verbatim-uia",
            "verbatim-uia-rops",
            "verbatim-win32",
        ],
    ),
    (
        Tier::Tool,
        &[
            "xtask",
            "mockapp",
            "verbatim-agent",
            "verbatim-e2e",
            "verbatim-inspect",
            "verbatim-term-sim",
            "verbatim-test-console",
            "verbatim-trace-waits",
        ],
    ),
];

/// The roles' crates, each role's apart from every other's.
pub const ROLES: &[&[&str]] = &[
    &["verbatim-outpost", "verbatim-outpost-logic"],
    &["verbatim-listener", "verbatim-listener-logic"],
    &["verbatim-supervisor"],
];

/// The crates through which Verbatim reaches the Windows API, which no
/// platform-neutral or logic crate may reach by any path.
pub const WINDOWS_BINDINGS: &[&str] = &["windows", "windows-core"];

/// Whether `name` is a raw Windows binding, which no workspace crate a
/// platform-neutral or logic crate reaches may depend on directly:
/// `winapi`, or any `windows-` or `windows_` crate but
/// [`WINDOWS_BINDINGS`], which no path may reach at all. Named by prefix,
/// since each of them (`windows-sys`, `windows-link`, `windows-targets`
/// and the rest) can declare and call Win32 functions. Beneath a
/// third-party library they are cross-platform code, as they are beneath
/// [`WORKSPACE_HACK`], which only unifies those libraries' features.
#[must_use]
pub fn is_raw_windows_binding(name: &str) -> bool {
    !WINDOWS_BINDINGS.contains(&name)
        && (name == "winapi" || name.starts_with("windows-") || name.starts_with("windows_"))
}

/// The generated crate that unifies third-party features across the
/// workspace (`docs/tooling/setup.md`), exempt from
/// [`is_raw_windows_binding`].
pub const WORKSPACE_HACK: &str = "workspace-hack";

/// The dependency graph the rules read: each package by its id, with its
/// name and whether it is a workspace member, and its normal and build
/// edges.
#[derive(Debug, Default)]
pub struct Graph {
    /// Each package's name and whether it is a workspace member, by id.
    pub packages: BTreeMap<String, (String, bool)>,
    /// Each package's normal and build dependencies, by id.
    pub edges: BTreeMap<String, Vec<String>>,
}

impl Graph {
    /// The graph in `cargo metadata`'s JSON.
    ///
    /// # Errors
    ///
    /// When the JSON is not `cargo metadata`'s.
    pub fn from_metadata(metadata: &Value) -> Result<Self, String> {
        let members: BTreeSet<&str> = metadata["workspace_members"]
            .as_array()
            .ok_or("cargo metadata has no workspace members")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut graph = Self::default();
        for package in metadata["packages"]
            .as_array()
            .ok_or("cargo metadata has no packages")?
        {
            let id = package["id"].as_str().ok_or("a package has no id")?;
            let name = package["name"].as_str().ok_or("a package has no name")?;
            graph
                .packages
                .insert(id.to_owned(), (name.to_owned(), members.contains(id)));
        }
        for node in metadata["resolve"]["nodes"]
            .as_array()
            .ok_or("cargo metadata has no resolved graph")?
        {
            let id = node["id"].as_str().ok_or("a node has no id")?;
            let mut deps = Vec::new();
            for dep in node["deps"].as_array().map_or(&[][..], Vec::as_slice) {
                // An edge counts when any of its kinds is normal (null) or
                // build, whatever its target.
                let counts = dep["dep_kinds"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .any(|kind| kind["kind"].is_null() || kind["kind"] == "build");
                if counts && let Some(pkg) = dep["pkg"].as_str() {
                    deps.push(pkg.to_owned());
                }
            }
            graph.edges.insert(id.to_owned(), deps);
        }
        Ok(graph)
    }

    fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.packages.get(id).map_or(id, |(name, _)| name.as_str())
    }

    fn is_member(&self, id: &str) -> bool {
        self.packages.get(id).is_some_and(|(_, member)| *member)
    }

    /// Every package `from` reaches, each with the path to it from `from`,
    /// shortest first.
    fn reached(&self, from: &str) -> BTreeMap<String, Vec<String>> {
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut queue = VecDeque::from([vec![from.to_owned()]]);
        while let Some(path) = queue.pop_front() {
            let at = path.last().expect("a path is never empty").clone();
            for next in self.edges.get(&at).map_or(&[][..], Vec::as_slice) {
                if next == from || paths.contains_key(next) {
                    continue;
                }
                let mut longer = path.clone();
                longer.push(next.clone());
                paths.insert(next.clone(), longer.clone());
                queue.push_back(longer);
            }
        }
        paths
    }

    fn path_names(&self, path: &[String]) -> String {
        path.iter()
            .map(|id| self.name(id))
            .collect::<Vec<_>>()
            .join(" -> ")
    }
}

/// The tier `name` is classed in, if any.
fn tier_of(tiers: &[(Tier, &[&str])], name: &str) -> Option<Tier> {
    tiers
        .iter()
        .find(|(_, names)| names.contains(&name))
        .map(|(tier, _)| *tier)
}

/// Every member in no tier or in more than one, and every crate a tier
/// names that is no member.
fn classing_offences(members: &BTreeMap<&str, &str>, tiers: &[(Tier, &[&str])]) -> Vec<String> {
    let mut offences = Vec::new();
    for name in members.keys() {
        let classed = tiers
            .iter()
            .filter(|(_, names)| names.contains(name))
            .count();
        match classed {
            0 => offences.push(format!(
                "{name} is in no tier: class it in xtask/src/tiers.rs"
            )),
            1 => {}
            _ => offences.push(format!("{name} is classed in more than one tier")),
        }
    }
    for (_, names) in tiers {
        for name in *names {
            if !members.contains_key(name) {
                offences.push(format!("{name} is classed but is no workspace member"));
            }
        }
    }
    offences
}

/// Every offence against the rules in `graph`, with `tiers` and `roles`
/// the classes, each naming its path; none when the graph keeps them.
#[must_use]
pub fn offences(graph: &Graph, tiers: &[(Tier, &[&str])], roles: &[&[&str]]) -> Vec<String> {
    let members: BTreeMap<&str, &str> = graph
        .packages
        .iter()
        .filter(|(_, (_, member))| *member)
        .map(|(id, (name, _))| (name.as_str(), id.as_str()))
        .collect();
    let mut offences = classing_offences(&members, tiers);
    for (name, id) in &members {
        let tier = tier_of(tiers, name);
        let reached = graph.reached(id);
        if matches!(tier, Some(Tier::PlatformNeutral | Tier::Logic)) {
            for (to, path) in &reached {
                let to_name = graph.name(to);
                if graph.is_member(to) {
                    let allowed = match tier {
                        Some(Tier::PlatformNeutral) => {
                            tier_of(tiers, to_name) == Some(Tier::PlatformNeutral)
                        }
                        _ => matches!(
                            tier_of(tiers, to_name),
                            Some(Tier::PlatformNeutral | Tier::Logic)
                        ),
                    };
                    if !allowed {
                        offences.push(format!(
                            "{name}, a {} crate, reaches {to_name}: {}",
                            tier_name(tier),
                            graph.path_names(path)
                        ));
                    }
                }
                if WINDOWS_BINDINGS.contains(&to_name) {
                    offences.push(format!(
                        "{name}, a {} crate, reaches {to_name}: {}",
                        tier_name(tier),
                        graph.path_names(path)
                    ));
                }
            }
            // A raw binding directly under any workspace crate it reaches,
            // itself included, but the workspace-hack crate.
            let mut workspace_crates = vec![vec![(*id).to_owned()]];
            workspace_crates.extend(
                reached
                    .iter()
                    .filter(|(to, _)| graph.is_member(to))
                    .map(|(_, path)| path.clone()),
            );
            for path in workspace_crates {
                let at = path.last().expect("a path is never empty");
                if graph.name(at) == WORKSPACE_HACK {
                    continue;
                }
                for dep in graph.edges.get(at).map_or(&[][..], Vec::as_slice) {
                    let dep_name = graph.name(dep);
                    if is_raw_windows_binding(dep_name) {
                        let mut full = path.clone();
                        full.push(dep.clone());
                        offences.push(format!(
                            "{name}, a {} crate, reaches {dep_name} through {}, a workspace crate: {}",
                            tier_name(tier),
                            graph.name(at),
                            graph.path_names(&full)
                        ));
                    }
                }
            }
        }
        if let Some(role) = roles.iter().find(|role| role.contains(name)) {
            for (to, path) in &reached {
                let to_name = graph.name(to);
                if graph.is_member(to)
                    && !role.contains(&to_name)
                    && roles.iter().any(|other| other.contains(&to_name))
                {
                    offences.push(format!(
                        "{name} reaches another role's crate, {to_name}: {}",
                        graph.path_names(path)
                    ));
                }
            }
        }
    }
    offences.sort();
    offences.dedup();
    offences
}

fn tier_name(tier: Option<Tier>) -> &'static str {
    match tier {
        Some(Tier::PlatformNeutral) => "platform-neutral",
        Some(Tier::Logic) => "logic",
        Some(Tier::Windows) => "Windows",
        Some(Tier::Tool) => "tool",
        None => "unclassed",
    }
}

/// Runs the check on the workspace at `root`.
///
/// # Errors
///
/// When `cargo metadata` fails, or the graph breaks a rule: the error names
/// every offence and its path.
pub fn check(root: &Path) -> Result<(), String> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--all-features"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("could not run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata's output is not JSON: {error}"))?;
    let graph = Graph::from_metadata(&metadata)?;
    let offences = offences(&graph, TIERS, ROLES);
    if offences.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the crate tiers are broken (see the NVDA provenance section of CLAUDE.md):\n  {}",
            offences.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph of `members` and third-party `others`, with `edges` between
    /// them by name, each package's id its name.
    fn graph(members: &[&str], others: &[&str], edges: &[(&str, &str)]) -> Graph {
        let mut graph = Graph::default();
        for (names, member) in [(members, true), (others, false)] {
            for name in names {
                graph
                    .packages
                    .insert((*name).to_owned(), ((*name).to_owned(), member));
                graph.edges.insert((*name).to_owned(), Vec::new());
            }
        }
        for (from, to) in edges {
            graph
                .edges
                .get_mut(*from)
                .expect("a package of the graph")
                .push((*to).to_owned());
        }
        graph
    }

    const TEST_TIERS: &[(Tier, &[&str])] = &[
        (Tier::PlatformNeutral, &["model", "core", WORKSPACE_HACK]),
        (Tier::Logic, &["outpost-logic", "listener-logic"]),
        (Tier::Windows, &["outpost", "listener", "win32"]),
        (Tier::Tool, &["xtask"]),
    ];

    const TEST_ROLES: &[&[&str]] = &[
        &["outpost", "outpost-logic"],
        &["listener", "listener-logic"],
    ];

    const ALL: &[&str] = &[
        "model",
        "core",
        WORKSPACE_HACK,
        "outpost-logic",
        "listener-logic",
        "outpost",
        "listener",
        "win32",
        "xtask",
    ];

    #[test]
    fn a_member_in_no_tier_is_an_offence() {
        let mut members = ALL.to_vec();
        members.push("gui");
        assert_eq!(
            offences(&graph(&members, &[], &[]), TEST_TIERS, TEST_ROLES),
            ["gui is in no tier: class it in xtask/src/tiers.rs"]
        );
    }

    #[test]
    fn a_platform_neutral_crate_may_reach_no_logic_crate() {
        let graph = graph(ALL, &[], &[("core", "model"), ("model", "outpost-logic")]);
        assert_eq!(
            offences(&graph, TEST_TIERS, TEST_ROLES),
            [
                "core, a platform-neutral crate, reaches outpost-logic: core -> model -> outpost-logic",
                "model, a platform-neutral crate, reaches outpost-logic: model -> outpost-logic",
            ]
        );
    }

    #[test]
    fn a_logic_crate_may_reach_no_windows_crate() {
        let graph = graph(
            ALL,
            &[],
            &[("outpost-logic", "core"), ("outpost-logic", "win32")],
        );
        assert_eq!(
            offences(&graph, TEST_TIERS, TEST_ROLES),
            ["outpost-logic, a logic crate, reaches win32: outpost-logic -> win32"]
        );
    }

    #[test]
    fn no_logic_or_platform_neutral_crate_reaches_the_windows_bindings_by_any_path() {
        let graph = graph(
            ALL,
            &["chrono", "windows-core", "windows-sys", "anstream"],
            &[
                ("model", "chrono"),
                ("chrono", "windows-core"),
                ("listener-logic", "windows-sys"),
                ("core", "anstream"),
                ("anstream", "windows-sys"),
                (WORKSPACE_HACK, "windows-sys"),
            ],
        );
        assert_eq!(
            offences(&graph, TEST_TIERS, TEST_ROLES),
            [
                "listener-logic, a logic crate, reaches windows-sys through listener-logic, a workspace crate: listener-logic -> windows-sys",
                "model, a platform-neutral crate, reaches windows-core: model -> chrono -> windows-core",
            ]
        );
    }

    #[test]
    fn every_raw_windows_binding_is_named_by_its_prefix() {
        let graph = graph(
            ALL,
            &["windows-link", "windows-targets", "winapi", "windows-core"],
            &[
                ("model", "windows-link"),
                ("outpost-logic", "windows-targets"),
                ("core", "winapi"),
                (WORKSPACE_HACK, "windows-link"),
            ],
        );
        assert_eq!(
            offences(&graph, TEST_TIERS, TEST_ROLES),
            [
                "core, a platform-neutral crate, reaches winapi through core, a workspace crate: core -> winapi",
                "model, a platform-neutral crate, reaches windows-link through model, a workspace crate: model -> windows-link",
                "outpost-logic, a logic crate, reaches windows-targets through outpost-logic, a workspace crate: outpost-logic -> windows-targets",
            ]
        );
    }

    #[test]
    fn a_roles_crate_may_reach_no_other_roles() {
        let graph = graph(
            ALL,
            &[],
            &[
                ("outpost", "outpost-logic"),
                ("outpost", "win32"),
                ("listener", "outpost-logic"),
            ],
        );
        assert_eq!(
            offences(&graph, TEST_TIERS, TEST_ROLES),
            ["listener reaches another role's crate, outpost-logic: listener -> outpost-logic"]
        );
    }
}
