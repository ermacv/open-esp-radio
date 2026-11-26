#!/usr/bin/env python3
"""Trial (tasks/h003): the cases of a package's `tests/ui` as `ui_test` cases.

    convert.py <tests/ui> <destination> [header|name|runner]

Each `<case>.rs` is copied with its markers rewritten; every other file below
`tests/ui` but a snapshot (`*.stderr`) is copied as it is, so an `include!`
finds what it names. Nothing in `tests/ui` changes.

The third argument is how a case says it is a twin (`header` when absent):
`header` as below; `name` calls the rejecting revision `ui_reject` and writes
no `compile-flags` line; `runner` writes no line at all and leaves both
revisions to the runner's defaults.

    the twin (`ui_reject`)             //@revisions: accept reject
                                       //@[accept] check-pass
                                       //@[reject] compile-flags: --cfg ui_reject
    // ui-error: E0499                 //~[reject] E0499
    // ui-error: message               //~[reject] ERROR: message
    // ui-error: E0080: message        //~[reject] ERROR: message      (the code goes: see REPORT)
    // ui-error@-N: X                  //~[reject]^^ X                 (N carets)
    // ui-error@+N: X                  //~[reject]vv X                 (N letters)
    // ui-error@outside: E0080: msg    //@[reject] error-in-other-file: msg   (a header line)

`ui_test` places an error that a macro's expansion reports at the line of the
outermost invocation in the case, where `ui-cases` reads the primary location
rustc prints. The case's snapshot (`<case>.stderr`) names both, so a marker
whose error the snapshot shows "in this macro invocation" moves to that line.

The script prints one line a marker that had no exact counterpart.
"""

import pathlib
import re
import shutil
import sys

MARKER = re.compile(r"\s*// ui-error(?P<place>@[+-]\d+|@outside)?: (?P<error>.*)$")
CODE = re.compile(r"E\d{4}$")
# The lines a case opens with, and the name of its rejecting revision.
TWINS = {
    "header": (
        [
            "//@revisions: accept reject",
            "//@[accept] check-pass",
            "//@[reject] compile-flags: --cfg ui_reject",
        ],
        "reject",
    ),
    "name": (["//@revisions: accept ui_reject", "//@[accept] check-pass"], "ui_reject"),
    "runner": ([], "ui_reject"),
}
LOCATION = re.compile(r"\s*(?:-->|:::) (?P<file>\S+?)(?::(?P<line>\d+):\d+)?$")
GUTTER = re.compile(r"\s*(?P<line>\d+) \|")


def invocation(snapshot: str, case: str, line: int) -> int | None:
    """The last line of `case` that `snapshot` labels "in this macro
    invocation" in the error whose primary location is `line` of `case`."""
    found = None
    inside = False
    file = None
    previous = None
    for text in snapshot.splitlines():
        if text.startswith(("error", "warning")):
            inside = False
            file = None
        location = LOCATION.match(text)
        if location is not None:
            file = location["file"]
            if text.lstrip().startswith("-->"):
                inside = file == case and location["line"] == str(line)
        elif inside and file == case and "in this macro invocation" in text and previous:
            found = int(previous["line"])
        previous = GUTTER.match(text) or previous
    return found


def expected(error: str) -> tuple[str | None, str | None]:
    """The code and the message a marker names, as `ui-cases` reads them."""
    head, colon, rest = error.partition(":")
    if CODE.match(head.strip()):
        return head.strip(), rest.strip() if colon else None
    return None, error.strip()


def convert(name: str, source: str, snapshot: str, twin: str, notes: list[str]) -> str:
    header, reject = TWINS[twin]
    other_file: list[str] = []
    body: list[str] = []
    moved: list[tuple[int, str]] = []
    for number, line in enumerate(source.splitlines(), start=1):
        marker = MARKER.search(line)
        if marker is None:
            if "ui-error" in line:
                notes.append(f"{name}:{number}: a line names `ui-error` and is no marker")
            body.append(line)
            continue
        code, message = expected(marker["error"])
        code_line = line[: marker.start()]
        place = marker["place"]
        if place == "@outside":
            if message is None:
                notes.append(f"{name}:{number}: `@outside` with a code alone: nothing to match by")
                message = code
            elif code is not None:
                notes.append(f"{name}:{number}: `@outside` {code} and its message: the message alone is matched")
            other_file.append(f"//@[{reject}] error-in-other-file: {message}")
            if code_line.strip():
                body.append(code_line)
            continue
        if message is None:
            pattern = code
        else:
            pattern = f"ERROR: {message}"
            if code is not None:
                notes.append(f"{name}:{number}: {code} and its message at a line: the message alone is matched")
        offset = int(place[1:]) if place is not None else 0
        invoked = invocation(snapshot, f"tests/ui/{name}", number + offset)
        if invoked is not None and invoked != number + offset:
            notes.append(
                f"{name}:{number}: the error is a macro's: `ui_test` places it at line "
                f"{invoked}, the invocation, and the annotation moves there"
            )
            moved.append((invoked, f"//~[{reject}] {pattern}"))
            if code_line.strip():
                body.append(code_line)
            else:
                notes.append(f"{name}:{number}: a marker on a line of its own: the line is kept empty")
                body.append("")
            continue
        arrow = ("^" if offset < 0 else "v") * abs(offset)
        annotation = f"//~[{reject}]{arrow} {pattern}"
        indent = line[: len(line) - len(line.lstrip())]
        body.append(f"{code_line} {annotation}" if code_line.strip() else indent + annotation)
    for target, annotation in moved:
        body[target - 1] = f"{body[target - 1]} {annotation}"
    return "\n".join(header + other_file + body) + "\n"


def main() -> int:
    ui, destination = (pathlib.Path(argument) for argument in sys.argv[1:3])
    twin = sys.argv[3] if len(sys.argv) > 3 else "header"
    if destination.exists():
        shutil.rmtree(destination)
    notes: list[str] = []
    cases = 0
    for path in sorted(ui.rglob("*")):
        if path.is_dir() or path.suffix == ".stderr":
            continue
        target = destination / path.relative_to(ui)
        target.parent.mkdir(parents=True, exist_ok=True)
        if path.parent == ui and path.suffix == ".rs":
            snapshot = path.with_suffix(".stderr")
            snapshot = snapshot.read_text() if snapshot.exists() else ""
            target.write_text(convert(path.name, path.read_text(), snapshot, twin, notes))
            cases += 1
        else:
            shutil.copyfile(path, target)
    print(f"{cases} cases converted into {destination}")
    for note in notes:
        print(note)
    return 0


if __name__ == "__main__":
    sys.exit(main())
