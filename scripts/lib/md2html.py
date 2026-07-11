#!/usr/bin/env python3
"""Minimal Markdown -> styled HTML converter for Sparkle release notes.

Reads Markdown on stdin, writes a self-contained HTML document on stdout.
Supports the subset used by docs/releases/*.md: ## / ### headings, unordered
and ordered lists, bold, inline code, links, horizontal rules and paragraphs.
"""

import html
import re
import sys

STYLE = """
body {
    font-family: -apple-system, "PingFang SC", sans-serif;
    font-size: 13px;
    line-height: 1.65;
    color: #3d2e1f;
    margin: 0;
    padding: 14px 18px;
}
h2 { font-size: 17px; margin: 0 0 10px; }
h3 { font-size: 14px; margin: 18px 0 6px; }
p { margin: 6px 0; }
ul, ol { margin: 6px 0; padding-left: 22px; }
li { margin: 3px 0; }
a { color: #b4552d; text-decoration: none; }
code {
    font-family: ui-monospace, Menlo, monospace;
    font-size: 12px;
    background: rgba(61, 46, 31, 0.07);
    border-radius: 4px;
    padding: 1px 4px;
}
hr { border: none; border-top: 1px solid rgba(61, 46, 31, 0.15); margin: 14px 0; }
@media (prefers-color-scheme: dark) {
    body { color: #e8e2d8; }
    a { color: #e8956d; }
    code { background: rgba(232, 226, 216, 0.12); }
    hr { border-top-color: rgba(232, 226, 216, 0.2); }
}
"""


def inline(text: str) -> str:
    text = html.escape(text, quote=False)
    text = re.sub(r"`([^`]+)`", r"<code>\1</code>", text)
    text = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r'<a href="\2">\1</a>', text)
    # bare URLs
    text = re.sub(r"(?<![\"'>])(https?://[^\s<]+)", r'<a href="\1">\1</a>', text)
    return text


def convert(md: str) -> str:
    out = []
    list_tag = None

    def close_list():
        nonlocal list_tag
        if list_tag:
            out.append(f"</{list_tag}>")
            list_tag = None

    for raw in md.splitlines():
        line = raw.rstrip()
        stripped = line.strip()
        if not stripped:
            close_list()
            continue
        if stripped in ("---", "***", "___"):
            close_list()
            out.append("<hr>")
        elif stripped.startswith("### "):
            close_list()
            out.append(f"<h3>{inline(stripped[4:])}</h3>")
        elif stripped.startswith("## "):
            close_list()
            out.append(f"<h2>{inline(stripped[3:])}</h2>")
        elif stripped.startswith("# "):
            close_list()
            out.append(f"<h2>{inline(stripped[2:])}</h2>")
        elif stripped.startswith(("- ", "* ")):
            if list_tag != "ul":
                close_list()
                out.append("<ul>")
                list_tag = "ul"
            out.append(f"<li>{inline(stripped[2:])}</li>")
        elif re.match(r"^\d+\.\s+", stripped):
            if list_tag != "ol":
                close_list()
                out.append("<ol>")
                list_tag = "ol"
            out.append(f"<li>{inline(re.sub(r'^\\d+\\.\\s+', '', stripped))}</li>")
        else:
            close_list()
            out.append(f"<p>{inline(stripped)}</p>")

    close_list()
    body = "\n".join(out)
    return (
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">"
        f"<style>{STYLE}</style></head><body>\n{body}\n</body></html>"
    )


if __name__ == "__main__":
    sys.stdout.write(convert(sys.stdin.read()))
