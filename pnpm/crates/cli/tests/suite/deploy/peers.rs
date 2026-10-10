use super::{
    AddMockedRegistry, CommandExtra, CommandTempCwd, Lockfile, PkgName, fs, pacquet_cmd,
    write_ambiguous_peer_workspace, write_peer_workspace, write_project,
};
use assert_cmd::assert::OutputAssertExt;

/// The workspace resolves `lib`'s peer from `lib`'s own devDependencies, which
/// a production deploy leaves behind. The deployed graph carries exactly one
/// resolution of that peer, so the synthesized snapshot can bind it.
#[test]
fn shared_lockfile_deploy_binds_a_singleton_peer_of_a_linked_workspace_package() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_peer_workspace(&workspace);

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    let lib_real = fs::canonicalize(deploy_dir.join("node_modules/lib")).unwrap();
    let peer = lib_real
        .parent()
        .unwrap()
        .join("@pnpm.e2e/peer-a");
    assert!(peer.exists(), "the deployed workspace package should resolve its peer");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fs::canonicalize(&peer).unwrap().join("package.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["version"], "1.0.0");

    drop((root, mock_instance));
}

/// Deploy does not adjudicate peer ranges. Injecting the package binds the
/// consumer's version even when it falls outside the declared range — pnpm
/// treats that as a resolution-time warning — so the non-injected path binds
/// it too rather than inventing a stricter rule for linked packages.
#[test]
fn shared_lockfile_deploy_binds_a_singleton_peer_outside_the_declared_range() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_peer_workspace(&workspace);
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            // app pins 1.0.0, which does not satisfy this.
            "peerDependencies": { "@pnpm.e2e/peer-a": "1.0.1" },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    let lib_real = fs::canonicalize(deploy_dir.join("node_modules/lib")).unwrap();
    let peer = lib_real
        .parent()
        .unwrap()
        .join("@pnpm.e2e/peer-a");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fs::canonicalize(&peer).unwrap().join("package.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["version"], "1.0.0");

    drop((root, mock_instance));
}

/// No ancestor of `lib` depends on its peer, and the deployed graph holds two
/// versions of it, so nothing says which one injecting `lib` would bind.
#[test]
fn shared_lockfile_deploy_refuses_a_linked_workspace_package_with_an_ambiguous_peer() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_ambiguous_peer_workspace(&workspace);
    write_project(
        &workspace,
        "other-v1-0-0",
        &serde_json::json!({
            "name": "other-v1-0-0",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": { "@pnpm.e2e/peer-a": "1.0.0" },
        }),
    );
    write_project(
        &workspace,
        "app",
        &serde_json::json!({
            "name": "app",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": {
                "lib": "workspace:*",
                "other": "workspace:*",
                "other-v1-0-0": "workspace:*",
            },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    let output = pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .output()
        .expect("run pacquet deploy");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The rendered wording is shared with the TypeScript CLI's
    // ERR_PNPM_DEPLOY_AMBIGUOUS_PEER; keep the two in step.
    for expected in [
        "ERR_PNPM_DEPLOY_AMBIGUOUS_PEER",
        "Workspace package 'lib' declares a peer dependency on '@pnpm.e2e/peer-a'",
        "more than one version (1.0.0, 1.0.1)",
        r#"Pin '@pnpm.e2e/peer-a' to a single version with an "overrides" entry"#,
    ] {
        assert!(stderr.contains(expected), "stderr should mention {expected}:\n{stderr}");
    }

    drop((root, mock_instance));
}

/// `app` depends on the peer itself, so injecting `lib` would bind `app`'s
/// version, whichever other versions the deployed graph holds.
#[test]
fn shared_lockfile_deploy_binds_a_linked_peer_to_the_version_its_parent_provides() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_ambiguous_peer_workspace(&workspace);

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    assert_eq!(dependency_version(&deploy_dir, "lib", "@pnpm.e2e/peer-a"), "1.0.0");
    assert_eq!(dependency_version(&deploy_dir, "other", "@pnpm.e2e/peer-a"), "1.0.1");

    drop((root, mock_instance));
}

