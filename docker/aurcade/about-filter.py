#!/usr/bin/env python3
import html
import os
import posixpath
import re
import subprocess
import sys
from urllib.parse import quote, urlsplit, urlunsplit

IMAGE_SRC = re.compile(
    r'(?P<prefix><img\b[^>]*?\bsrc\s*=\s*)(?P<quote>["\'])(?P<url>[^"\']+)(?P=quote)',
    re.IGNORECASE,
)
FORMATTER = "/usr/lib/cgit/filters/about-formatting.sh"


def rewrite_images(document: str, repository: str) -> str:
    base = f"/{quote(repository.strip('/'), safe='/')}/plain/"

    def replace(match: re.Match[str]) -> str:
        url = html.unescape(match.group("url"))
        parts = urlsplit(url)
        if parts.scheme or parts.netloc or url.startswith(("/", "#")):
            return match.group(0)
        path = posixpath.normpath(parts.path)
        if path in ("", ".", "..") or path.startswith("../"):
            return match.group(0)
        rewritten = urlunsplit(("", "", base + quote(path, safe="/%:@-._~"), parts.query, parts.fragment))
        return (
            match.group("prefix")
            + match.group("quote")
            + html.escape(rewritten, quote=True)
            + match.group("quote")
        )

    return IMAGE_SRC.sub(replace, document)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: aurcade-about-filter FILENAME", file=sys.stderr)
        return 2
    rendered = subprocess.run(
        [FORMATTER, sys.argv[1]],
        input=sys.stdin.buffer.read(),
        stdout=subprocess.PIPE,
        check=False,
    )
    if rendered.returncode:
        return rendered.returncode
    document = rendered.stdout.decode("utf-8")
    repository = os.environ.get("CGIT_REPO_URL", "")
    sys.stdout.write(rewrite_images(document, repository) if repository else document)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
