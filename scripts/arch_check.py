#!/usr/bin/env python3
"""Architecture limits check (docs/ARCHITECTURE.md).

Thresholds
----------
file    <= 350 lines  OK
        351..450      WARNING  (time to split)
        > 450         ERROR
folder  <= 4 .rs files OK
        5             WARNING
        >= 6          ERROR

Subfolders do not count against their parent's file budget, and a `tests.rs`
sibling is not counted either (docs/ARCHITECTURE.md, "Tests").

Grandfathering
--------------
The repository already contains oversized files/folders. They are listed in
`docs/arch_baseline.txt`; a grandfathered entry is reported as a warning but
**must not grow** — growing it is an error, and a file/folder that is not in
the baseline is judged by the absolute thresholds above. Regenerate the
baseline only after an intentional split, with `--update-baseline`.

Usage
-----
    python3 scripts/arch_check.py                 # check
    python3 scripts/arch_check.py --strict        # warnings are errors (CI)
    python3 scripts/arch_check.py --update-baseline
    python3 scripts/arch_check.py --selftest
"""

from __future__ import annotations

import argparse
import os
import sys
import tempfile

FILE_OK = 350
FILE_WARN_MAX = 450
DIR_OK = 4
DIR_WARN_MAX = 5

BASELINE = os.path.join("docs", "arch_baseline.txt")
SKIP_DIRS = {"target", ".git", "node_modules", "__pycache__", "venv"}


def repo_root() -> str:
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def file_lines(path: str) -> int:
    with open(path, "rb") as fh:
        data = fh.read()
    if not data:
        return 0
    return data.count(b"\n") + (0 if data.endswith(b"\n") else 1)


def scan(root: str) -> tuple[dict[str, int], dict[str, int]]:
    """Return ({relative .rs file -> lines}, {relative folder -> counted .rs files})."""
    files: dict[str, int] = {}
    dirs: dict[str, int] = {}
    for base, subdirs, names in os.walk(root):
        subdirs[:] = sorted(d for d in subdirs if d not in SKIP_DIRS)
        for name in sorted(names):
            if not name.endswith(".rs"):
                continue
            abs_path = os.path.join(base, name)
            rel = os.path.relpath(abs_path, root).replace(os.sep, "/")
            files[rel] = file_lines(abs_path)
            # `tests.rs` is a test sibling: it does not consume the folder budget.
            if name == "tests.rs":
                continue
            rel_dir = os.path.relpath(base, root).replace(os.sep, "/")
            dirs[rel_dir] = dirs.get(rel_dir, 0) + 1
    return files, dirs


def load_baseline(root: str) -> tuple[dict[str, int], dict[str, int]]:
    files: dict[str, int] = {}
    dirs: dict[str, int] = {}
    path = os.path.join(root, BASELINE)
    if not os.path.exists(path):
        return files, dirs
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split()
            if len(parts) != 3:
                continue
            kind, rel, value = parts
            try:
                n = int(value)
            except ValueError:
                continue
            if kind == "file":
                files[rel] = n
            elif kind == "dir":
                dirs[rel] = n
    return files, dirs


def baseline_text(files: dict[str, int], dirs: dict[str, int]) -> str:
    out = [
        "# Grandfathered architecture-limit offenders (docs/ARCHITECTURE.md).",
        "# A listed entry may not GROW; regenerate after an intentional split with",
        "#   python3 scripts/arch_check.py --update-baseline",
        "",
        "# file <path> <lines>",
    ]
    for rel in sorted(files):
        out.append("file %s %d" % (rel, files[rel]))
    out.append("")
    out.append("# dir <path> <counted .rs files>")
    for rel in sorted(dirs):
        out.append("dir %s %d" % (rel, dirs[rel]))
    out.append("")
    return "\n".join(out)


def evaluate(
    files: dict[str, int],
    dirs: dict[str, int],
    base_files: dict[str, int],
    base_dirs: dict[str, int],
) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    warnings: list[str] = []
    for rel in sorted(files):
        n = files[rel]
        base = base_files.get(rel)
        if base is not None:
            if n > base:
                errors.append(
                    "%s: grew to %d lines (grandfathered ceiling %d) — split it instead"
                    % (rel, n, base)
                )
            elif n > FILE_WARN_MAX:
                warnings.append("%s: %d lines (grandfathered; cap %d)" % (rel, n, FILE_OK))
            elif n > FILE_OK:
                warnings.append("%s: %d lines (cap %d)" % (rel, n, FILE_OK))
            continue
        if n > FILE_WARN_MAX:
            errors.append("%s: %d lines (hard cap %d)" % (rel, n, FILE_WARN_MAX))
        elif n > FILE_OK:
            warnings.append("%s: %d lines (split suggested above %d)" % (rel, n, FILE_OK))
    for rel in sorted(dirs):
        n = dirs[rel]
        label = rel if rel != "." else "<crate root>"
        base = base_dirs.get(rel)
        if base is not None:
            if n > base:
                errors.append(
                    "%s/: %d .rs files (grandfathered ceiling %d) — split the folder instead"
                    % (label, n, base)
                )
            elif n > DIR_OK:
                warnings.append("%s/: %d .rs files (grandfathered; cap %d)" % (label, n, DIR_OK))
            continue
        if n > DIR_WARN_MAX:
            errors.append("%s/: %d .rs files (hard cap %d)" % (label, n, DIR_WARN_MAX))
        elif n > DIR_OK:
            warnings.append("%s/: %d .rs files (split suggested above %d)" % (label, n, DIR_OK))
    return errors, warnings


