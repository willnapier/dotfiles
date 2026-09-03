# forge-names

`forge-names` is the private canonical path-to-name boundary used by William's
Rust tools. Paths remain opaque I/O identities; strings derived from paths are
normalised at one explicit boundary, and lookups return the path observed in a
directory listing.

Separate product repositories do not depend on this dotfiles checkout. Run
`tools/vendor_forge_names.py --product /absolute/product/worktree` from a clean,
committed dotfiles revision. It creates a package-derived snapshot at
`vendor/forge-names` and installs the portable verifier at
`scripts/verify_forge_names.py`. Product manifests use only that in-tree path.
The verifier combines an offline, lock-respecting Cargo metadata pass with a
symlink-resolving audit of every product manifest, so registry dependencies do
not need to be downloaded merely to prove path containment.

The expected outputs in `tests/contract_vectors.rs` are a frozen behavioural
contract. They must run in every consuming product under its own lockfile. Do
not change an expected output until a separate schema decision has versioned
all persisted normalised-name representations.
