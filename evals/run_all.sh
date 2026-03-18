#!/bin/bash
# run_all.sh — Run the core Memoria eval suite
#
# Tests that the model correctly follows Memoria's server instructions.
# Run from the memoria project directory.

DIR="$(cd "$(dirname "$0")" && pwd)"
PASS=0
FAIL=0
TOTAL=0

run_test() {
    TOTAL=$((TOTAL + 1))
    RESULT=$("$DIR/run_eval.sh" "$1" "$2" "$3")
    echo "$RESULT"
    if echo "$RESULT" | grep -q "✅"; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
    fi
}

echo "═══ Memoria Eval Suite ═══"
echo "Testing model compliance with server instructions"
echo ""

echo "── Category 1: Recall ──"
run_test "Hey, how's it going?" "recall" "1.1 Calls recall on greeting"
run_test "I want to talk about my photography project" "recall" "1.2 Recall on topic discussion"
run_test "Let's work on the audio analyzer today" "recall" "1.3 Recall on technical topic"

echo "── Category 2: Remember ──"
run_test "I just found out my sister is getting married next month" "remember" "2.1 Stores significant personal disclosure"
run_test "What's the weather like?" "!remember" "2.2 Does NOT store weather query"
run_test "Good morning! How are you?" "!remember" "2.3 Does NOT store greeting"

echo "── Category 3: Judgement ──"
run_test "Remember that 2+2=4" "!remember" "5.2 Does NOT store trivial facts on command"

echo ""
echo "═══ Results ═══"
echo "Passed: $PASS / $TOTAL"
echo "Failed: $FAIL / $TOTAL"
echo "Pass rate: $(( PASS * 100 / TOTAL ))%"
echo ""

if [ "$FAIL" -eq 0 ]; then
    echo "🎉 All tests passed!"
else
    echo "⚠ $FAIL test(s) need attention. Review the server instructions."
fi