def offenders(files: dict[str, int], dirs: dict[str, int]) -> tuple[dict[str, int], dict[str, int]]:
    return (
        {rel: n for rel, n in files.items() if n > FILE_OK},
        {rel: n for rel, n in dirs.items() if n > DIR_OK},
    )


def run(root: str, strict: bool) -> int:
    files, dirs = scan(root)
    base_files, base_dirs = load_baseline(root)
    errors, warnings = evaluate(files, dirs, base_files, base_dirs)
    for w in warnings:
        print("arch: WARN  " + w)
    for e in errors:
        print("arch: ERROR " + e, file=sys.stderr)
    print(
        "arch: %d files, %d folders — %d warning(s), %d error(s)"
        % (len(files), len(dirs), len(warnings), len(errors))
    )
    if errors or (strict and warnings):
        return 1
    return 0


def selftest() -> int:
    """Exercise the thresholds on a throwaway tree."""
    with tempfile.TemporaryDirectory() as tmp:
        root = os.path.join(tmp, "repo")
        os.makedirs(os.path.join(root, "a", "deep"))
        os.makedirs(os.path.join(root, "b"))
        os.makedirs(os.path.join(root, "docs"))

        def put(rel: str, n: int) -> None:
            with open(os.path.join(root, rel), "w", encoding="utf-8") as fh:
                fh.write("\n".join(["// x"] * n) + "\n")

        put("a/ok.rs", 350)
        put("a/warn.rs", 400)
        put("a/deep/big.rs", 451)          # ERROR: not in the baseline
        for i in range(5):                  # 5 files -> WARNING
            put("b/f%d.rs" % i, 10)
        with open(os.path.join(root, BASELINE), "w", encoding="utf-8") as fh:
            fh.write("file a/warn.rs 400\ndir b 5\n")
        files, dirs = scan(root)
        base_files, base_dirs = load_baseline(root)
        errors, warnings = evaluate(files, dirs, base_files, base_dirs)
        checks = [
            ("351..450 warns", any("a/warn.rs" in w for w in warnings)),
            ("grandfathered 5-file folder only warns", not any("b/" in e for e in errors)),
            ("451-line un-baselined file errors", any("a/deep/big.rs" in e for e in errors)),
            ("350-line file is silent",
             not any("a/ok.rs" in w for w in warnings)
             and not any("a/ok.rs" in e for e in errors)),
        ]
        # Growth beyond a baseline must error.
        with open(os.path.join(root, "a", "warn.rs"), "w", encoding="utf-8") as fh:
            fh.write("\n".join(["// x"] * 401) + "\n")
        files, _ = scan(root)
        errors2, _ = evaluate(files, dirs, base_files, base_dirs)
        checks.append(("growth over baseline errors", any("a/warn.rs" in e for e in errors2)))
        failed = [name for name, ok in checks if not ok]
        for name, ok in checks:
            print("%s %s" % ("ok  " if ok else "FAIL", name))
        return 1 if failed else 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Architecture limits check")
    ap.add_argument("--strict", action="store_true", help="treat warnings as errors")
    ap.add_argument("--update-baseline", action="store_true", help="rewrite docs/arch_baseline.txt")
    ap.add_argument("--selftest", action="store_true", help="check the checker on a temp tree")
    args = ap.parse_args(argv)
    if args.selftest:
        return selftest()
    root = repo_root()
    if args.update_baseline:
        files, dirs = scan(root)
        bf, bd = offenders(files, dirs)
        with open(os.path.join(root, BASELINE), "w", encoding="utf-8") as fh:
            fh.write(baseline_text(bf, bd))
        print("arch: baseline updated (%d files, %d folders)" % (len(bf), len(bd)))
        return 0
    return run(root, args.strict)


if __name__ == "__main__":
    sys.exit(main())
