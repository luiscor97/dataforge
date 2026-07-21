#!/usr/bin/env python3
"""Generate a deterministic CycloneDX 1.5 SBOM from `cargo metadata`.

The output is reproducible: components are sorted and no wall-clock
timestamp is embedded, so re-running against the same `Cargo.lock` produces
byte-identical JSON. Run from the repository root:

    python scripts/generate-sbom.py > docs/sbom/dataforge.cdx.json

Requires only `cargo` and Python 3 — no extra cargo subcommand.
"""

import json
import subprocess
import sys


def cargo_metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    )
    return json.loads(out.stdout)


def component(pkg: dict) -> dict:
    source = pkg.get("source") or ""
    is_registry = source.startswith("registry+")
    comp = {
        "type": "library",
        "name": pkg["name"],
        "version": pkg["version"],
        "scope": "required",
    }
    if pkg.get("description"):
        comp["description"] = pkg["description"].strip()
    if pkg.get("license"):
        # A SPDX expression; CycloneDX accepts it as a license expression.
        comp["licenses"] = [{"expression": pkg["license"]}]
    if is_registry:
        comp["purl"] = f"pkg:cargo/{pkg['name']}@{pkg['version']}"
        comp["externalReferences"] = [
            {"type": "distribution", "url": "https://crates.io/"}
        ]
    else:
        # Workspace-local crate; mark it as first-party.
        comp["properties"] = [{"name": "dataforge:origin", "value": "workspace"}]
    return comp


def main() -> int:
    # Emit UTF-8 regardless of the platform's console/locale encoding, so a
    # dependency description with non-ASCII characters never breaks the run.
    try:
        sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    except (AttributeError, ValueError):
        pass
    meta = cargo_metadata()
    members = set(meta.get("workspace_members", []))
    packages = meta["packages"]

    components = [
        component(pkg)
        for pkg in packages
        # The virtual workspace root has no real package identity to list as
        # a dependency of itself; every real crate (ours + external) is a
        # component.
        if True
    ]
    components.sort(key=lambda c: (c["name"], c["version"]))

    workspace_names = sorted(
        pkg["name"] for pkg in packages if pkg["id"] in members
    )

    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "dataforge",
                "version": next(
                    (p["version"] for p in packages if p["name"] == "df-facade"),
                    "0.0.0",
                ),
                "description": "Local-first, verifiable document reconstruction engine",
            },
            "tools": [
                {"vendor": "DataForge", "name": "generate-sbom.py", "version": "1"}
            ],
            "properties": [
                {
                    "name": "dataforge:workspace-members",
                    "value": ", ".join(workspace_names),
                },
                {
                    "name": "dataforge:component-count",
                    "value": str(len(components)),
                },
            ],
        },
        "components": components,
    }
    json.dump(bom, sys.stdout, indent=2, ensure_ascii=False, sort_keys=False)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
