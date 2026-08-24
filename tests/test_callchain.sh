#!/bin/bash

# Test script to demonstrate call chain functionality
# This script indexes the test file and queries call relationships
#
# Run from the repository root: tests/test_callchain.sh

set -e

cd "$(dirname "$0")"

# Prefer the symlinks build.sh makes, fall back to a plain cargo build so the
# script works in a tree where build.sh has not been run.
if [ -x ../bin/semcode-index ]; then
	SEMCODE_INDEX=../bin/semcode-index
	SEMCODE=../bin/semcode
elif [ -x ../target/release/semcode-index ]; then
	SEMCODE_INDEX=../target/release/semcode-index
	SEMCODE=../target/release/semcode
else
	echo "Build first: cargo build --release" >&2
	exit 1
fi

echo "=== Call Chain Test ==="
echo "Testing with simple C file: test_callchain.c"
echo

# Indexing walks the whole git tree the source lives in, so the fixtures are
# copied into a repository of their own. Indexing them in place would index
# all of semcode.
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cp test_callchain.c test_header.h "$WORK/"
git -C "$WORK" init -q
git -C "$WORK" add .
git -C "$WORK" -c user.email=test@example.com -c user.name=Test commit -q -m fixture

echo "1. Indexing test file..."
"$SEMCODE_INDEX" --source "$WORK" --database "$WORK/db" --extensions c

echo
echo "2. Testing function queries..."
echo

# Test individual function lookups
echo "--- Looking up main function ---"
echo "func main" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "--- Looking up add_numbers function ---"
echo "func add_numbers" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "3. Testing call chain queries..."
echo

# Test callers (who calls this function)
echo "--- Who calls add_numbers? (should be: calculate_sum, process_math) ---"
echo "callers add_numbers" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "--- Who calls print_result? (should be: calculate_product, process_math, main) ---"  
echo "callers print_result" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "--- Who calls calculate_sum? (should be: process_math, main) ---"
echo "callers calculate_sum" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "4. Testing callees (who this function calls)..."
echo

# Test callees (who this function calls)
echo "--- What does main call? (should be: process_math, calculate_sum, print_result) ---"
echo "callees main" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "--- What does process_math call? (should be: calculate_sum, calculate_product, print_result, add_numbers) ---"
echo "callees process_math" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "--- What does calculate_product call? (should be: multiply_numbers, print_result) ---"
echo "callees calculate_product" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "5. Testing full call chain..."
echo

echo "--- Call chain for add_numbers ---"
echo "callchain add_numbers" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "--- Call chain for print_result ---"
echo "callchain print_result" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "6. Dumping functions to inspect call data..."
echo "dump-functions $WORK/test_functions.json" | "$SEMCODE" --database "$WORK/db" --git-repo "$WORK"

echo
echo "=== Test Complete ==="
echo "Function dump written under the temporary tree, removed on exit"
echo "Expected vs actual call relationships are documented in test_callchain.c"