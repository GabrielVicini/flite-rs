"""Build the upstream Flite reference binaries that flite-rs is verified against.

The bit-exactness claim only means something if it is checked against a real
Flite build, so this compiles one from an upstream source tree. Two binaries
come out of it:

  reffile   text file in, WAV out. This is what the corpus test compares
            against, sample for sample.
  refdump   segments, syllables and pitch targets for one sentence, in the
            format `cargo run --example analysis` prints, for bisecting a
            divergence.

Upstream ships no build system that produces these, and its own driver does not
compile with MSVC, so the source list is assembled here instead.

Usage:

    python tools/reference/build.py --flite-src PATH/TO/flite

MSVC is used on Windows and `cc` elsewhere. Set VCVARS to a vcvars64.bat to
override the Visual Studio that gets picked, or CC to choose a compiler on
other platforms.
"""

import argparse
import os
import platform
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# Every .c file in these directories is compiled. cg is not used by the kal
# voice but several of its symbols are referenced by the shared synthesis code.
SOURCE_DIRS = [
    "src/hrg",
    "src/lexicon",
    "src/regex",
    "src/speech",
    "src/stats",
    "src/synth",
    "src/utils",
    "src/wavesynth",
    "src/cg",
    "lang/cmulex",
    "lang/usenglish",
    "lang/cmu_us_kal",
]

# The rest of src/audio is platform-specific output drivers, which the file
# and dump paths never reach.
EXTRA_SOURCES = [
    "src/audio/audio.c",
    "src/audio/au_none.c",
    "src/audio/au_streaming.c",
]

# Alternative platform implementations, files that are #included by another
# translation unit rather than compiled on their own, and the network audio
# clients. Compiling any of them is either a duplicate symbol or a syntax
# error on a modern toolchain.
EXCLUDED = {
    "cst_file_palmos.c",
    "cst_file_wince.c",
    "cst_mmap_posix.c",
    "cst_mmap_win32.c",
    "cmu_lex_data_raw.c",
    "cmu_lex_num_bytes.c",
    "cmu_lex_phones_huff_table.c",
    "cmu_lex_entries_huff_table.c",
    "auclient.c",
    "auserver.c",
}

INCLUDE_DIRS = ["include", "lang/usenglish", "lang/cmulex", "lang/cmu_us_kal"]

DRIVERS = ["reffile.c", "refdump.c"]

HERE = Path(__file__).resolve().parent


def collect_sources(root):
    """Every upstream .c file the reference binaries need."""
    sources = []
    for directory in SOURCE_DIRS:
        path = root / directory
        if not path.is_dir():
            sys.exit(f"not a Flite source tree: {path} is missing")
        sources.extend(sorted(p for p in path.glob("*.c") if p.name not in EXCLUDED))
    for extra in EXTRA_SOURCES:
        sources.append(root / extra)

    # Object files all land in one directory, so two sources sharing a base
    # name would silently overwrite each other.
    seen = {}
    for source in sources:
        if source.name in seen:
            sys.exit(f"duplicate source name: {source} and {seen[source.name]}")
        seen[source.name] = source
    return sources


def stale(source, obj):
    return not obj.exists() or obj.stat().st_mtime < source.stat().st_mtime


def write_response(path, arguments):
    path.write_text("".join(f'"{a}"\n' for a in arguments), encoding="utf-8")
    return path


def find_vcvars():
    override = os.environ.get("VCVARS")
    if override:
        return Path(override)
    program_files = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
    vswhere = Path(program_files) / "Microsoft Visual Studio/Installer/vswhere.exe"
    if not vswhere.exists():
        return None
    found = subprocess.run(
        [
            str(vswhere),
            "-latest",
            "-prerelease",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ],
        capture_output=True,
        text=True,
    )
    for line in found.stdout.splitlines():
        candidate = Path(line.strip()) / "VC/Auxiliary/Build/vcvars64.bat"
        if candidate.exists():
            return candidate
    return None


