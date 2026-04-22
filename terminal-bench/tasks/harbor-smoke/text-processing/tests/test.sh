#!/bin/bash

mkdir -p /logs/verifier

SCRIPT=""
# Check /tmp first, then search trial workdirs
if [ -f /tmp/wordfreq.py ]; then
  SCRIPT="/tmp/wordfreq.py"
else
  SCRIPT=$(find /var/tmp -name "wordfreq.py" -type f 2>/dev/null | head -1)
fi

if [ -n "$SCRIPT" ] && echo 'the the the dog dog cat' | python3 "$SCRIPT" | head -1 | grep -q 'the'; then
  echo 1 > /logs/verifier/reward.txt
else
  echo 0 > /logs/verifier/reward.txt
fi
