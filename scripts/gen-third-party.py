#!/usr/bin/env python3
"""Write THIRD_PARTY.md: the licence notices Petrel has to carry.

MIT, BSD and Apache all say the same thing about binaries — ship the notice
with the software — so a release that includes none of them is distributing
several hundred libraries against their terms. This walks the dependency
graph, finds each crate's own licence file in the cargo registry, and writes
one document with every distinct notice and the crates it covers.

The interface is software shipped too: its JavaScript is bundled into the
app, and the npm packages in it carry the same kind of terms. They were
missing — the document covered the Rust half of Petrel only. Their notices
come from the installed tree, each package's own licence file, the same way.

    python3 scripts/gen-third-party.py

Crates that ship no licence file are listed with their SPDX expression and no
text, which is the most that can honestly be said about them. Run it again
whenever dependencies change; CI checks nothing here, a human reads the diff.
"""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY.md"
UI = ROOT / "apps" / "desktop" / "ui"
# The typefaces the interface bundles keep their notice beside them, in the
# file the app carries with the fonts; this document carries a copy.
FONTS = UI / "public" / "fonts" / "OFL.md"

# The licence file the crate ships, under any of the names people use.
NAMES = re.compile(
    r"^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)([-._].*)?$",
    re.IGNORECASE,
)


def crates(target: str) -> list[dict]:
    """Every third-party package that is compiled into a build for `target`."""
    out = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            target,
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    meta = json.loads(out.stdout)
    # By package id. Cargo used to write a member as "name version (source)",
    # and the first word was its name; it now writes "path+file://…#name@1.0",
    # which matched no name, and Petrel's own crates were listed as third-party.
    ours = set(meta["workspace_members"])
    return [p for p in meta["packages"] if p["id"] not in ours]


def js_packages() -> list[dict]:
    """Every npm package the interface's production build can bundle.

    Its production dependencies, direct and transitive, as installed: more
    than the bundler keeps, which is the safe side for notices. A failure
    stops the script rather than writing a document without them.
    """
    out = subprocess.run(
        ["pnpm", "--dir", str(UI), "licenses", "list", "--prod", "--json"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    found = []
    for spdx, pkgs in json.loads(out.stdout).items():
        for p in pkgs:
            for version, path in zip(p["versions"], p["paths"]):
                found.append(
                    {
                        "name": p["name"],
                        "version": version,
                        "license": p.get("license") or spdx,
                        "dir": path,
                    }
                )
    return sorted(found, key=lambda p: (p["name"], p["version"]))


def notices(pkg: dict) -> list[str]:
    """The text of every licence file the crate ships, longest first."""
    return notices_in(pathlib.Path(pkg["manifest_path"]).parent)


def notices_in(d: pathlib.Path) -> list[str]:
    """The text of every licence file in a package's directory."""
    found = []
    for f in sorted(d.iterdir()) if d.is_dir() else []:
        if f.is_file() and NAMES.match(f.name):
            try:
                text = f.read_text(encoding="utf-8", errors="replace").strip()
            except OSError:
                continue
            if text:
                found.append(text)
    return found


def main() -> int:
    # Both Macs, so a crate that only compiles for one architecture is still
    # covered; Windows and Linux packages are built from the same tree.
    targets = [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
    ]
    seen: dict[tuple[str, str], dict] = {}
    for t in targets:
        try:
            for p in crates(t):
                seen[(p["name"], p["version"])] = p
        except subprocess.CalledProcessError as e:
            print(f"skipping {t}: {e.stderr.strip()[:200]}", file=sys.stderr)

    packages = [seen[k] for k in sorted(seen)]
    js = js_packages()

    # One section per distinct notice, listing the crates that ship it. Many
    # hundreds of crates carry byte-identical Apache-2.0 text; printing it
    # once keeps the document readable.
    by_text: dict[str, list[str]] = {}
    no_text: list[str] = []
    for p in packages:
        label = f"{p['name']} {p['version']} ({p.get('license') or 'no SPDX expression'})"
        found = notices(p)
        if not found:
            no_text.append(label)
            continue
        for text in found:
            by_text.setdefault(text, []).append(label)
    for p in js:
        label = f"{p['name']} {p['version']} ({p['license'] or 'no SPDX expression'}, npm)"
        found = notices_in(pathlib.Path(p["dir"]))
        if not found:
            no_text.append(label)
            continue
        for text in found:
            by_text.setdefault(text, []).append(label)

    lines = [
        "# Third-party notices",
        "",
        "Petrel is MIT licensed (see LICENSE). It is built with the libraries",
        "below, whose licences ask that their notices travel with the software.",
        "This file is generated by `scripts/gen-third-party.py`; edit that, not this.",
        "",
        f"{len(packages)} Rust packages, over the four platforms Petrel is built for,",
        f"and {len(js)} npm packages in its interface, marked npm, and the typefaces",
        "the interface bundles, whose notices are at the end.",
        "",
    ]
    for i, (text, users) in enumerate(
        sorted(by_text.items(), key=lambda kv: (-len(kv[1]), kv[1][0])), start=1
    ):
        lines.append(f"## {i}. Covering {len(users)} package(s)")
        lines.append("")
        for u in sorted(set(users)):
            lines.append(f"- {u}")
        lines.append("")
        lines.append("```")
        lines.append(text)
        lines.append("```")
        lines.append("")
    if no_text:
        lines.append("## Packages that ship no licence file")
        lines.append("")
        lines.append("Their terms are the SPDX expression in their manifest:")
        lines.append("")
        for u in sorted(set(no_text)):
            lines.append(f"- {u}")
        lines.append("")

    lines.append("## Typefaces in the interface")
    lines.append("")
    for line in FONTS.read_text(encoding="utf-8").splitlines()[1:]:
        lines.append("#" + line if line.startswith("## ") else line)
    lines.append("")

    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"{OUT}: {len(packages)} Rust and {len(js)} npm packages, {len(by_text)} distinct notices")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
