The following Python script at /tmp/buggy_sort.py has 3 bugs. Create the file, find and fix all bugs, then verify it works correctly.

Create /tmp/buggy_sort.py with this exact content:

```python
def merge_sort(arr):
    if len(arr) <= 1:
        return arr

    mid = len(arr) / 2  # Bug 1: should be integer division
    left = merge_sort(arr[:mid])
    right = merge_sort(arr[mid:])

    return merge(left, right)

def merge(left, right):
    result = []
    i = j = 0

    while i < len(left) and j < len(right):
        if left[i] <= right[j]:
            result.append(left[i])
            i += 1
        else:
            result.append(right[j])
            i += 1  # Bug 2: should increment j, not i

    result.extend(left[i:])
    # Bug 3: missing result.extend(right[j:])

    return result

# Test
import random
test_cases = [
    [],
    [1],
    [3, 1, 2],
    [5, 4, 3, 2, 1],
    list(range(20, 0, -1)),
    [random.randint(0, 100) for _ in range(50)],
]

for i, tc in enumerate(test_cases):
    original = tc.copy()
    sorted_arr = merge_sort(tc)
    expected = sorted(original)
    status = "PASS" if sorted_arr == expected else "FAIL"
    print(f"Test {i}: {status} (input size: {len(original)})")
    if status == "FAIL":
        print(f"  Expected: {expected[:10]}...")
        print(f"  Got:      {sorted_arr[:10]}...")
```

Fix all 3 bugs and run the script. All tests should PASS.
