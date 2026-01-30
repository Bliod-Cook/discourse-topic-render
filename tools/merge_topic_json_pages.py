#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple


PostsPath = Tuple[str, ...]


def _die(msg: str, code: int = 2) -> None:
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(code)


def _load_json(path: Path) -> Dict[str, Any]:
    try:
        with path.open("rb") as f:
            return json.load(f)
    except Exception as e:
        _die(f"failed to read json: {path}: {e}")


def _find_posts(doc: Dict[str, Any]) -> Tuple[List[Dict[str, Any]], PostsPath]:
    ps = doc.get("post_stream")
    if isinstance(ps, dict):
        posts = ps.get("posts")
        if isinstance(posts, list):
            return posts, ("post_stream", "posts")

    posts = doc.get("posts")
    if isinstance(posts, list):
        return posts, ("posts",)

    _die("input json does not contain `post_stream.posts` (or top-level `posts`)")


def _set_path(doc: Dict[str, Any], path: PostsPath, value: Any) -> None:
    cur: Any = doc
    for k in path[:-1]:
        if not isinstance(cur, dict) or k not in cur:
            _die(f"cannot set path {'.'.join(path)}: missing key `{k}`")
        cur = cur[k]
    if not isinstance(cur, dict):
        _die(f"cannot set path {'.'.join(path)}: parent is not an object")
    cur[path[-1]] = value


def _topic_id(doc: Dict[str, Any]) -> Optional[int]:
    tid = doc.get("id")
    if isinstance(tid, int):
        return tid
    # Some responses might include `topic_id` at top-level.
    tid = doc.get("topic_id")
    if isinstance(tid, int):
        return tid
    return None


def _post_key(post: Dict[str, Any]) -> Tuple[str, Any]:
    # Prefer stable `id`, then `post_number`.
    if isinstance(post.get("id"), int):
        return ("id", post["id"])
    if isinstance(post.get("post_number"), int):
        return ("post_number", post["post_number"])
    # Fallback: try (username, created_at) if present; else object identity hash.
    u = post.get("username")
    c = post.get("created_at")
    if isinstance(u, str) and isinstance(c, str):
        return ("username+created_at", (u, c))
    return ("_index", id(post))


def _post_sort_key(post: Dict[str, Any]) -> Tuple[int, int]:
    pn = post.get("post_number")
    if isinstance(pn, int):
        return (0, pn)
    pid = post.get("id")
    if isinstance(pid, int):
        return (1, pid)
    return (2, 0)


def _merge_users(base: Dict[str, Any], others: Sequence[Dict[str, Any]]) -> None:
    if "users" not in base or not isinstance(base.get("users"), list):
        return

    seen: Dict[Tuple[str, Any], Dict[str, Any]] = {}
    merged: List[Dict[str, Any]] = []

    def add(user: Any) -> None:
        if not isinstance(user, dict):
            return
        key: Tuple[str, Any]
        if isinstance(user.get("id"), int):
            key = ("id", user["id"])
        elif isinstance(user.get("username"), str):
            key = ("username", user["username"])
        else:
            return
        if key in seen:
            return
        seen[key] = user
        merged.append(user)

    for u in base.get("users", []):
        add(u)
    for doc in others:
        for u in doc.get("users", []) if isinstance(doc.get("users"), list) else []:
            add(u)

    base["users"] = merged


def _set_post_stream_stream(base: Dict[str, Any], posts: List[Dict[str, Any]]) -> None:
    ps = base.get("post_stream")
    if not isinstance(ps, dict):
        return
    ids: List[int] = []
    for p in posts:
        pid = p.get("id")
        if isinstance(pid, int):
            ids.append(pid)
    if ids:
        ps["stream"] = ids


def merge_topic_json_pages(inputs: Sequence[Path]) -> Dict[str, Any]:
    if not inputs:
        _die("no input files provided")

    docs = [_load_json(p) for p in inputs]
    base = docs[0]

    base_posts, posts_path = _find_posts(base)
    other_posts_lists = []
    for i, doc in enumerate(docs[1:], start=2):
        posts, path = _find_posts(doc)
        if path != posts_path:
            _die(
                f"input #{i} posts location differs: {'.'.join(path)} != {'.'.join(posts_path)}"
            )
        other_posts_lists.append(posts)

    base_id = _topic_id(base)
    if base_id is not None:
        for i, doc in enumerate(docs[1:], start=2):
            tid = _topic_id(doc)
            if tid is not None and tid != base_id:
                _die(f"topic id mismatch: input #1 id={base_id} but input #{i} id={tid}")

    merged_map: Dict[Tuple[str, Any], Dict[str, Any]] = {}
    merged: List[Dict[str, Any]] = []

    def add_posts(posts: List[Any]) -> None:
        for item in posts:
            if not isinstance(item, dict):
                continue
            k = _post_key(item)
            if k in merged_map:
                continue
            merged_map[k] = item
            merged.append(item)

    add_posts(base_posts)
    for posts in other_posts_lists:
        add_posts(posts)

    merged.sort(key=_post_sort_key)

    _set_path(base, posts_path, merged)
    _merge_users(base, docs[1:])
    _set_post_stream_stream(base, merged)

    return base


def main(argv: Optional[Sequence[str]] = None) -> int:
    ap = argparse.ArgumentParser(
        description="Merge multiple paginated Discourse topic JSON files into one (merging post_stream.posts)."
    )
    ap.add_argument(
        "inputs",
        nargs="+",
        type=Path,
        help="Input JSON files (e.g. topic-page1.json topic-page2.json ...)",
    )
    ap.add_argument(
        "-o",
        "--output",
        type=Path,
        required=True,
        help="Output JSON path (merged).",
    )
    args = ap.parse_args(argv)

    merged = merge_topic_json_pages(args.inputs)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as f:
        json.dump(merged, f, ensure_ascii=False, indent=2)
        f.write("\n")

    # Summary to stderr (non-identifying, just counts).
    posts, _ = _find_posts(merged)
    print(
        f"merged {len(args.inputs)} files -> {len(posts)} posts -> {args.output}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

