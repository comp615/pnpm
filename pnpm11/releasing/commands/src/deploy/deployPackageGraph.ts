import * as dp from '@pnpm/deps.path'
import { PnpmError } from '@pnpm/error'
import {
  isPeerSatisfactionEdge,
  type PeerSatisfactionEdges,
  pruneDanglingPeerSatisfactionEdges,
} from '@pnpm/lockfile.peer-edges'
import type {
  PackageSnapshot,
  PackageSnapshots,
  ProjectSnapshot,
  ResolvedDependencies,
} from '@pnpm/lockfile.types'
import type { DependenciesField, DepPath, ProjectManifest } from '@pnpm/types'

export interface FilterDeployPackageSnapshotsOptions {
  include: { [dependenciesField in DependenciesField]: boolean }
  peerSatisfactionEdges: PeerSatisfactionEdges
}

/** Takes ownership of `packages`: the retained snapshots are edited in place. */
export function filterDeployPackageSnapshots (
  importer: ProjectSnapshot,
  packages: PackageSnapshots,
  opts: FilterDeployPackageSnapshotsOptions
): PackageSnapshots {
  const { include, peerSatisfactionEdges } = opts
  const reachable = collectReachableDepPaths(importer, packages, opts)
  return pruneDanglingPeerSatisfactionEdges(Object.fromEntries(
    Array.from(reachable, (depPath) => {
      const snapshot = packages[depPath]
      // A retained snapshot's optional edges point at packages this filter just dropped.
      if (!include.optionalDependencies) snapshot.optionalDependencies = undefined
      return [depPath, snapshot]
    })
  ) as PackageSnapshots, peerSatisfactionEdges)
}

function collectReachableDepPaths (
  importer: ProjectSnapshot,
  packages: PackageSnapshots,
  opts: FilterDeployPackageSnapshotsOptions
): Set<DepPath> {
  const queue: DepPath[] = []
  const enqueue = (dependencies: ResolvedDependencies | undefined, parent?: DepPath) => {
    queue.push(...listRetainedEdgeTargets(dependencies, { packages, parent, peerSatisfactionEdges: opts.peerSatisfactionEdges }))
  }

  enqueue(importer.dependencies)
  enqueue(importer.devDependencies)
  enqueue(importer.optionalDependencies)

  const reachable = new Set<DepPath>()
  let head = 0
  while (head < queue.length) {
    const depPath = queue[head++]!
    if (reachable.has(depPath)) continue
    reachable.add(depPath)

    const snapshot = packages[depPath]
    if (snapshot == null) continue
    enqueue(snapshot.dependencies, depPath)
    if (opts.include.optionalDependencies) enqueue(snapshot.optionalDependencies, depPath)
  }
  return reachable
}

function listRetainedEdgeTargets (
  dependencies: ResolvedDependencies | undefined,
  ctx: {
    packages: PackageSnapshots
    parent?: DepPath
    peerSatisfactionEdges: PeerSatisfactionEdges
  }
): DepPath[] {
  const targets: DepPath[] = []
  for (const [alias, reference] of Object.entries(dependencies ?? {})) {
    if (ctx.parent != null && isPeerSatisfactionEdge(ctx.peerSatisfactionEdges, ctx.parent, alias)) continue
    const depPath = dp.refToRelative(reference, alias)
    if (depPath != null && ctx.packages[depPath] != null) targets.push(depPath)
  }
  return targets
}

export interface LinkedWorkspaceProject {
  manifest: ProjectManifest
  /**
   * The project's dev dependencies that are also its peers, recorded only in
   * an injected workspace.
   * There a workspace package is linked rather than injected only when its
   * injected resolution matched its own importer, dev dependencies included,
   * so a peer it also lists as a dev dependency was bound to exactly that.
   */
  dedupedPeerResolutions: ResolvedDependencies | undefined
}

/**
 * Resolves the peer dependencies of linked workspace packages against the
 * deployed graph, editing the snapshots in `packages` in place.
 *
 * A linked workspace package has no package snapshot in the shared lockfile, so
 * the importer its deployed snapshot is synthesized from carries no peer
 * bindings and they cannot be recovered afterwards. Each one is bound the way
 * injecting the package would bind it: to what its parents provide, see
 * {@link findAncestorPeerReferences}. A peer no ancestor provides binds to the
 * deployed graph's own resolution. A peer already bound by either dependency
 * map, or absent from the deployed graph entirely, is left alone.
 *
 * @throws PnpmError DEPLOY_AMBIGUOUS_PEER when the ancestors, or else the
 * deployed graph, offer more than one resolution for a peer, since choosing
 * between them is precisely the decision injecting the package would have made.
 */
