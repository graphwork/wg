Implement a simple in-memory key-value store in Python at /tmp/kvstore.py with the following features:

1. Commands (read from stdin, one per line):
   - SET key value — store key-value pair
   - GET key — print the value, or "NULL" if not found
   - DEL key — delete the key, print "1" if existed, "0" if not
   - COUNT — print number of keys
   - BEGIN — start a transaction
   - COMMIT — commit the current transaction
   - ROLLBACK — rollback the current transaction, print "NO TRANSACTION" if none active
   - END — exit the program

2. Transactions must support nesting (BEGIN inside BEGIN).
3. ROLLBACK only undoes the most recent BEGIN.
4. After COMMIT, all changes are permanent (all transaction levels committed).

Test with this input sequence:
```
SET a 10
GET a
BEGIN
SET a 20
GET a
BEGIN
SET a 30
GET a
ROLLBACK
GET a
COMMIT
GET a
BEGIN
SET b 100
GET b
ROLLBACK
GET b
COUNT
END
```

Expected output:
10
20
30
20
20
100
NULL
1

Create the test input at /tmp/kv_test.txt and run: python3 /tmp/kvstore.py < /tmp/kv_test.txt
Verify the output matches expected.
