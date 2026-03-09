import argparse
import os
import re
import sys
from dataclasses import dataclass
from typing import Iterable, Optional, Tuple
from urllib.parse import unquote, urlparse


FILE_URI_RE = re.compile(r"file:///[^\s\)\"'>]+")


@dataclass(frozen=True)
class ReplaceResult:
    new_text: str
    matches: int
    changed_links: int
    skipped: int


def _posix_relpath(from_dir: str, to_path: str) -> str:
    rel = os.path.relpath(to_path, start=from_dir)
    return rel.replace(os.sep, "/")


def _split_fragment(uri: str) -> Tuple[str, str]:
    if "#" not in uri:
        return uri, ""
    base, frag = uri.split("#", 1)
    return base, "#" + frag


def _file_uri_to_path(uri: str) -> Optional[str]:
    p = urlparse(uri)
    if p.scheme != "file":
        return None
    return unquote(p.path)


def _iter_md_files(root: str) -> Iterable[str]:
    for dirpath, _, filenames in os.walk(root):
        for name in filenames:
            if name.lower().endswith(".md"):
                yield os.path.join(dirpath, name)


def _normalize_abs(p: str) -> str:
    return os.path.normpath(os.path.abspath(p))

def _map_to_docs_if_known_subpath(abs_path: str, docs_root_n: str) -> Optional[str]:
    needles = [
        "/acceptance/",
        "/acceptance_extras/",
        "/e2e/",
        "/iteration/",
    ]
    for needle in needles:
        if needle not in abs_path:
            continue
        suffix = abs_path.split(needle, 1)[1]
        base = needle.strip("/")
        candidate = _normalize_abs(os.path.join(docs_root_n, base, suffix))
        if os.path.exists(candidate):
            return candidate
    return None


def rewrite_file_uris(
    *,
    md_path: str,
    text: str,
    scope_root: str,
    only_under_docs: bool,
    docs_root: str,
    skip_missing: bool,
    rewrite_known_docs_subpaths: bool,
) -> ReplaceResult:
    md_dir = os.path.dirname(md_path)
    scope_root_n = _normalize_abs(scope_root)
    docs_root_n = _normalize_abs(docs_root)
    stats = {"matches": 0, "changed": 0, "skipped": 0}

    def repl(m: re.Match) -> str:
        uri = m.group(0)
        stats["matches"] += 1
        base, frag = _split_fragment(uri)
        path = _file_uri_to_path(base)
        if not path:
            return uri
        abs_path = _normalize_abs(path)
        target = abs_path

        in_scope = target.startswith(scope_root_n + os.sep) or target == scope_root_n
        if not in_scope and rewrite_known_docs_subpaths:
            mapped = _map_to_docs_if_known_subpath(target, docs_root_n)
            if mapped is not None:
                target = mapped
                in_scope = target.startswith(scope_root_n + os.sep) or target == scope_root_n

        if not in_scope:
            stats["skipped"] += 1
            return uri

        if only_under_docs and not (target.startswith(docs_root_n + os.sep) or target == docs_root_n):
            stats["skipped"] += 1
            return uri
        if skip_missing and not os.path.exists(target):
            stats["skipped"] += 1
            return uri
        rel = _posix_relpath(md_dir, target)
        out = rel + frag
        if out != uri:
            stats["changed"] += 1
        return out

    new_text = FILE_URI_RE.sub(repl, text)
    return ReplaceResult(
        new_text=new_text,
        matches=stats["matches"],
        changed_links=stats["changed"],
        skipped=stats["skipped"],
    )


def main(argv: Optional[Iterable[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        prog="convert_file_links.py",
        description="Convert file:// absolute links in Markdown into relative links.",
    )
    parser.add_argument(
        "--docs-root",
        required=True,
        help="Absolute path to DeepAgentsRS/docs directory.",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="Apply changes in-place. Without this flag, performs a dry-run.",
    )
    parser.add_argument(
        "--scope",
        choices=["docs", "repo"],
        default="docs",
        help="Which file:// targets are eligible for conversion.",
    )
    parser.add_argument(
        "--scope-root",
        help="Override scope root with an absolute path. When set, --scope is ignored.",
    )
    parser.add_argument(
        "--rewrite-known-docs-subpaths",
        action="store_true",
        help="Also rewrite file:// links pointing to older docs layouts (acceptance/e2e/iteration) by mapping them into docs_root if the target exists.",
    )
    parser.add_argument(
        "--skip-missing",
        action="store_true",
        help="Skip converting links whose target path does not exist.",
    )
    parser.add_argument(
        "--fail-on-changes",
        action="store_true",
        help="Exit with non-zero if any changes would be made (useful in CI).",
    )
    args = parser.parse_args(list(argv) if argv is not None else None)

    docs_root = _normalize_abs(args.docs_root)
    if not os.path.isdir(docs_root):
        print(f"docs_root is not a directory: {docs_root}", file=sys.stderr)
        return 2

    repo_root = _normalize_abs(os.path.dirname(docs_root))
    if args.scope_root:
        scope_root = _normalize_abs(args.scope_root)
        only_under_docs = False
    else:
        scope_root = docs_root if args.scope == "docs" else repo_root
        only_under_docs = args.scope == "docs"

    changed_files = 0
    total_matches = 0
    total_changed_links = 0
    total_skipped = 0
    for md_path in sorted(_iter_md_files(docs_root)):
        with open(md_path, "r", encoding="utf-8") as f:
            text = f.read()
        r = rewrite_file_uris(
            md_path=md_path,
            text=text,
            scope_root=scope_root,
            only_under_docs=only_under_docs,
            docs_root=docs_root,
            skip_missing=args.skip_missing,
            rewrite_known_docs_subpaths=args.rewrite_known_docs_subpaths,
        )
        total_matches += r.matches
        total_changed_links += r.changed_links
        total_skipped += r.skipped
        if r.new_text != text:
            changed_files += 1
            if args.apply:
                with open(md_path, "w", encoding="utf-8") as f:
                    f.write(r.new_text)

    print(
        f"files_changed={changed_files} links_changed={total_changed_links} links_seen={total_matches} skipped={total_skipped} apply={args.apply}"
    )
    if args.fail_on_changes and changed_files > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
