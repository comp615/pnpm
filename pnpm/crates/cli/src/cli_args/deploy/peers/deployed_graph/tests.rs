use super::{DeployedGraph, ancestor_peer_providers, dependents_by_snapshot};
use pnpm_lockfile::{PkgName, PkgNameVerPeer, SnapshotDepRef, SnapshotEntry};
use pretty_assertions::assert_eq;
use std::collections::HashMap;

fn key(text: &str) -> PkgNameVerPeer {
    text.parse().unwrap()
}

fn reference(text: &str) -> SnapshotDepRef {
    text.parse().unwrap()
}

/// A snapshot depending on `dependencies`, each an `(alias, reference)` pair.
fn snapshot(dependencies: &[(&str, &str)]) -> SnapshotEntry {
    SnapshotEntry {
        dependencies: Some(
            dependencies
                .iter()
                .map(|(alias, target)| (alias.parse().unwrap(), reference(target)))
                .collect(),
        ),
        ..SnapshotEntry::default()
    }
}

/// The providers of `peer` for `lib@1.0.0`, which `mid@1.0.0` and
/// `mid-2@1.0.0` depend on.
fn providers(mid: &[(&str, &str)], mid_2: &[(&str, &str)]) -> Vec<SnapshotDepRef> {
    let snapshots = HashMap::from([
        (key("lib@1.0.0"), SnapshotEntry::default()),
        (key("mid@1.0.0"), snapshot(mid)),
        (key("mid-2@1.0.0"), snapshot(mid_2)),
    ]);
    let graph = DeployedGraph {
        importer: None,
        snapshots: &snapshots,
        candidates: HashMap::default(),
        dependents: dependents_by_snapshot(None, &snapshots),
    };
    let peer: PkgName = "peer".parse().unwrap();
    ancestor_peer_providers(&graph, &key("lib@1.0.0"), &peer)
}

#[test]
fn two_spellings_of_one_package_count_once() {
    let found = providers(
        &[("lib", "1.0.0"), ("peer", "1.0.0")],
        &[("lib", "1.0.0"), ("peer", "peer@1.0.0")],
    );
    assert_eq!(found.len(), 1);
}

#[test]
fn two_packages_at_one_version_are_distinct() {
    let found = providers(
        &[("lib", "1.0.0"), ("peer", "1.0.0")],
        &[("lib", "1.0.0"), ("peer", "other@1.0.0")],
    );
    assert_eq!(found.len(), 2);
}

/// A `<root>/` link points into the package that declares it, so it names a
/// different package than any other provider.
#[test]
fn a_link_into_the_providing_package_is_its_own_provider() {
    let found = providers(
        &[("lib", "1.0.0"), ("peer", "link:<root>/vendor/peer")],
        &[("lib", "1.0.0"), ("peer", "1.0.0")],
    );
    assert_eq!(found.len(), 2);
}
