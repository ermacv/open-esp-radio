#!/usr/bin/env python3
"""PreToolUse guard for Claude Code sessions in this repository.

Reads the hook input JSON on stdin and blocks (exit 2, one line on stderr):

- a Read of a large generated file (raw PAC, PAC API catalog, published SVD
  or binding index, vendor fact registries), or a Bash command that prints
  one whole (cat, head, sed, ...): grep it with an explicit path;
- an Edit or Write of such a file: regenerate it with its owner's command.

Everything else exits 0 without output, leaving the normal permission flow.
Only the Python standard library is used.
"""

import json
import os
import re
import shlex
import sys

GENERATED = re.compile(
    r"(?:^|/)(?:"
    r"chips/[^/]+/[^/]*pac-raw/src/lib\.rs"
    r"|chips/[^/]+/[^/]*pac/src/generated\.rs"
    r"|registers/(?:[^/]+/)?published/[^/]+\.(?:svd|bindings\.toml)"
    r"|vendor/facts/provenance\.toml"
    r"|vendor/facts/names/[^/]+\.toml"
    r")$"
)
GENERATED_HINT = (
    "is a large generated file: grep it with an explicit path "
    "(Grep with path=<file> and -C context) instead of reading it"
)

# Generated files and the command that writes each; never edited by hand.
REGENERATED = [
    (re.compile(r"(?:^|/)(?:chips/[^/]+/[^/]*pac-raw/src/lib\.rs"
                r"|chips/[^/]+/[^/]*pac/src/generated\.rs"
                r"|registers/(?:[^/]+/)?published/[^/]+\.(?:svd|bindings\.toml))$"),
     "`cargo registers generate --manifest registers/<chip>/publication/<publication>.toml` "
     "after editing the register model or policy"),
    (re.compile(r"(?:^|/)linker/dma-window\.x$"),
     "`cargo registers generate --manifest registers/<chip>/publication/registers.toml` "
     "after editing the memory map or the register model"),
    (re.compile(r"(?:^|/)vendor/facts/(?:provenance\.toml|names/[^/]+\.toml)$"),
     "no tool in this repository writes it: a machine-written vendor fact registry "
     "changes only by re-deriving it, which the owner decides"),
]

# Commands that print a file's contents when it is named as an argument.
READERS = {
    "cat", "tac", "less", "more", "bat", "batcat", "head", "tail", "sed",
    "awk", "gawk", "nl", "xxd", "od", "hexdump", "strings", "view",
}


def block(message):
    print(f"Blocked by .claude/hooks/pre_tool_use.py: {message}", file=sys.stderr)
    sys.exit(2)


def words_of(command):
    """Every word of a shell command, its separators dropped."""
    lexer = shlex.shlex(command.replace("\n", " ; "), posix=True,
                        punctuation_chars=";&|()<>")
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        return list(lexer)
    except ValueError:
        return re.findall(r"[;&|()<>]+|[^\s;&|()<>]+", command)


def check_bash(tool_input):
    command = tool_input.get("command") or ""
    words = words_of(command)
    for index, word in enumerate(words):
        name = os.path.basename(word)
        if name in READERS:
            candidates = words[index + 1:]
        elif name == "git" and index + 1 < len(words) and words[index + 1] in {"show", "cat-file"}:
            candidates = [w.split(":", 1)[-1] for w in words[index + 2:]]
        else:
            continue
        for candidate in candidates:
            if candidate and set(candidate) <= set(";&|()"):
                break
            if GENERATED.search(candidate.lstrip("<")):
                block(f"`{candidate}` {GENERATED_HINT}")


def check_read(tool_input):
    path = tool_input.get("file_path") or ""
    if GENERATED.search(path.replace("\\", "/")):
        block(f"`{path}` {GENERATED_HINT}")


def check_edit(tool_input):
    path = (tool_input.get("file_path") or "").replace("\\", "/")
    for pattern, command in REGENERATED:
        if pattern.search(path):
            block(f"`{path}` is generated; never edit it by hand, regenerate it "
                  f"with {command} (CLAUDE.md, Generated files)")


def main():
    try:
        event = json.load(sys.stdin)
    except (ValueError, OSError):
        return 0
    tool = event.get("tool_name")
    tool_input = event.get("tool_input") or {}
    if tool == "Bash":
        check_bash(tool_input)
    elif tool == "Read":
        check_read(tool_input)
    elif tool in {"Edit", "Write"}:
        check_edit(tool_input)
    return 0


if __name__ == "__main__":
    sys.exit(main())
