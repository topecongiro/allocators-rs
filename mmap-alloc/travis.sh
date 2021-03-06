#!/bin/bash

# Copyright 2017 the authors. See the 'Copyright and license' section of the
# README.md file at the top-level directory of this repository.
#
# Licensed under the Apache License, Version 2.0 (the LICENSE file). This file
# may not be copied, modified, or distributed except according to those terms.

set -x
set -e

cargo build --verbose --all
RUST_BACKTRACE=1 cargo test --verbose --all -- --ignored
for feature in test-no-std; do
  RUST_BACKTRACE=1 cargo test --verbose --all --features "$feature" -- --ignored
done
