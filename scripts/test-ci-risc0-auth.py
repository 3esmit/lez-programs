#!/usr/bin/env python3
"""Verify RISC Zero installation uses the least-privilege workflow token."""

from pathlib import Path
import re
import sys


action = Path(".github/actions/install-risc0/action.yml").read_text(encoding="utf-8")
workflow = Path(".github/workflows/ci.yml").read_text(encoding="utf-8")

checks = {
    "composite installer authenticates rzup through the environment": re.search(
        r"""
        -\s+name:\s+Install\s+risc0\s*
        env:\s*
        GITHUB_TOKEN:\s+\$\{\{\s*github\.token\s*\}\}\s*
        run:
        """,
        action,
        flags=re.VERBOSE,
    )
    is not None,
    "CI token is limited to repository read access": re.search(
        r"(?m)^permissions:\n  contents: read$",
        workflow,
    )
    is not None,
    "installer still invokes rzup": "/home/runner/.risc0/bin/rzup install" in action,
}

failed = [description for description, passed in checks.items() if not passed]
if failed:
    for description in failed:
        print(f"FAIL: {description}", file=sys.stderr)
    raise SystemExit(1)

print("RISC Zero CI authentication contract valid")
