#!/usr/bin/env python3
"""Compare PDF text, metadata, images, fonts, and errors through external Typst."""

import base64
import http.server
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]
WORK = Path(os.environ.get("CONF_WORK") or tempfile.mkdtemp(prefix="carta-pdf.")).resolve() / "pdf"
WORK.mkdir(parents=True, exist_ok=True)
CONVERTERS = {
    "carta": Path(os.environ.get("CARTA_BIN", ROOT / "target/debug/carta")).resolve(),
    "oracle": ROOT / ".oracle/bin/pandoc",
}
ENV = dict(os.environ, SOURCE_DATE_EPOCH="0")
PNG = (ROOT / "corpus/standalone/rectangle.png").read_bytes()


def run(arguments, **keywords):
    return subprocess.run(arguments, cwd=WORK, env=ENV, capture_output=True, timeout=60, **keywords)


def inspect(tool, path, *arguments):
    return run([tool, *arguments, str(path)], check=True).stdout.decode()


def convert(name, label, source, arguments=(), source_format="markdown", success=True):
    output = WORK / f"{label}-{name}.pdf"
    output.unlink(missing_ok=True)
    result = run([str(CONVERTERS[name]), "-f", source_format, "--pdf-engine=typst", *arguments, "-o", str(output)], input=source.encode() if isinstance(source, str) else source)
    (WORK / f"{label}-{name}.log").write_bytes(result.stderr)
    if success:
        assert result.returncode == 0, result.stderr.decode()
        assert output.read_bytes().startswith(b"%PDF-")
    else:
        assert result.returncode != 0, f"{name} accepted invalid input"
        assert not output.exists(), f"{name} wrote output after failure"
    return output


def text(path):
    return " ".join(run(["pdftotext", str(path), "-"], check=True).stdout.decode().split())


def compare(label, source, arguments=(), source_format="markdown", phrases=(), images=False, font=None):
    paths = [convert(name, label, source, arguments, source_format) for name in CONVERTERS]
    page_geometry = [[line for line in inspect("pdfinfo", path).splitlines() if line.startswith(("Page size:", "Pages:"))] for path in paths]
    assert page_geometry[0] == page_geometry[1], page_geometry
    texts = [text(path) for path in paths]
    if phrases:
        for phrase in phrases:
            assert all(phrase in value for value in texts), (phrase, texts)
    else:
        assert texts[0] == texts[1], texts
    if images:
        sizes = []
        for path in paths:
            rows = inspect("pdfimages", path, "-list").splitlines()[2:]
            sizes.append([row.split()[3:5] for row in rows])
        assert sizes[0] == sizes[1] and sizes[0], sizes
    if font:
        for path in paths:
            assert font in inspect("pdffonts", path), path
    return paths


