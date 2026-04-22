Write a Python script at /tmp/json_to_csv.py that:

1. First, create a JSON dataset at /tmp/employees.json containing an array of 15 employee records with fields: id (int), name (string), department (string from: Engineering, Sales, Marketing, HR), salary (int between 50000-150000), start_date (string YYYY-MM-DD between 2020-2025), active (boolean).

2. The script should:
   a. Read /tmp/employees.json
   b. Filter to only active employees
   c. Compute per-department statistics:
      - count of active employees
      - average salary (rounded to nearest integer)
      - min and max salary
      - earliest start_date
   d. Output a CSV to /tmp/dept_summary.csv with columns: department, count, avg_salary, min_salary, max_salary, earliest_start
   e. Sort by department name alphabetically
   f. Print the CSV to stdout as well

3. Verify:
   - python3 -c "import csv; r=list(csv.DictReader(open('/tmp/dept_summary.csv'))); print(f'{len(r)} departments'); [print(row) for row in r]"
   - Check that all departments with active employees are represented
   - Check that averages are reasonable
