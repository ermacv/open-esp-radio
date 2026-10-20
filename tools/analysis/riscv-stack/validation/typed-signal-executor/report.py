#!/usr/bin/env python3
"""Summarize the production firmware gate; symbol names only label its sites."""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


def run(*args):
    return subprocess.run(args, check=True, text=True, capture_output=True).stdout


def measure(directory, output, labels):
    output.mkdir(parents=True, exist_ok=True)
    elf = directory / "runtime.elf"
    sizes = run("llvm-size", "-A", str(elf))
    (output / "sections.txt").write_text(sizes)
    sections = {row.split()[0]: int(row.split()[1])
                for row in sizes.splitlines() if row.startswith(".")}
    symbols = run("llvm-nm", "--demangle", "--defined-only", str(elf))
    relevant = [row for row in symbols.splitlines()
                if any(name in row for name in
                       ("signal::", "SignalExecutor", "pend_signal", "signal_context", "SIGNAL"))]
    (output / "signal-symbols.txt").write_text("\n".join(relevant) + "\n")
    for name in ("placement.txt", "runtime-stack.txt", "interrupt-stack.txt", "bootstrap-stack.txt"):
        shutil.copy2(directory / name, output / name)
    irq = (directory / "interrupt-stack.txt").read_text()
    runtime = (directory / "runtime-stack.txt").read_text()
    if labels:
        assert hashlib.sha256(elf.read_bytes()).hexdigest() == labels["elf_sha256"], "labels refer to a different ELF"
    sites, levels = [], []
    hart = level = None
    for row in irq.splitlines():
        if match := re.match(r"hart (\d+):", row):
            hart = int(match[1])
        elif match := re.match(r"  level (\d+): (.*)", row):
            level = int(match[1])
            levels.append({"hart": hart, "level": level, "bound": match[2]})
        elif match := re.search(r"unresolved (.*) at (0x[0-9a-f]+)", row):
            kind, address = match.groups()
            info = json.loads(run("llvm-symbolizer", "--output-style=JSON", "--obj=" + str(elf), address))[0]
            chain = info["Symbol"]
            names = "\n".join(frame["FunctionName"] for frame in chain)
            if "SignalPend" in names:
                category = "signal-pend"
            elif "IrqSignal>::owned" in names:
                category = "signal-running"
            elif "ExternalRxBuffer" in names or "ExternalRxHandoffSlot" in names:
                category = "external-rx-release"
            elif "Receiver<" in names and "indirect jump" in kind:
                category = "receiver-jump-table"
            elif "phy::" in names and "indirect jump" in kind:
                category = "phy-jump-table"
            elif "SyncExecutor>::poll" in names:
                category = "executor-task-poll"
            else:
                category = "other"
            category = labels.get("sites", {}).get(address, category)
            sites.append({"hart": hart, "level": level, "address": address,
                          "kind": kind, "category": category, "inline_chain": chain})
    return {
        "runtime_bin": (directory / "runtime.bin").stat().st_size,
        "runtime_bin_sha256": hashlib.sha256((directory / "runtime.bin").read_bytes()).hexdigest(),
        "runtime_elf_sha256": hashlib.sha256(elf.read_bytes()).hexdigest(),
        "sections": sections,
        "levels": levels,
        "sites": sites,
        "hart_bounds": [row for row in irq.splitlines() if row.startswith("hart ")],
        "task_bounds": [row for row in runtime.splitlines() if re.match(r"(?:CPU\d|stack|task|root|bound)", row)],
        "runtime_frame_coverage": next(row for row in runtime.splitlines() if row.startswith("frame coverage:")),
        "cached_memory": next(row for row in irq.splitlines() if row.startswith("handler code:")),
        "signal_symbols": relevant,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    args = parser.parse_args()
    labels = json.loads(Path(__file__).with_name("labels.json").read_text())
    summary = {"base": args.base, "head": args.head, "images": {}}
    for app in ("w0", "w1", "w2"):
        summary["images"][app] = {
            stage: measure(getattr(args, stage) / app, args.output / stage / app, labels.get(f"{stage}/{app}", {}))
            for stage in ("before", "after")
        }
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    with (args.output / "sites.tsv").open("w") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(("image", "stage", "hart", "level", "address", "category", "kind", "inline leaf"))
        for app, stages in summary["images"].items():
            for stage, data in stages.items():
                for site in data["sites"]:
                    leaf = site["inline_chain"][0]
                    writer.writerow((app, stage, site["hart"], site["level"], site["address"],
                                     site["category"], site["kind"],
                                     f'{leaf["FunctionName"]} ({leaf["FileName"]}:{leaf["Line"]})'))
    lines = ["| Image | Metric | Before | After | Delta |", "| --- | --- | ---: | ---: | ---: |"]
    for app, stages in summary["images"].items():
        for name in ("runtime.bin", ".isr.text", ".text", ".bss", ".critical.data", ".critical.bss"):
            values = [data["runtime_bin"] if name == "runtime.bin" else data["sections"][name]
                      for data in stages.values()]
            lines.append(f"| {app} | {name} | {values[0]} | {values[1]} | {values[1] - values[0]:+} |")
    (args.output / "sizes.md").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
