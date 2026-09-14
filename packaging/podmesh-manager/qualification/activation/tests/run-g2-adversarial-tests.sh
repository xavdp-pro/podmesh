#!/bin/bash
set -euo pipefail

tests=$(cd -- "$(dirname -- "$0")" && pwd)
python3 -m py_compile "$tests/g2_adversarial_fixture.py" "$tests/test_g2_adversarial.py"
PYTHONPATH="$tests${PYTHONPATH:+:$PYTHONPATH}" \
  python3 -m unittest -v "$tests/test_g2_adversarial.py"
