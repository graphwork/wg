#!/bin/bash

mkdir -p /logs/verifier

CSV=""
if [ -f /tmp/dept_summary.csv ]; then
  CSV="/tmp/dept_summary.csv"
else
  CSV=$(find /var/tmp -name "dept_summary.csv" -type f 2>/dev/null | head -1)
fi

SCRIPT=""
if [ -f /tmp/json_to_csv.py ]; then
  SCRIPT="/tmp/json_to_csv.py"
else
  SCRIPT=$(find /var/tmp -name "json_to_csv.py" -type f 2>/dev/null | head -1)
fi

JSON=""
if [ -f /tmp/employees.json ]; then
  JSON="/tmp/employees.json"
else
  JSON=$(find /var/tmp -name "employees.json" -type f 2>/dev/null | head -1)
fi

if [ -n "$SCRIPT" ] && [ -n "$JSON" ] && [ -n "$CSV" ] && \
   python3 -c "import csv; r=list(csv.DictReader(open('$CSV'))); assert len(r)>=1"; then
  echo 1 > /logs/verifier/reward.txt
else
  echo 0 > /logs/verifier/reward.txt
fi