def run_in_vc_env(vcvars, commands, cwd):
    """Run commands in a shell that has had vcvars applied.

    cl.exe needs a large set of environment variables that only the batch file
    knows how to compute, and they do not survive back into this process, so
    the whole build runs inside one generated batch file.
    """
    script = cwd / "_build.bat"
    lines = ["@echo off", f'call "{vcvars}" >nul || exit /b 1']
    for command in commands:
        lines.append(command)
        lines.append("if errorlevel 1 exit /b 1")
    script.write_text("\r\n".join(lines) + "\r\n", encoding="utf-8")
    result = subprocess.run(["cmd", "/c", str(script)], cwd=cwd)
    script.unlink()
    if result.returncode != 0:
        sys.exit("reference build failed")


def build_msvc(root, sources, objdir, outdir):
    vcvars = find_vcvars()
    if vcvars is None or not vcvars.exists():
        sys.exit(
            "no Visual Studio C++ toolchain found; set VCVARS to a vcvars64.bat"
        )

    includes = " ".join(f'/I"{root / d}"' for d in INCLUDE_DIRS)
    flags = f"/nologo /O2 /W0 /MP /D_CRT_SECURE_NO_WARNINGS {includes}"
    commands = []

    # The doubled backslash is load bearing: a single one before the closing
    # quote would escape it and cl would read the rest of the line as part of
    # the path.
    into_objdir = f'/Fo"{objdir}\\\\"'

    outdated = [s for s in sources if stale(s, objdir / (s.stem + ".obj"))]
    if outdated:
        response = write_response(objdir / "sources.rsp", outdated)
        commands.append(f'cl {flags} /c @"{response}" {into_objdir}')

    library = write_response(
        objdir / "objects.rsp", [objdir / (s.stem + ".obj") for s in sources]
    )
    for driver in DRIVERS:
        exe = outdir / (Path(driver).stem + ".exe")
        commands.append(f'cl {flags} /c "{HERE / driver}" {into_objdir}')
        commands.append(
            f'cl /nologo /Fe:"{exe}" "{objdir / (Path(driver).stem + ".obj")}" '
            f'@"{library}"'
        )
    run_in_vc_env(vcvars, commands, objdir)


def build_cc(root, sources, objdir, outdir):
    compiler = os.environ.get("CC", "cc")
    includes = [f"-I{root / d}" for d in INCLUDE_DIRS]
    flags = ["-O2", "-w", *includes]

    def compile_one(source):
        obj = objdir / (source.stem + ".o")
        if not stale(source, obj):
            return obj
        subprocess.run(
            [compiler, *flags, "-c", str(source), "-o", str(obj)], check=True
        )
        return obj

    try:
        with ThreadPoolExecutor() as pool:
            objects = list(pool.map(compile_one, sources))
        for driver in DRIVERS:
            exe = outdir / Path(driver).stem
            subprocess.run(
                [
                    compiler,
                    *flags,
                    str(HERE / driver),
                    *[str(o) for o in objects],
                    "-o",
                    str(exe),
                    "-lm",
                ],
                check=True,
            )
    except subprocess.CalledProcessError:
        sys.exit("reference build failed")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--flite-src",
        default=os.environ.get("FLITE_SRC"),
        help="an unpacked upstream Flite source tree (or set FLITE_SRC)",
    )
    parser.add_argument(
        "--out",
        default=HERE / "build",
        help="where to put the binaries (default tools/reference/build)",
    )
    args = parser.parse_args()

    if not args.flite_src:
        parser.error("--flite-src is required (or set FLITE_SRC)")

    root = Path(args.flite_src).resolve()
    outdir = Path(args.out).resolve()
    objdir = outdir / "obj"
    objdir.mkdir(parents=True, exist_ok=True)

    sources = collect_sources(root)
    print(f"building {len(sources)} upstream sources from {root}")

    if platform.system() == "Windows":
        build_msvc(root, sources, objdir, outdir)
    else:
        build_cc(root, sources, objdir, outdir)

    for driver in DRIVERS:
        name = Path(driver).stem
        exe = outdir / (name + ".exe" if platform.system() == "Windows" else name)
        if not exe.exists():
            sys.exit(f"expected {exe} to have been built")
        print(f"built {exe}")


if __name__ == "__main__":
    main()
