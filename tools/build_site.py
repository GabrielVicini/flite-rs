#!/usr/bin/env python3
"""Build the demo site.

Compiles the engine to WebAssembly once per voice set, copies the page next to
the modules, and writes the manifest the page reads to learn what this build
actually contains — version, commit, module sizes. Nothing about the site is
hand-maintained: the page asks the manifest which engines exist and hides the
ones a build left out.

    python tools/build_site.py --out _site
    python tools/build_site.py --out _site --skip-build   # reuse target/

The output is a directory of static files. There is no bundler, no npm and no
wasm-bindgen anywhere in here on purpose: the page loads the `.wasm` file
itself and calls into it directly.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WASM_CRATE = ROOT / "wasm"
WEB = ROOT / "web"
TARGET = "wasm32-unknown-unknown"

# One module per voice set. A visitor downloads only the one whose engine they
# pick, which is why the smallest is also the one the page starts on.
MODULES = {
    "core": [],
    "kal16": ["kal16"],
    "clustergen": ["clustergen"],
}


def run(command: list[str], **kwargs) -> subprocess.CompletedProcess:
    print("$", " ".join(command), flush=True)
    return subprocess.run(command, check=True, **kwargs)


def capture(command: list[str]) -> str:
    try:
        out = subprocess.run(command, check=True, capture_output=True, text=True)
        return out.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return ""


def crate_version() -> str:
    manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', manifest, re.MULTILINE)
    return match.group(1) if match else ""


def build_module(name: str, features: list[str], skip: bool) -> Path:
    """Build one wasm module and return the path to it."""
    artefact = WASM_CRATE / "target" / TARGET / "release" / "flite_rs_wasm.wasm"
    staged = WASM_CRATE / "target" / f"{name}.wasm"

    if skip and staged.exists():
        return staged

    command = ["cargo", "build", "--release", "--target", TARGET]
    if features:
        command += ["--features", ",".join(features)]
    run(command, cwd=WASM_CRATE)

    # Each build overwrites the same artefact, so it is stashed under its own
    # name before the next one runs.
    staged.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(artefact, staged)
    return staged


def optimise(path: Path) -> Path:
    """Run wasm-opt if it happens to be installed; skip it if not.

    Measured at about 1% of each module, because almost all of a module is
    voice data and the optimiser only touches code. Worth having, not worth
    requiring, so a build without it carries on and says so.

    `--all-features` rather than a list: rustc decides which wasm proposals it
    emits and that set moves with the compiler, so naming them here means the
    optimiser starts refusing the module the next time rustc picks up
    something new. It did exactly that over nontrapping-float-to-int.
    """
    if shutil.which("wasm-opt") is None:
        print(f"  wasm-opt not found, shipping {path.name} unoptimised")
        return path
    out = path.with_suffix(".opt.wasm")
    try:
        run(["wasm-opt", "-O2", "--all-features", str(path), "-o", str(out)])
    except subprocess.CalledProcessError:
        print(f"  wasm-opt failed on {path.name}; shipping the plain build")
        return path
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default="_site", help="where to write the site")
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="reuse modules already staged under wasm/target",
    )
    parser.add_argument(
        "--only",
        action="append",
        choices=sorted(MODULES),
        help="build just these modules (repeatable); the page hides the rest",
    )
    parser.add_argument(
        "--cname",
        help="write a CNAME for a custom domain; omit it to publish on github.io",
    )
    args = parser.parse_args()

    wanted = {name: MODULES[name] for name in (args.only or MODULES)}
    out = Path(args.out).resolve()
    if out.exists():
        shutil.rmtree(out)
    (out / "wasm").mkdir(parents=True)

    manifest_modules: dict[str, dict] = {}
    for name, features in wanted.items():
        print(f"\n== {name} ==", flush=True)
        built = optimise(build_module(name, features, args.skip_build))
        data = built.read_bytes()

        # Content-hashed, so a cached module is never a stale one and the files
        # can be served with a long max-age.
        digest = hashlib.sha256(data).hexdigest()[:10]
        filename = f"{name}-{digest}.wasm"
        (out / "wasm" / filename).write_bytes(data)

        manifest_modules[name] = {
            "file": f"wasm/{filename}",
            "bytes": len(data),
            "sha256": digest,
            "features": features,
        }
        print(f"  {filename}  {len(data) / 1024 / 1024:.2f} MB")

    for item in WEB.iterdir():
        target = out / item.name
        if item.is_dir():
            shutil.copytree(item, target)
        else:
            shutil.copy2(item, target)

    manifest = {
        "version": crate_version(),
        "commit": capture(["git", "rev-parse", "HEAD"]),
        "branch": capture(["git", "rev-parse", "--abbrev-ref", "HEAD"]),
        "built": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "rustc": capture(["rustc", "--version"]).replace("rustc ", "").split(" ")[0],
        "modules": manifest_modules,
    }
    (out / "build.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    # GitHub Pages runs Jekyll otherwise, which quietly drops files and
    # directories whose names begin with an underscore.
    (out / ".nojekyll").write_text("", encoding="utf-8")

    # A custom domain is a deployment decision, not a property of the site, so
    # it is passed in rather than committed: a CNAME for a domain whose DNS is
    # not pointed here takes the site off the air at both addresses.
    if args.cname:
        (out / "CNAME").write_text(args.cname.strip() + "\n", encoding="utf-8")

    total = sum(m["bytes"] for m in manifest_modules.values())
    print(f"\nwrote {out} — {len(manifest_modules)} modules, {total / 1024 / 1024:.1f} MB total")
    return 0


if __name__ == "__main__":
    sys.exit(main())