/// `lib`'s injected resolution matches its own importer, dev dependencies
/// included, so the injected workspace links it into `app`. The deploy binds
/// the peer to that shared resolution, even though `other` brings a second
/// version of the peer into the deployed graph.
#[test]
fn injected_workspace_deploy_binds_the_peer_of_a_deduped_workspace_package() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_ambiguous_peer_workspace(&workspace);
    let workspace_yaml = fs::read_to_string(workspace.join("pnpm-workspace.yaml"))
        .unwrap()
        .replace("injectWorkspacePackages: false", "injectWorkspacePackages: true");
    fs::write(workspace.join("pnpm-workspace.yaml"), workspace_yaml).unwrap();
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "@pnpm.e2e/peer-a": "*" },
            "devDependencies": { "@pnpm.e2e/peer-a": "1.0.0" },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let workspace_lockfile = Lockfile::load_wanted_from_dir(&workspace).unwrap().unwrap();
    let lib: PkgName = "lib".parse().unwrap();
    let lib_version =
        &workspace_lockfile.importers["packages/app"].dependencies.as_ref().unwrap()[&lib].version;
    assert_eq!(lib_version.to_string(), "link:../lib");
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    let lib_real = fs::canonicalize(deploy_dir.join("node_modules/lib")).unwrap();
    let peer = lib_real
        .parent()
        .unwrap()
        .join("@pnpm.e2e/peer-a");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fs::canonicalize(&peer).unwrap().join("package.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["version"], "1.0.0");

    drop((root, mock_instance));
}

/// A dev dependency that links outside the workspace cannot name a deployed
/// snapshot, so it never blocks the deploy: not as an unrelated dev dependency
/// of `lib`, not as `lib`'s peer, and not as the peer of `tool`, which the
/// deploy does not include.
#[test]
fn injected_workspace_deploy_ignores_external_dev_links_of_linked_workspace_packages() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_peer_workspace(&workspace);
    let workspace_yaml = fs::read_to_string(workspace.join("pnpm-workspace.yaml"))
        .unwrap()
        .replace("injectWorkspacePackages: false", "injectWorkspacePackages: true");
    fs::write(workspace.join("pnpm-workspace.yaml"), workspace_yaml).unwrap();
    let external = root.path().join("external");
    fs::create_dir_all(&external).unwrap();
    fs::write(
        external.join("package.json"),
        serde_json::json!({ "name": "external", "version": "1.0.0" }).to_string(),
    )
    .unwrap();
    let external_link = format!("link:{}", external.display());
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "@pnpm.e2e/peer-a": "*", "external": "*" },
            "devDependencies": {
                "@pnpm.e2e/peer-a": "1.0.0",
                "external": external_link,
                "other-external": external_link,
            },
        }),
    );
    write_project(
        &workspace,
        "tool",
        &serde_json::json!({
            "name": "tool",
            "version": "1.0.0",
            "peerDependencies": { "external": "*" },
            "devDependencies": { "external": external_link },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    assert!(deploy_dir.join("node_modules/lib").exists());

    drop((root, mock_instance));
}

/// The remedy `ERR_PNPM_DEPLOY_AMBIGUOUS_PEER` suggests: collapsing the peer to
/// one version makes the binding unambiguous, so the deploy goes through
/// without injecting the workspace or falling back to the legacy implementation.
#[test]
fn an_override_collapsing_the_peer_unblocks_a_non_injected_deploy() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_ambiguous_peer_workspace(&workspace);
    let mut workspace_yaml = fs::read_to_string(workspace.join("pnpm-workspace.yaml")).unwrap();
    workspace_yaml.push_str("overrides:\n  '@pnpm.e2e/peer-a': 1.0.0\n");
    fs::write(workspace.join("pnpm-workspace.yaml"), workspace_yaml).unwrap();

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    let lib_real = fs::canonicalize(deploy_dir.join("node_modules/lib")).unwrap();
    let peer = lib_real
        .parent()
        .unwrap()
        .join("@pnpm.e2e/peer-a");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fs::canonicalize(&peer).unwrap().join("package.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["version"], "1.0.0");

    drop((root, mock_instance));
}

/// A peer that the package also declares as an optional dependency is already
/// bound. Re-binding it would copy it into the required map and quietly promote
/// it, changing what `--no-optional` and a failed fetch mean for it.
#[test]
fn shared_lockfile_deploy_keeps_an_optional_peer_optional() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_peer_workspace(&workspace);
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "@pnpm.e2e/peer-a": "*" },
            "optionalDependencies": { "@pnpm.e2e/peer-a": "1.0.0" },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    let deploy_lockfile = Lockfile::load_wanted_from_dir(&deploy_dir).unwrap().unwrap();
    let peer: PkgName = "@pnpm.e2e/peer-a".parse().unwrap();
    let lib = deploy_lockfile.snapshots
        .as_ref()
        .expect("deploy snapshots")
        .iter()
        .find(|(key, _)| key.name.to_string() == "lib")
        .map(|(_, snapshot)| snapshot)
        .expect("the deployed lib snapshot");
    assert!(
        lib.optional_dependencies
            .as_ref()
            .is_some_and(|deps| deps.contains_key(&peer)),
        "the peer should stay in the optional map: {lib:#?}",
    );
    assert!(
        !lib.dependencies
            .as_ref()
            .is_some_and(|deps| deps.contains_key(&peer)),
        "the peer should not also be copied into the required map: {lib:#?}",
    );

    drop((root, mock_instance));
}

