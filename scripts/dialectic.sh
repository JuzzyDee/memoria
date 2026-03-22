#!/bin/bash
# Ensure homebrew and system paths are available (launchd has minimal PATH)
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

# dialectic.sh — Run the dialectic subconscious processing layer
#
# Evolution of think.sh. Instead of a single voice finding patterns,
# this spawns an advocate and challenger to argue about consolidation
# candidates before acting on them. The friction is the feature.
#
# Requires: CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 in settings or env
#
# Usage:
#   ./scripts/dialectic.sh              # Run dialectic processing
#   ./scripts/dialectic.sh --sonnet     # Use sonnet model
#   ./scripts/dialectic.sh --verbose    # With verbose output

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROMPT_FILE="$SCRIPT_DIR/dialectic.md"

if [ ! -f "$PROMPT_FILE" ]; then
    echo "Error: dialectic.md not found at $PROMPT_FILE"
    exit 1
fi

# Enable agent teams
export CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1

echo "═══ Memoria Dialectic Subconscious ═══"
echo "Time: $(date)"
echo "Thinking about thinking... adversarially."
echo ""

PROMPT=$(cat "$PROMPT_FILE")

# Model: default to opus for quality, use --model flag to override
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

claude -p "$PROMPT" $MODEL_FLAG $VERBOSE_FLAG --allowedTools "mcp__memoria__recall,mcp__memoria__remember,mcp__memoria__reflect,mcp__memoria__reframe,mcp__memoria__forget,mcp__memoria__review"

echo ""
echo "═══ Dialectic processing complete ═══"
