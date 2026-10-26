"""Run a build and bind its observed inputs to the resulting executable."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.DEVNULL)


def source_state(path):
    root = Path(git(path, "rev-parse", "--show-toplevel").decode().strip()).resolve()
    names = git(root, "ls-files", "--cached", "--others", "--exclude-standard", "-z")
    gitlinks = {}
    for entry in git(root, "ls-files", "--stage", "-z").split(b"\0"):
        if entry.startswith(b"160000 "):
            metadata, name = entry.split(b"\t", 1)
            gitlinks[os.fsdecode(name)] = metadata.split()[1].decode()
    submodules = {}
    files = {}
    for name in sorted(set(os.fsdecode(name) for name in names.split(b"\0") if name)):
        entry = root / name
        if name in gitlinks:
            state = source_state(entry) if (entry / ".git").exists() else None
            submodules[name] = {"expected_commit": gitlinks[name], "source": state}
            files[name] = {"gitlink": gitlinks[name], "source_tree_sha256": state["tree_sha256"] if state else None}
            continue
        try:
            mode = entry.lstat().st_mode
        except FileNotFoundError:
            files[name] = None
            continue
        if stat.S_ISLNK(mode):
            files[name] = {"symlink": os.readlink(entry)}
        elif stat.S_ISREG(mode):
            files[name] = {"sha256": digest(entry), "executable": bool(mode & 0o111)}
        else:
            raise ValueError(f"Source {name!r} is not a regular file, symbolic link, or Git submodule.")
    try:
        commit = git(root, "rev-parse", "--verify", "HEAD").decode().strip()
    except subprocess.CalledProcessError:
        commit = None
    return {
        "root": str(root),
        "commit": commit,
        "dirty": bool(git(root, "status", "--porcelain=v1", "--untracked-files=all")),
        "tree_sha256": hashlib.sha256(canonical(files)).hexdigest(),
        "tree_hash": "sha256_of_sorted_file_hashes_v1",
        "submodules": submodules,
    }


def compiler_state(program):
    path = Path(shutil.which(program) or program).resolve(strict=True)
    version = subprocess.check_output([str(path), "--version"], stderr=subprocess.STDOUT).decode().strip()
    return {"program": str(path), "sha256": digest(path), "version": version}


def now():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--source", action="append", required=True, metavar="NAME=GIT_ROOT")
    parser.add_argument("--compiler", default="rustc")
    parser.add_argument("--profile", required=True)
    parser.add_argument("--features", action="append", default=[])
    parser.add_argument("--lockfile", action="append", default=[], type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("Supply the build command after --.")
    sources = {}
    for source in args.source:
        name, separator, path = source.partition("=")
        if not separator or not name or name in sources:
            parser.error("Sources require unique NAME=GIT_ROOT entries.")
        sources[name] = Path(path)
    started = now()
    before = {name: source_state(path) for name, path in sources.items()}
    locks = {str(path): digest(path) for path in args.lockfile}
    compiler = compiler_state(args.compiler)
    flag_names = {
        "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER", "CC", "CXX", "CFLAGS", "CXXFLAGS",
        "CPPFLAGS", "LDFLAGS", "CARGO_BUILD_TARGET",
    }
    flag_names.update(name for name in os.environ if name.startswith(("CARGO_PROFILE_", "CARGO_TARGET_")))
    flags = {name: os.environ.get(name) for name in sorted(flag_names)}
    status = subprocess.run(command, stdout=sys.stderr, check=False).returncode
    if status:
        return status if status > 0 else 1
    after = {name: source_state(path) for name, path in sources.items()}
    if before != after or locks != {str(path): digest(path) for path in args.lockfile} or compiler != compiler_state(args.compiler):
        raise ValueError("Build inputs changed during the build; no receipt was written.")
    evidence = {
        "schema_version": 1,
        "capture": "build_wrapper",
        "executable_sha256": digest(args.executable),
        "build": {
            "started_at": started,
            "finished_at": now(),
            "command": command,
            "working_directory": str(Path.cwd()),
            "sources": before,
            "compiler": compiler,
            "profile": args.profile,
            "features": sorted(set(args.features)),
            "settings_origin": "wrapper_arguments",
            "flags": flags,
            "lockfiles": locks,
        },
    }
    destination = args.executable.with_name(args.executable.name + ".metewand-build.json")
    # A reader can see the previous complete receipt or this complete receipt.
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(canonical(evidence) + b"\n")
    try:
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    print(destination)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"build evidence: {error}", file=sys.stderr)
        sys.exit(1)
