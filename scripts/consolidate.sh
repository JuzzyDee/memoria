#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROMPT=$(cat "$SCRIPT_DIR/consolidate.md")
/opt/homebrew/bin/claude -p "$PROMPT" --model sonnet --allowedTools "mcp__oneiro__recall,mcp__oneiro__remember,mcp__oneiro__reflect,mcp__oneiro__reframe,mcp__oneiro__forget,mcp__oneiro__review"
