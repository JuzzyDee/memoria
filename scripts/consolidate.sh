#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROMPT=$(cat "$SCRIPT_DIR/consolidate.md")
/opt/homebrew/bin/claude -p "$PROMPT" --model sonnet --allowedTools "mcp__memoria__recall,mcp__memoria__remember,mcp__memoria__reflect,mcp__memoria__reframe,mcp__memoria__forget,mcp__memoria__review"