export function bindSingletonPeers (
  importer: ProjectSnapshot,
  packages: PackageSnapshots,
  linkedWorkspaceProjects: Map<DepPath, LinkedWorkspaceProject>
): void {
  if (linkedWorkspaceProjects.size === 0) return

  const graph: DeployedGraph = {
    importer,
    packages,
    references: collectReferencesByName(importer, packages),
    dependents: collectDependents(importer, packages),
  }
  for (const [depPath, linkedProject] of linkedWorkspaceProjects) {
    const snapshot = packages[depPath]
    if (snapshot == null) continue
    bindPeersOfLinkedProject(snapshot, { depPath, linkedProject, graph })
  }
}

/** The deployed project's node in {@link DeployedGraph.dependents}. */
const IMPORTER = Symbol('importer')

type Dependent = DepPath | typeof IMPORTER

interface DeployedGraph {
  importer: ProjectSnapshot
  packages: PackageSnapshots
  references: Map<string, Set<string>>
  /** The nodes that depend on each snapshot. */
  dependents: Map<DepPath, Dependent[]>
}

interface BindPeersContext {
  depPath: DepPath
  linkedProject: LinkedWorkspaceProject
  graph: DeployedGraph
}

function bindPeersOfLinkedProject (snapshot: PackageSnapshot, ctx: BindPeersContext): void {
  const { manifest } = ctx.linkedProject
  for (const peerName of Object.keys(manifest.peerDependencies ?? {})) {
    // Consult the manifest, not just the snapshot: the graph prune clears the
    // optional map before this runs, so a peer the package depends on
    // optionally is invisible in the snapshot under `--no-optional`, and
    // binding it there would resurrect a dependency the flag excluded.
    if (declaresDependency(manifest, peerName)) continue
    if (declaresDependency(snapshot, peerName)) continue
    const reference = pickPeerReference(peerName, ctx)
    if (reference == null) continue
    snapshot.dependencies = { ...snapshot.dependencies, [peerName]: reference }
  }
}

function pickPeerReference (peerName: string, ctx: BindPeersContext): string | undefined {
  const dedupedReference = findDedupedPeerReference(peerName, ctx)
  if (dedupedReference != null) return dedupedReference
  const ancestorReferences = findAncestorPeerReferences(peerName, ctx)
  if (ancestorReferences.size === 1) return Array.from(ancestorReferences.values())[0]
  if (ancestorReferences.size > 1) {
    throw ambiguousPeerError(peerName, Array.from(ancestorReferences.values(), reference => referenceVersion(reference, peerName)), ctx)
  }
  const candidates = ctx.graph.references.get(peerName)
  // A peer the deployed graph does not provide at all stays unresolved,
  // exactly as it is in the workspace this deploy was taken from.
  if (candidates == null) return undefined
  if (candidates.size > 1) throw ambiguousPeerError(peerName, Array.from(candidates), ctx)
  return Array.from(candidates)[0]
}

function ambiguousPeerError (peerName: string, versions: string[], ctx: BindPeersContext): PnpmError {
  return new PnpmError('DEPLOY_AMBIGUOUS_PEER', `Workspace package '${ctx.linkedProject.manifest.name ?? ctx.depPath}' declares a peer dependency on '${peerName}', which resolves to more than one version (${versions.sort().join(', ')}) in the deployed graph. Without "injectWorkspacePackages" there is no snapshot to bind it to.`, {
    hint: `Pin '${peerName}' to a single version with an "overrides" entry, set "injectWorkspacePackages" to true, or run "pnpm deploy" with the "--legacy" flag.`,
  })
}

function findDedupedPeerReference (peerName: string, ctx: BindPeersContext): string | undefined {
  const { dedupedPeerResolutions } = ctx.linkedProject
  if (dedupedPeerResolutions == null || !Object.hasOwn(dedupedPeerResolutions, peerName)) return undefined
  const dedupedReference = dedupedPeerResolutions[peerName]
  if (dedupedReference == null) return undefined
  const dedupedDepPath = dp.refToRelative(dedupedReference, peerName)
  return dedupedDepPath != null && ctx.graph.packages[dedupedDepPath] != null ? dedupedReference : undefined
}

/**
 * The references the nearest ancestors of the linked package that depend on
 * `peerName` resolve it to, one per path up the graph, the same way injecting
 * the package would resolve its peer from its parents. An ancestor that does
 * not depend on `peerName` passes the search on to its own dependents.
 *
 * Keyed by the dependency path the reference resolves to, so two spellings of
 * one package count once and two packages at one version do not.
 */
