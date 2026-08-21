#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Fail if the plugin manifests disagree with the crate version.

weir ships as two things that have to move together: a plugin (hook manifests,
delivered through the marketplaces) and a binary (installed separately). Users
update them independently, so the versions drifting apart is the normal failure,
not the exotic one. Cargo.toml is the single source of truth; this check is what
stops a release going out with manifests still on the previous number.

Usage: version-check.py [--set X.Y.Z]
"""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CARGO = ROOT / "Cargo.toml"
MANIFESTS = [ROOT / ".claude-plugin" / "plugin.json", ROOT / ".codex-plugin" / "plugin.json"]


def crate_version() -> str:
    m = re.search(r'^\[workspace\.package\](?:.|\n)*?^version = "([^"]+)"', CARGO.read_text(), re.M)
    if not m:
        sys.exit("Cargo.toml: no [workspace.package] version")
    return m.group(1)


def main() -> int:
    if "--set" in sys.argv:
        new = sys.argv[sys.argv.index("--set") + 1]
        if not re.fullmatch(r"\d+\.\d+\.\d+", new):
            sys.exit(f"not a semver: {new}")
        text = CARGO.read_text()
        text = re.sub(
            r'(^\[workspace\.package\](?:.|\n)*?^version = ")[^"]+(")',
            rf"\g<1>{new}\g<2>",
            text,
            count=1,
            flags=re.M,
        )
        CARGO.write_text(text)
        for p in MANIFESTS:
            d = json.loads(p.read_text())
            d["version"] = new
            p.write_text(json.dumps(d, indent=2) + "\n")
        print(f"set {new} in Cargo.toml and {len(MANIFESTS)} manifests")
        return 0

    want = crate_version()
    bad = []
    for p in MANIFESTS:
        got = json.loads(p.read_text()).get("version")
        mark = "ok" if got == want else "MISMATCH"
        print(f"  {p.relative_to(ROOT)}: {got}  [{mark}]")
        if got != want:
            bad.append(p.relative_to(ROOT))
    print(f"  Cargo.toml: {want}")
    if bad:
        print(f"\nRun: scripts/version-check.py --set {want}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
