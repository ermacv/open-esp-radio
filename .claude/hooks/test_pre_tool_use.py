"""Regressions of pre_tool_use.py, run as Claude Code runs it: JSON on stdin.

Run them by hand: `python3 -m unittest discover -s .claude/hooks`.
"""

import json
import os
import subprocess
import sys
import unittest

HOOK = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pre_tool_use.py")


def run(tool, **tool_input):
    """The hook's exit status and stderr for one tool call."""
    event = {"tool_name": tool, "tool_input": tool_input}
    result = subprocess.run([sys.executable, HOOK], input=json.dumps(event),
                            capture_output=True, text=True, check=False)
    return result.returncode, result.stderr


class Edits(unittest.TestCase):
    def assert_blocked(self, tool, path, command):
        status, stderr = run(tool, file_path=path)
        self.assertEqual(status, 2, path)
        self.assertIn("is generated", stderr)
        self.assertIn(command, stderr)

    def test_generated_files_are_regenerated_not_edited(self):
        for tool, path, command in [
            ("Edit", "/w/chips/esp32s31/pac/src/generated.rs", "cargo registers generate"),
            ("Write", "chips/esp32s31/pac/src/generated.rs", "cargo registers generate --chip"),
            ("Edit", "/w/chips/esp32s31/layout/linker/dma-window.x", "cargo registers generate --chip"),
            ("Edit", "/w/chips/esp32s31/vendor/facts/idf-function-signatures.toml", "cargo registers signatures"),
            ("Write", "chips/esp32s31/vendor/facts/idf-function-signatures.provenance.toml", "cargo registers signatures"),
            ("Edit", "/w/chips/esp32s31/vendor/facts/provenance.toml", "re-deriving"),
            ("Write", "chips/esp32c5/vendor/facts/names/libpp.toml", "re-deriving"),
            ("Write", "/w/chips/esp32s31/vendor/phy-baselines/4d59230d/wifi-cold.json", "phy-baseline generate"),
        ]:
            self.assert_blocked(tool, path, command)

    def test_handwritten_neighbours_stay_editable(self):
        for path in [
            "/w/chips/esp32s31/pac/src/lib.rs",
            "/w/chips/esp32s31/registers/chip.toml",
            "/w/chips/esp32s31/registers/types/timg.toml",
            "/w/chips/esp32s31/registers/layouts/wifi-rx-dma-descriptor.toml",
            "/w/chips/esp32s31/registers/policy.toml",
            "/w/chips/esp32s31/vendor/artifacts.toml",
            "/w/chips/esp32s31/vendor/facts/names/README.md",
            "/w/.claude/hooks/pre_tool_use.py",
            "/w/chips/espressif/staged-layout/linker/runtime/sections.x",
            "/w/chips/esp32s31/vendor/phy-baselines/README.md",
        ]:
            self.assertEqual(run("Edit", file_path=path), (0, ""), path)


class Reads(unittest.TestCase):
    def test_a_generated_file_is_never_read_whole(self):
        status, stderr = run("Read", file_path="/w/chips/esp32s31/pac/src/generated.rs")
        self.assertEqual(status, 2)
        self.assertIn("grep it with an explicit path", stderr)
        for command in [
            "head -n 50 chips/esp32s31/pac/src/generated.rs",
            "cd x && cat chips/esp32s31/vendor/facts/provenance.toml",
            "git show HEAD:chips/esp32s31/pac/src/generated.rs",
            "cat chips/esp32s31/vendor/facts/idf-function-signatures.toml",
            "cat chips/esp32s31/vendor/phy-baselines/4d59230d/wifi-cold.json",
        ]:
            status, stderr = run("Bash", command=command)
            self.assertEqual(status, 2, command)
            self.assertIn("grep it with an explicit path", stderr)

    def test_run_rules_are_gone(self):
        # Builds in the foreground, waiting by name and other commands are
        # not this guard's business.
        for command in [
            "cargo build -p oer-xtask",
            "pkill -f cargo",
            "sleep 30",
            "grep -n x chips/esp32s31/pac/src/generated.rs",
            "cat chips/esp32s31/layout/linker/dma-window.x",
        ]:
            self.assertEqual(run("Bash", command=command), (0, ""), command)


if __name__ == "__main__":
    unittest.main()
