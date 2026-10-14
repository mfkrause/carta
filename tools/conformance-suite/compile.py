#!/usr/bin/env python3
"""Compile owned standalone fixtures with both converters and check PDF text."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "corpus/standalone"
WORK = Path(os.environ.get("CONF_WORK") or tempfile.mkdtemp(prefix="carta-compile.")).resolve()
CONVERTERS = {
    "carta": Path(os.environ.get("CARTA_BIN", ROOT / "target/debug/carta")).resolve(),
    "oracle": ROOT / ".oracle/bin/pandoc",
}
CASES = {
    "horizontal-rule": ("markdown", "md", ["Before the rule.", "After the rule."]),
    "image": ("markdown", "md", ["Before the image.", "After the image."]),
    "image-wide": ("markdown", "md", ["Wide image."]),
    "image-tall": ("markdown", "md", ["Tall image."]),
    "strikeout": ("markdown", "md", ["Keep this", "remove that"]),
    "rowspan": ("html", "html", ["Shared cell", "First row", "Second row"]),
    "svg": ("markdown", "md", ["Before the vector image.", "After the vector image."]),
}


def run(arguments, directory, log):
    with log.open("ab") as output:
        subprocess.run(
            arguments,
            cwd=directory,
            stdout=output,
            stderr=subprocess.STDOUT,
            check=True,
            timeout=60,
        )


def compile_case(target, case, name, binary):
    directory = WORK / target / case / name
    directory.mkdir(parents=True, exist_ok=True)
    log = directory / "compile.log"
    log.write_text("")
    source_format, extension, expected = CASES[case]
    suffix = "typ" if target == "typst" else "tex"
    source = directory / f"document.{suffix}"
    for asset in ("rectangle.png", "rectangle.svg", "wide.png", "tall.png"):
        shutil.copyfile(FIXTURES / asset, directory / asset)
    run(
        [str(binary), "-f", source_format, "-t", target, "-s",
         str(FIXTURES / f"{case}.{extension}"), "-o", str(source)],
        directory, log,
    )
    if target == "typst":
        command = ["typst", "compile", source.name, "document.pdf"]
    else:
        command = ["pdflatex", "-interaction=nonstopmode", "-halt-on-error",
                   "-shell-escape" if case == "svg" else "-no-shell-escape", source.name]
    run(command, directory, log)
    run(["pdftotext", "document.pdf", "document.txt"], directory, log)
    text = " ".join((directory / "document.txt").read_text().split())
    for phrase in expected:
        if phrase not in text:
            raise ValueError(f"missing PDF text: {phrase}")


def main():
    for program in ("typst", "pdflatex", "inkscape", "pdftotext"):
        if shutil.which(program) is None:
            raise SystemExit(f"missing program: {program}")
    failed = 0
    for target in ("typst", "latex", "beamer"):
        cases = ["horizontal-rule"] if target == "typst" else [
            "image", "image-wide", "image-tall", "strikeout", "rowspan", "svg",
        ]
        passed = errors = 0
        for case in cases:
            for name, binary in CONVERTERS.items():
                try:
                    compile_case(target, case, name, binary)
                except (OSError, ValueError, subprocess.SubprocessError) as error:
                    errors += 1
                    print(f"FAIL {target}/{case}/{name}: {error}", flush=True)
                else:
                    passed += 1
                    print(f"PASS {target}/{case}/{name}", flush=True)
        print(f"RESULT compile {target} pass={passed} fail={errors} err=0 skip=0")
        failed += errors
    print(f"Documents and logs: {WORK}")
    return bool(failed)


if __name__ == "__main__":
    raise SystemExit(main())
