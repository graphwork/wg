#!/bin/bash

mkdir -p /logs/verifier

SCRIPT=""
if [ -f /tmp/buggy_sort.py ]; then
  SCRIPT="/tmp/buggy_sort.py"
else
  SCRIPT=$(find /var/tmp -name "buggy_sort.py" -type f 2>/dev/null | head -1)
fi

if [ -n "$SCRIPT" ] && \
   python3 "$SCRIPT" 2>&1 | grep -v FAIL | grep -c PASS | \
   python3 -c "import sys; n=int(sys.stdin.read().strip()); sys.exit(0 if n>=6 else 1)"; then
  echo 1 > /logs/verifier/reward.txt
else
  echo 0 > /logs/verifier/reward.txt
fi
