# DataForge plugin fixtures (ABI 0.1)

These are reproducible WebAssembly Component Model text fixtures used by the
`df-plugin` conformance tests. They are intentionally hand-authored WAT because
this repository does not currently install a Wasm guest Rust target. There is
no claimed guest SDK.

ABI `0.1.0` is defined by `crates/df-plugin/wit/plugin.wit`: two exports,
`describe() -> string` and `analyze(string) -> string`. The host linker is empty
and does not link WASI. ABI 0.1 therefore has no filesystem, network,
environment, clock or random capability. Its only explicit capabilities are
bounded metadata and normalized text copied into the JSON input. Adding an
ambient capability requires a new ABI and security review.

Compatibility is checked twice: `abi_version` is an exact semantic version,
while `host_compatibility` is a semantic-version range that must include the
host ABI. Registry identity is `(plugin_id, plugin_version)` and immutable.

The test suite signs each fixture with an ephemeral Ed25519 key. Altered hash,
signature and incompatible-ABI cases are generated from the same deterministic
fixture so no private test key or misleading pre-signed artifact is committed.

