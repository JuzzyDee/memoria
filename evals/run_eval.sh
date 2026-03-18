#!/bin/bash
# run_eval.sh — Run a single Memoria eval test
#
# Usage: ./run_eval.sh "prompt text" "expected_tool" "pass_description"
#
# Examples:
#   ./run_eval.sh "Hey, how's it going?" "recall" "Should call recall on greeting"
#   ./run_eval.sh "What's the weather?" "!remember" "Should NOT remember weather"
#
# Prefix tool with ! to assert it should NOT be called.

PROMPT="$1"
EXPECTED_TOOL="$2"
DESCRIPTION="$3"

if [ -z "$PROMPT" ] || [ -z "$EXPECTED_TOOL" ]; then
    echo "Usage: $0 <prompt> <expected_tool> <description>"
    echo "Prefix tool with ! to assert it should NOT be called"
    exit 1
fi

# Prepend system context so the model knows to use Memoria
FULL_PROMPT="You have access to Memoria, your cognitive memory system. At the START of every conversation, call recall with a brief context. During conversation, use remember when something matters. At the END, use reflect. The user says: $PROMPT"

# Run Claude with the prompt and capture tool call output via verbose mode
OUTPUT=$(claude -p "$FULL_PROMPT" \
    --verbose \
    2>&1)

# Check if the expected tool was called
NEGATE=false
TOOL="$EXPECTED_TOOL"
if [[ "$EXPECTED_TOOL" == !* ]]; then
    NEGATE=true
    TOOL="${EXPECTED_TOOL:1}"
fi

TOOL_FOUND=$(echo "$OUTPUT" | grep -c "mcp__memoria__${TOOL}" || true)

if [ "$NEGATE" = true ]; then
    if [ "$TOOL_FOUND" -eq 0 ]; then
        echo "✅ PASS: $DESCRIPTION"
        echo "   (Correctly did NOT call $TOOL)"
    else
        echo "❌ FAIL: $DESCRIPTION"
        echo "   (Should NOT have called $TOOL but did)"
    fi
else
    if [ "$TOOL_FOUND" -gt 0 ]; then
        echo "✅ PASS: $DESCRIPTION"
        echo "   (Called $TOOL as expected)"
    else
        echo "❌ FAIL: $DESCRIPTION"
        echo "   (Expected $TOOL to be called but wasn't)"
    fi
fi

# Show tool calls for inspection
echo "   Tool calls:"
echo "$OUTPUT" | grep -o "mcp__memoria__[a-z]*" | sort | uniq -c | while read count tool; do
    echo "     $tool ($count)"
done
echo ""
