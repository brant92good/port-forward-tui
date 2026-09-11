# Dependency notices

Ports itself is [MIT licensed](../../LICENSE). Compiled 0.9+ bundles also include
`LICENSE.txt` and `THIRD_PARTY_NOTICES.txt` beside the executable. The installer
checks their hashes before replacing installed files.

[The combined notices](THIRD_PARTY_NOTICES.txt) cover 166 locked runtime and build
dependency distributions across the five release targets. Development-only
dependencies are excluded. [The inventory](dependencies.json) records versions,
source downloads and notice hashes. These documents do not change upstream terms.

`scripts/collect_licenses.py` reads the Cargo dependency graph for each target.
It fails when an included crate has no notice file, except for the five explicit
version/source-bound records in [upstream/sources.json](upstream/sources.json).
Those distributions omit the text: the record supplies upstream provenance and
the document digest, so updates require review. In particular, the objc2 project's
MIT text was added after some included crate releases; both its original license
declaration and that later document are identified separately.

The `selectors` 0.38.0 dependency is used without source modifications under
MPL-2.0. Its complete covered source is available from the
[versioned crate download](https://crates.io/api/v1/crates/selectors/0.38.0/download)
and [pinned upstream directory](https://github.com/servo/stylo/tree/572ecba2d1600e7c3d490586692a209faf703baa/selectors).
The combined notices include the [MPL text](https://www.mozilla.org/en-US/MPL/2.0/).

To update, run `python scripts/collect_licenses.py`, review the inventory and
upstream exceptions, then run `python scripts/collect_licenses.py --check`.
Collection reads cached crate files and metadata; it does not execute build
scripts. CI also tests legacy install compatibility and rejection of missing,
partial, unexpected or tampered notice bundles in temporary directories.
