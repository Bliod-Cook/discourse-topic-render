#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Optional, Sequence, Set, Union
from urllib.parse import urljoin, urlsplit
from urllib.request import Request, urlopen


Origin = Union[Path, str]  # local file path, or remote URL string


IMPORT_RE = re.compile(
    r"""@import\s+(?:url\(\s*)?
        (?:
          "(?P<u_d>[^"]+)"
          |'(?P<u_s>[^']+)'
          |(?P<u2>[^);]+)
        )
        \s*\)?\s*(?P<media>[^;]*)\s*;""",
    re.IGNORECASE | re.VERBOSE,
)

CHARSET_RE = re.compile(
    r"""^\s*@charset\s+(?:"[^"]+"|'[^']+')\s*;\s*""",
    re.IGNORECASE,
)

COMMENT_RE = re.compile(r"/\*.*?\*/", re.DOTALL)


def _die(msg: str, code: int = 2) -> None:
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(code)


def _origin_key(origin: Origin) -> str:
    if isinstance(origin, Path):
        return f"file:{origin.resolve()}"
    return origin


def _is_non_fetchable_url(raw: str) -> bool:
    s = raw.strip().lower()
    return (
        not s
        or s.startswith("data:")
        or s.startswith("about:")
        or s.startswith("#")
        or s.startswith("blob:")
    )


def _scheme_from_base(base_url: Optional[str], origin: Origin) -> str:
    if base_url:
        s = urlsplit(base_url).scheme
        if s:
            return s
    if isinstance(origin, str):
        s = urlsplit(origin).scheme
        if s:
            return s
    return "https"


def _mask_comments(css: str) -> str:
    # Replace comment bodies with spaces so regex indices align with original.
    return COMMENT_RE.sub(lambda m: " " * (m.end() - m.start()), css)


def _read_local_text(path: Path, encoding: str) -> str:
    try:
        return path.read_text(encoding=encoding)
    except Exception as e:
        _die(f"failed to read css: {path}: {e}")


def _fetch_remote_text(url: str, user_agent: str) -> str:
    try:
        req = Request(url, headers={"User-Agent": user_agent})
        with urlopen(req, timeout=30) as r:
            data = r.read()
    except Exception as e:
        _die(f"failed to fetch css: {url}: {e}")
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("utf-8", errors="replace")


@dataclass
class _Ctx:
    base_url: Optional[str]
    encoding: str
    inline_imports: bool
    fetch_remote_imports: bool
    user_agent: str
    visited: Set[str]
    charset_stmt: Optional[str] = None


def _extract_charset(css: str, ctx: _Ctx) -> str:
    m = CHARSET_RE.match(css)
    if not m:
        return css
    stmt = m.group(0).strip()
    if ctx.charset_stmt is None:
        ctx.charset_stmt = stmt
    return css[m.end() :]


def _resolve_import(origin: Origin, raw: str, ctx: _Ctx) -> Optional[Origin]:
    s = raw.strip()
    if _is_non_fetchable_url(s):
        return None

    if s.startswith(("http://", "https://")):
        return s
    if s.startswith("//"):
        scheme = _scheme_from_base(ctx.base_url, origin)
        return f"{scheme}:{s}"

    # Split off ?query / #fragment for filesystem resolution.
    parts = urlsplit(s)
    path_part = parts.path

    # Site-root absolute path: requires --base-url to turn into a URL.
    if path_part.startswith("/"):
        if ctx.base_url:
            return urljoin(ctx.base_url, s)
        # If it happens to exist as an absolute filesystem path, allow it.
        p = Path(path_part)
        if p.exists():
            return p
        return None

    if isinstance(origin, Path):
        return (origin.parent / path_part).resolve()

    # Origin is a remote URL.
    return urljoin(origin, s)


def _bundle_origin(origin: Origin, ctx: _Ctx) -> str:
    key = _origin_key(origin)
    if key in ctx.visited:
        return ""
    ctx.visited.add(key)

    if isinstance(origin, Path):
        css = _read_local_text(origin, ctx.encoding)
        base: Origin = origin
    else:
        css = _fetch_remote_text(origin, ctx.user_agent)
        base = origin

    css = _extract_charset(css, ctx)
    return _inline_imports(css, base, ctx)


def _inline_imports(css: str, origin: Origin, ctx: _Ctx) -> str:
    if not ctx.inline_imports:
        return css

    masked = _mask_comments(css)
    out: list[str] = []
    last = 0

    for m in IMPORT_RE.finditer(masked):
        out.append(css[last : m.start()])

        url_raw = (
            (m.group("u_d") or m.group("u_s") or m.group("u2") or "").strip()
        )
        media = (m.group("media") or "").strip()

        resolved = _resolve_import(origin, url_raw, ctx)
        if resolved is None:
            # Keep the original @import statement.
            out.append(css[m.start() : m.end()])
            last = m.end()
            continue

        if isinstance(resolved, str) and not ctx.fetch_remote_imports:
            # Keep remote @import unless allowed to fetch.
            out.append(css[m.start() : m.end()])
            last = m.end()
            continue

        imported = _bundle_origin(resolved, ctx)
        if not imported.strip():
            last = m.end()
            continue

        if media:
            out.append(f"@media {media} {{\n{imported}\n}}\n")
        else:
            out.append(imported)
            if not imported.endswith("\n"):
                out.append("\n")

        last = m.end()

    out.append(css[last:])
    return "".join(out)


def merge_css(inputs: Sequence[Path], output: Path, args: argparse.Namespace) -> None:
    ctx = _Ctx(
        base_url=args.base_url,
        encoding=args.encoding,
        inline_imports=not args.no_inline_imports,
        fetch_remote_imports=args.fetch_remote_imports,
        user_agent=args.user_agent,
        visited=set(),
    )

    chunks: list[str] = []
    for p in inputs:
        chunks.append(_bundle_origin(p, ctx))
        if chunks and not chunks[-1].endswith("\n"):
            chunks.append("\n")

    merged = "".join(chunks)
    if ctx.charset_stmt:
        merged = ctx.charset_stmt + "\n" + merged

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(merged, encoding="utf-8")


def main(argv: Optional[Sequence[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Merge multiple CSS files into one.")
    ap.add_argument(
        "inputs",
        nargs="+",
        type=Path,
        help="Input CSS files (merged in the given order).",
    )
    ap.add_argument(
        "-o",
        "--output",
        type=Path,
        required=True,
        help="Output CSS file path.",
    )
    ap.add_argument(
        "--base-url",
        default=None,
        help='Base URL for resolving root-relative @import like "/foo.css".',
    )
    ap.add_argument(
        "--no-inline-imports",
        action="store_true",
        help="Do not inline @import (just concatenate).",
    )
    ap.add_argument(
        "--fetch-remote-imports",
        action="store_true",
        help="Fetch and inline remote @import URLs (requires network).",
    )
    ap.add_argument(
        "--encoding",
        default="utf-8",
        help="Input file encoding (default: utf-8).",
    )
    ap.add_argument(
        "--user-agent",
        default="merge-css/0.1",
        help="User-Agent for fetching remote @import (default: merge-css/0.1).",
    )
    args = ap.parse_args(argv)

    for p in args.inputs:
        if not p.exists():
            _die(f"input not found: {p}")
        if not p.is_file():
            _die(f"input is not a file: {p}")

    merge_css(args.inputs, args.output, args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