function findAncestorPeerReferences (peerName: string, ctx: BindPeersContext): Map<string, string> {
  const { graph } = ctx
  const references = new Map<string, string>()
  const visited = new Set<DepPath>([ctx.depPath])
  const queue: DepPath[] = [ctx.depPath]
  for (let depPath = queue.shift(); depPath != null; depPath = queue.shift()) {
    for (const dependent of graph.dependents.get(depPath) ?? []) {
      const reference = providedPeerReference(graph, dependent, peerName)
      if (reference != null) {
        references.set(dp.refToRelative(reference, peerName) ?? reference, reference)
      } else if (dependent !== IMPORTER && !visited.has(dependent)) {
        visited.add(dependent)
        queue.push(dependent)
      }
    }
  }
  return references
}

/** The reference `dependent` resolves `peerName` to, if it depends on it. */
function providedPeerReference (graph: DeployedGraph, dependent: Dependent, peerName: string): string | undefined {
  const dependencyMaps = dependent === IMPORTER
    ? [graph.importer.dependencies, graph.importer.devDependencies, graph.importer.optionalDependencies]
    : [graph.packages[dependent]?.dependencies, graph.packages[dependent]?.optionalDependencies]
  const providing = dependencyMaps.find(dependencies => dependencies != null && Object.hasOwn(dependencies, peerName))
  return providing?.[peerName]
}

/** The version part of the dependency path `reference` resolves to. */
function referenceVersion (reference: string, alias: string): string {
  const depPath = dp.refToRelative(reference, alias)
  if (depPath == null) return reference
  const { name } = dp.parse(depPath)
  return name == null ? depPath : depPath.slice(name.length + 1)
}

function collectDependents (importer: ProjectSnapshot, packages: PackageSnapshots): Map<DepPath, Dependent[]> {
  const dependents = new Map<DepPath, Dependent[]>()
  addDependents(dependents, IMPORTER, [importer.dependencies, importer.devDependencies, importer.optionalDependencies])
  for (const [depPath, snapshot] of Object.entries(packages) as Array<[DepPath, PackageSnapshot]>) {
    addDependents(dependents, depPath, [snapshot.dependencies, snapshot.optionalDependencies])
  }
  return dependents
}

function addDependents (
  dependents: Map<DepPath, Dependent[]>,
  dependent: Dependent,
  dependencyMaps: Array<ResolvedDependencies | undefined>
): void {
  for (const [alias, reference] of dependencyMaps.flatMap(dependencies => Object.entries(dependencies ?? {}))) {
    const child = dp.refToRelative(reference, alias)
    if (child == null) continue
    let dependentsOfChild = dependents.get(child)
    if (dependentsOfChild == null) dependents.set(child, dependentsOfChild = [])
    dependentsOfChild.push(dependent)
  }
}

/**
 * Keyed by the resolved dependency path rather than the reference that
 * spelled it, so an npm-aliased edge and a plain one that name the same
 * package count once.
 */
function collectReferencesByName (importer: ProjectSnapshot, packages: PackageSnapshots): Map<string, Set<string>> {
  const references = new Map<string, Set<string>>()
  addReferencesByName(references, importer.dependencies)
  addReferencesByName(references, importer.devDependencies)
  addReferencesByName(references, importer.optionalDependencies)
  for (const snapshot of Object.values(packages)) {
    addReferencesByName(references, snapshot.dependencies)
    addReferencesByName(references, snapshot.optionalDependencies)
  }
  return references
}

function addReferencesByName (references: Map<string, Set<string>>, dependencies: ResolvedDependencies | undefined): void {
  for (const [alias, reference] of Object.entries(dependencies ?? {})) {
    const depPath = dp.refToRelative(reference, alias)
    if (depPath == null) continue
    const { name } = dp.parse(depPath)
    if (name == null) continue
    let referencesOfName = references.get(name)
    if (referencesOfName == null) references.set(name, referencesOfName = new Set())
    referencesOfName.add(depPath.slice(name.length + 1))
  }
}

/**
 * Whether `source` binds `name` through one of its runtime dependency maps.
 *
 * Own keys only: a package may legitimately be named `constructor` or
 * `toString`, and a plain property read would find those on `Object.prototype`
 * and report a binding that does not exist.
 */
function declaresDependency (
  source: Pick<ProjectManifest, 'dependencies' | 'optionalDependencies'> | Pick<PackageSnapshot, 'dependencies' | 'optionalDependencies'>,
  name: string
): boolean {
  return (source.dependencies != null && Object.hasOwn(source.dependencies, name)) ||
    (source.optionalDependencies != null && Object.hasOwn(source.optionalDependencies, name))
}
