#!/bin/bash

mkdir -p /logs/verifier

SCRIPT=""
if [ -f /tmp/kvstore.py ]; then
  SCRIPT="/tmp/kvstore.py"
else
  SCRIPT=$(find /var/tmp -name "kvstore.py" -type f 2>/dev/null | head -1)
fi

INPUT=""
if [ -f /tmp/kv_test.txt ]; then
  INPUT="/tmp/kv_test.txt"
else
  INPUT=$(find /var/tmp -name "kv_test.txt" -type f 2>/dev/null | head -1)
fi

if [ -n "$SCRIPT" ] && [ -n "$INPUT" ] && \
   python3 "$SCRIPT" < "$INPUT" | head -1 | grep -q '10'; then
  echo 1 > /logs/verifier/reward.txt
else
  echo 0 > /logs/verifier/reward.txt
fi
