Write a Python script at /tmp/wordfreq.py that:
1. Reads text from stdin
2. Converts to lowercase and splits on whitespace
3. Strips punctuation (.,;:!?'"()-) from each word
4. Counts word frequencies
5. Prints the top 10 most frequent words in format: "word: count" (one per line, sorted by count descending, then alphabetically for ties)

Test it with this input:
echo "The quick brown fox jumps over the lazy dog. The dog barked at the fox. The fox ran away from the dog and the cat. The cat sat on the mat. The mat was on the floor. The floor was clean." | python3 /tmp/wordfreq.py

Expected: "the" should be the most frequent word. Verify the output makes sense and the counts are correct.