/// `--no-optional` clears the optional map before the binding step, so the
/// binder cannot see that the peer was already bound by an optional edge.
/// Re-binding it there would resurrect a dependency the flag excluded.
#[test]
fn shared_lockfile_deploy_does_not_resurrect_an_excluded_optional_peer() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_peer_workspace(&workspace);
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "@pnpm.e2e/peer-a": "*" },
            "optionalDependencies": { "@pnpm.e2e/peer-a": "1.0.0" },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--no-optional"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    let deploy_lockfile = Lockfile::load_wanted_from_dir(&deploy_dir).unwrap().unwrap();
    let peer: PkgName = "@pnpm.e2e/peer-a".parse().unwrap();
    let lib = deploy_lockfile.snapshots
        .as_ref()
        .expect("deploy snapshots")
        .iter()
        .find(|(key, _)| key.name.to_string() == "lib")
        .map(|(_, snapshot)| snapshot)
        .expect("the deployed lib snapshot");
    assert!(
        !lib.dependencies
            .as_ref()
            .is_some_and(|deps| deps.contains_key(&peer)),
        "an excluded optional peer must not come back as a required dependency: {lib:#?}",
    );

    drop((root, mock_instance));
}

/// `lib` declares `workspace:^` for its peer and lists the same as a dev
/// dependency, which the source lockfile records only in `lib`'s
/// devDependencies. `app` depends on the workspace project itself, and `other`
/// brings a registry copy into the deployed graph. `lib` binds to what `app`
/// provides, and `other` keeps its registry copy.
#[test]
fn shared_lockfile_deploy_binds_a_workspace_protocol_peer_to_the_workspace_project_its_parent_provides()
 {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_workspace_protocol_peer_workspace(&workspace);

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let workspace_lockfile = Lockfile::load_wanted_from_dir(&workspace).unwrap().unwrap();
    let peer_a: PkgName = "@pnpm.e2e/peer-a".parse().unwrap();
    let lib_importer = &workspace_lockfile.importers["packages/lib"];
    assert!(
        lib_importer.dependencies
            .as_ref()
            .is_none_or(|dependencies| !dependencies.contains_key(&peer_a)),
        "the source lockfile should not record the peer as a prod dependency of lib",
    );
    let dev_version = &lib_importer.dev_dependencies.as_ref().unwrap()[&peer_a].version;
    assert_eq!(dev_version.to_string(), "link:../peer-a");
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    assert_eq!(dependency_version(&deploy_dir, "lib", "@pnpm.e2e/peer-a"), "2.0.0");
    assert_eq!(dependency_version(&deploy_dir, "other", "@pnpm.e2e/peer-a"), "1.0.1");

    drop((root, mock_instance));
}

