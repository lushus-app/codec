#!/usr/bin/env python3
"""Assert the repository invariants that are cheaper to check textually.

See CLAUDE.md, "Non-negotiable invariants". Run it from the repository root:

    python3 .github/scripts/check_invariants.py

Exits non-zero and prints a GitHub Actions error annotation for each violation.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

CRATES = Path("crates")

FORBID = "#![forbid(unsafe_code)]"

# Wrappers around a C/C++ H.264 implementation. cargo-deny bans the ones we know
# by name across the whole resolved graph; this catches a new one by pattern at
# the point a crate declares it. Only dependency *names* are inspected, so prose
# in a doc comment may say "ffmpeg" freely -- the PRD and CLAUDE.md both do.
C_CODEC = re.compile(r"ffmpeg|libav|avcodec|avformat|openh264|x264|x265|dav1d", re.I)


def crate_roots() -> list[tuple[Path, Path | None]]:
    """Every crate manifest paired with its crate root, if one exists."""
    found = []
    for manifest in sorted(CRATES.glob("*/Cargo.toml")):
        src = manifest.parent / "src"
        root = next((src / n for n in ("lib.rs", "main.rs") if (src / n).is_file()), None)
        found.append((manifest, root))
    return found


def check_forbids_unsafe(errors: list[str]) -> None:
    """Invariant 2: every crate root forbids unsafe code."""
    for manifest, root in crate_roots():
        if root is None:
            errors.append(f"::error file={manifest}::crate has no src/lib.rs or src/main.rs")
        elif FORBID not in root.read_text(encoding="utf-8"):
            errors.append(f"::error file={root}::missing {FORBID} (CLAUDE.md invariant 2)")


def check_no_c_codec_dependency(errors: list[str]) -> None:
    """Invariant 1: nothing in crates/ depends on a C codec."""
    for manifest, _ in crate_roots():
        section = ""
        for lineno, line in enumerate(manifest.read_text(encoding="utf-8").splitlines(), 1):
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            if stripped.startswith("["):
                section = stripped.strip("[]")
                continue
            if "dependencies" not in section or "=" not in stripped:
                continue
            name = stripped.split("=", 1)[0].strip().strip('"')
            if C_CODEC.search(name):
                errors.append(
                    f"::error file={manifest},line={lineno}::dependency {name!r} wraps a "
                    f"C codec (CLAUDE.md invariant 1); it belongs in xtask/, tests/ or fuzz/"
                )


def main() -> int:
    if not CRATES.is_dir():
        print(f"::error::{CRATES}/ not found; run from the repository root")
        return 1

    errors: list[str] = []
    check_forbids_unsafe(errors)
    check_no_c_codec_dependency(errors)

    for error in errors:
        print(error)
    if errors:
        print(f"\n{len(errors)} invariant violation(s)", file=sys.stderr)
        return 1

    checked = len(crate_roots())
    print(f"invariants ok ({checked} crate{'s' if checked != 1 else ''} checked)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
