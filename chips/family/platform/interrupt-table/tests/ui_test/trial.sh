#!/bin/sh
# Trial (tasks/h003): build the scratch workspace that holds `ui_test` under
# the checkout's `target/` and run the converted cases there.
#
#   trial.sh            check the cases against their snapshots
#   trial.sh --bless    write the snapshots
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../../../.." && pwd)
scratch="$root/target/ui-test-trial"
mkdir -p "$scratch/deps"
cp "$here/scratch/Cargo.toml.in" "$scratch/Cargo.toml"
cp "$here/scratch/deps.Cargo.toml.in" "$scratch/deps/Cargo.toml"
cp "$here/scratch/runner.rs" "$scratch/runner.rs"
cp "$here/scratch/Cargo.lock.in" "$scratch/Cargo.lock"
: > "$scratch/deps/lib.rs"
cd "$scratch"
UI_TEST_CASES="${UI_TEST_CASES:-$here/cases}" exec cargo test --locked --test ui_test -- "$@"