/// `app` depends on a registry copy of the peer while `other` brings the
/// workspace project into the deployed graph. `lib`'s `workspace:` peer binds
/// to what its parent provides, as a direct dependency overrides the peer.
#[test]
fn shared_lockfile_deploy_binds_a_workspace_protocol_peer_to_the_registry_copy_its_parent_provides()
{
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_workspace_protocol_peer_workspace(&workspace);
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "@pnpm.e2e/peer-a": "workspace:^" },
        }),
    );
    write_project(
        &workspace,
        "other",
        &serde_json::json!({
            "name": "other",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": { "@pnpm.e2e/peer-a": "workspace:^" },
        }),
    );
    write_project(
        &workspace,
        "app",
        &serde_json::json!({
            "name": "app",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": {
                "lib": "workspace:*",
                "other": "workspace:*",
                "@pnpm.e2e/peer-a": "1.0.0",
            },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    assert_eq!(dependency_version(&deploy_dir, "lib", "@pnpm.e2e/peer-a"), "1.0.0");
    assert_eq!(dependency_version(&deploy_dir, "other", "@pnpm.e2e/peer-a"), "2.0.0");

    drop((root, mock_instance));
}

/// `app` provides the peer under the alias `lib` declares it with, so the
/// binding keeps the alias and resolves to the project the alias names.
#[test]
fn shared_lockfile_deploy_binds_an_aliased_peer_to_what_its_parent_provides() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_workspace_protocol_peer_workspace(&workspace);
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "compat": "workspace:@pnpm.e2e/peer-a@*" },
            "devDependencies": { "compat": "workspace:@pnpm.e2e/peer-a@*" },
        }),
    );
    write_project(
        &workspace,
        "app",
        &serde_json::json!({
            "name": "app",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": {
                "lib": "workspace:*",
                "other": "workspace:*",
                "compat": "workspace:@pnpm.e2e/peer-a@*",
            },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    assert_eq!(dependency_version(&deploy_dir, "lib", "compat"), "2.0.0");

    drop((root, mock_instance));
}

/// `mid` does not depend on `lib`'s peer, so the search continues to `app`,
/// which does.
#[test]
fn shared_lockfile_deploy_binds_a_linked_peer_to_what_an_ancestor_provides() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_workspace_protocol_peer_workspace(&workspace);
    write_project(
        &workspace,
        "mid",
        &serde_json::json!({
            "name": "mid",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": { "lib": "workspace:*" },
        }),
    );
    write_project(
        &workspace,
        "app",
        &serde_json::json!({
            "name": "app",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": {
                "mid": "workspace:*",
                "other": "workspace:*",
                "@pnpm.e2e/peer-a": "workspace:^",
            },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    let lib_dir = fs::canonicalize(deploy_dir.join("node_modules/mid"))
        .unwrap()
        .parent()
        .unwrap()
        .join("lib");
    let peer_manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            fs::canonicalize(lib_dir)
                .unwrap()
                .parent()
                .unwrap()
                .join("@pnpm.e2e/peer-a/package.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(peer_manifest["version"], "2.0.0");

    drop((root, mock_instance));
}

/// `mid` and `mid-2` both provide `lib`'s peer, as two different packages at
/// the same version, so injecting `lib` under each would bind a different one.
#[test]
fn shared_lockfile_deploy_refuses_a_peer_its_parents_provide_as_different_packages() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_peer_workspace(&workspace);
    for (name, peer) in [("mid", "1.0.0"), ("mid-2", "npm:@pnpm.e2e/peer-b@1.0.0")] {
        write_project(
            &workspace,
            name,
            &serde_json::json!({
                "name": name,
                "version": "1.0.0",
                "files": ["index.js"],
                "dependencies": { "lib": "workspace:*", "@pnpm.e2e/peer-a": peer },
            }),
        );
    }
    write_project(
        &workspace,
        "app",
        &serde_json::json!({
            "name": "app",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": { "mid": "workspace:*", "mid-2": "workspace:*" },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    let output = pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .output()
        .expect("run pacquet deploy");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in ["ERR_PNPM_DEPLOY_AMBIGUOUS_PEER", "(1.0.0, @pnpm.e2e/peer-b@1.0.0)"] {
        assert!(stderr.contains(expected), "stderr should mention {expected}:\n{stderr}");
    }

    drop((root, mock_instance));
}

/// Without a dev dependency on its peer, `autoInstallPeers` installs it as a
/// dependency of `lib`, which binds the peer before the deploy has to.
#[test]
fn shared_lockfile_deploy_keeps_an_auto_installed_workspace_protocol_peer() {
    let CommandTempCwd {
        pacquet,
        root,
        workspace,
        npmrc_info,
        ..
    } = CommandTempCwd::init().add_mocked_registry();
    let AddMockedRegistry { mock_instance, .. } = npmrc_info;
    write_workspace_protocol_peer_workspace(&workspace);
    let workspace_yaml = fs::read_to_string(workspace.join("pnpm-workspace.yaml"))
        .unwrap()
        .replace("autoInstallPeers: false", "autoInstallPeers: true");
    fs::write(workspace.join("pnpm-workspace.yaml"), workspace_yaml).unwrap();
    write_project(
        &workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "@pnpm.e2e/peer-a": "workspace:^" },
        }),
    );

    pacquet
        .with_arg("install")
        .assert()
        .success();
    let deploy_dir = fs::canonicalize(root.path()).unwrap().join("deploy");
    pacquet_cmd(&workspace)
        .with_args(["--filter", "app", "deploy", "--prod"])
        .with_arg(&deploy_dir)
        .assert()
        .success();

    assert_eq!(dependency_version(&deploy_dir, "lib", "@pnpm.e2e/peer-a"), "2.0.0");

    drop((root, mock_instance));
}

/// `app` depends on the workspace copy of `@pnpm.e2e/peer-a` and on `lib`,
/// whose peer and dev dependency on it are both `workspace:^`. `other`
/// depends on a registry copy.
fn write_workspace_protocol_peer_workspace(workspace: &std::path::Path) {
    write_ambiguous_peer_workspace(workspace);
    write_project(
        workspace,
        "peer-a",
        &serde_json::json!({
            "name": "@pnpm.e2e/peer-a",
            "version": "2.0.0",
            "files": ["index.js"],
        }),
    );
    write_project(
        workspace,
        "lib",
        &serde_json::json!({
            "name": "lib",
            "version": "1.0.0",
            "files": ["index.js"],
            "peerDependencies": { "@pnpm.e2e/peer-a": "workspace:^" },
            "devDependencies": { "@pnpm.e2e/peer-a": "workspace:^" },
        }),
    );
    write_project(
        workspace,
        "app",
        &serde_json::json!({
            "name": "app",
            "version": "1.0.0",
            "files": ["index.js"],
            "dependencies": {
                "lib": "workspace:*",
                "other": "workspace:*",
                "@pnpm.e2e/peer-a": "workspace:^",
            },
        }),
    );
}

/// The version of the package the deployed `package` resolves as `dependency`.
fn dependency_version(
    deploy_dir: &std::path::Path,
    package: &str,
    dependency: &str,
) -> serde_json::Value {
    let package_real = fs::canonicalize(deploy_dir.join("node_modules").join(package)).unwrap();
    let resolved = package_real
        .parent()
        .unwrap()
        .join(dependency);
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fs::canonicalize(&resolved).unwrap().join("package.json")).unwrap(),
    )
    .unwrap();
    manifest["version"].clone()
}
