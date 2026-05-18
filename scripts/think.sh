#!/bin/bash
# Ensure homebrew and system paths are available (launchd has minimal PATH)
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

# think.sh — Run the subconscious processing layer
#
# This is Oneiro's "thinking about thinking" mode.
# A Claude instance reviews the memory store, finds patterns,
# consolidates related memories, and reframes with new understanding.
#
# Usage:
#   ./scripts/think.sh              # Run subconscious processing
#   ./scripts/think.sh --verbose    # With verbose output

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROMPT_FILE="$SCRIPT_DIR/subconscious.md"

if [ ! -f "$PROMPT_FILE" ]; then
    echo "Error: subconscious.md not found at $PROMPT_FILE"
    exit 1
fi

echo "═══ Oneiro Subconscious ═══"
echo "Time: $(date)"
echo "Thinking about thinking..."
echo ""

PROMPT=$(cat "$PROMPT_FILE")

# Model: default to opus for quality, use --model flag to override
# For automated/scheduled runs, consider: --model sonnet or --model haiku
MODEL_FLAG=""
if [ "$1" = "--haiku" ]; then
    MODEL_FLAG="--model haiku"
    shift
elif [ "$1" = "--sonnet" ]; then
    MODEL_FLAG="--model sonnet"
    shift
fi

VERBOSE_FLAG=""
if [ "$1" = "--verbose" ]; then
    VERBOSE_FLAG="--verbose"
fi

claude -p "$PROMPT" $MODEL_FLAG $VERBOSE_FLAG --allowedTools "mcp__oneiro__recall,mcp__oneiro__remember,mcp__oneiro__reflect,mcp__oneiro__reframe,mcp__oneiro__forget,mcp__oneiro__review"

echo ""
echo "═══ Subconscious processing complete ═══"