def checks():
    yield "basic", lambda: compare("basic", "# Heading\n\nHello PDF.\n\n---\n\nEnd.")
    source = (ROOT / "corpus/pdf/content.md").read_text()
    yield "content", lambda: compare("content", source, ("--toc", "--number-sections"), phrases=(
        "PDF test", "Alice Example", "Bob Example", "bold text", "italic text", "web link", "First item", "Second item",
        "Step one", "Step two", "Quoted text", "Footnote text", "Alpha", "Beta", "block code", "inline code"))
    for case in ("image", "image-wide", "image-tall", "svg", "strikeout", "rowspan"):
        extension = "html" if case == "rowspan" else "md"
        source_format = "html" if case == "rowspan" else "markdown"
        document = (ROOT / f"corpus/standalone/{case}.{extension}").read_text()
        yield case, lambda case=case, document=document, source_format=source_format: compare(case, document,
            ("--resource-path", str(ROOT / "corpus/standalone")), source_format, images=case.startswith("image"))
    assets = WORK / "assets with spaces"
    assets.mkdir(exist_ok=True)
    (assets / "blue image.png").write_bytes(PNG)
    yield "escaped-path", lambda: compare("escaped-path", "Image: ![](<blue image.png>)", ("--resource-path", str(assets)), images=True)
    yield "absolute-path", lambda: compare("absolute-path", f"Image: ![](<{assets / 'blue image.png'}>)", images=True)
    uri = "data:image/png;base64," + base64.b64encode(PNG).decode()
    yield "data-uri", lambda: compare("data-uri", f"Image: ![]({uri})", images=True)
    yield "metadata-image", lambda: compare("metadata-image", f"---\ntitle: Title\nabstract: '![]({uri})'\n---\n\nBody.", images=True)
    yield "fonts", lambda: compare("fonts", "Body text and `code text`.\n\n$x^2$",
        ("-V", "mainfont=DejaVu Sans Mono", "-V", "codefont=DejaVu Sans Mono", "-V", "mathfont=New Computer Modern Math", "-V", "fontsize=14pt"),
        phrases=("Body text and code text.",), font="DejaVuSansMono")
    defaults = ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "DejaVu Sans", "DejaVuSans")
    if not Path(defaults[0]).exists():
        defaults = ("/System/Library/Fonts/Supplemental/Arial.ttf", "Arial", "Arial")
    font_file = Path(os.environ.get("PDF_TEST_FONT", defaults[0]))
    font_family = os.environ.get("PDF_TEST_FONT_FAMILY", defaults[1])
    font_postscript = os.environ.get("PDF_TEST_FONT_NAME", defaults[2])
    fonts = WORK / "fonts with spaces"
    fonts.mkdir(exist_ok=True)
    shutil.copyfile(font_file, fonts / font_file.name)
    yield "font-path", lambda: compare("font-path", "Custom font text.",
        ("--pdf-engine-opt=--font-path", f"--pdf-engine-opt={fonts}", "--pdf-engine-opt=--ignore-system-fonts", "-V", f"mainfont={font_family}"), font=font_postscript)
    template = WORK / "custom.typst"
    template.write_text('#set text(font: "DejaVu Sans Mono")\nCustom header\n\n$body$')
    yield "codefont", lambda: compare("codefont", "`Code font text`",
        ("-V", f"codefont={font_family}", "--pdf-engine-opt=--font-path", f"--pdf-engine-opt={fonts}"), font=font_postscript)
    yield "custom-template", lambda: compare("custom-template", "Body text.", ("--template", str(template)), font="DejaVuSansMono")
    yield "typst-output", lambda: compare("typst-output", "PDF from explicit Typst.", ("-t", "typst"))
    yield "explicit-pdf", lambda: compare("explicit-pdf", "PDF from explicit PDF.", ("-t", "pdf"))
    yield "missing-image", lambda: compare("missing-image", "Before ![Missing image](no-image.png) after.")
    for label, document in (("invalid-typst", "```{=typst}\n#unknown-function()\n```"),):
        yield label, lambda label=label, document=document: [convert(name, label, document, success=False) for name in CONVERTERS]


def metadata():
    paths = compare("metadata", '---\ntitle: A title\nauthor: "Alice *Example*"\n---\n\nBody.', phrases=("A title", "Alice Example", "Body."))
    for path in paths:
        info = inspect("pdfinfo", path)
        assert "Title:           A title" in info, info
        assert "Author:          Alice Example" in info, info


def reproducible():
    first = convert("carta", "repeat-first", "![](data:image/png;base64," + base64.b64encode(PNG).decode() + ")").read_bytes()
    second = convert("carta", "repeat-second", "![](data:image/png;base64," + base64.b64encode(PNG).decode() + ")").read_bytes()
    assert first == second, "PDF bytes change between runs"


def embedded():
    document = run([str(CONVERTERS["carta"]), "-f", "markdown", "-t", "docx"],
        input=("Embedded: ![](data:image/png;base64," + base64.b64encode(PNG).decode() + ")").encode(), check=True).stdout
    compare("embedded-image", document, source_format="docx", images=True)


def keywords():
    source = "---\ntitle: Title\nkeywords: [one]\n---\n\nBody."
    path = convert("carta", "keywords", source)
    assert "Keywords:        one" in inspect("pdfinfo", path)
    result = run([str(CONVERTERS["oracle"]), "-f", "markdown", "--pdf-engine=typst", "-o", str(WORK / "keywords-oracle.pdf")], input=source.encode())
    # The pinned binary emits an unquoted keyword; keep its failure separate from PDF parity.
    assert result.returncode != 0 and b"unknown variable: one" in result.stderr
    (WORK / "keywords-oracle.log").write_bytes(result.stderr)


class ImageServer(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "image/png")
        self.end_headers()
        self.wfile.write(PNG)

    def log_message(self, *arguments):
        pass


def remote():
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), ImageServer) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            compare("remote-image", f"Remote image: ![](http://127.0.0.1:{server.server_port}/image.png)", images=True)
        finally:
            server.shutdown()
            thread.join()


passed = failed = 0
for label, check in [*checks(), ("metadata", metadata), ("reproducible", reproducible), ("remote-image", remote), ("embedded-image", embedded), ("keywords (oracle failure verified)", keywords)]:
    try:
        check()
        passed += 1
        print(f"PASS {label}", flush=True)
    except (AssertionError, OSError, subprocess.SubprocessError) as error:
        failed += 1
        print(f"FAIL {label}: {error}", flush=True)
print(f"RESULT pdf typst pass={passed} fail={failed} err=0 skip=0")
print(f"PDF evidence: {WORK}")
raise SystemExit(bool(failed))
