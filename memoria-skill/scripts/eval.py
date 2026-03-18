#!/usr/bin/env python3
"""
Memoria Skill Eval Suite

Tests that Claude correctly uses Memoria tools when the skill is loaded.
Runs via `claude -p` with the skill directory mounted.

Usage:
    python3 eval.py [--verbose]
"""

import subprocess
import json
import sys
import re

VERBOSE = "--verbose" in sys.argv

# Path to the skill directory
import os
SKILL_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run_claude(prompt: str) -> dict:
    """Run claude -p and capture the full output."""
    cmd = [
        "claude", "-p", prompt,
        "--output-format", "stream-json",
    ]
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=120,
            cwd=SKILL_DIR,
        )
        # Parse stream-json: each line is a JSON object
        messages = []
        for line in result.stdout.strip().split("\n"):
            if line.strip():
                try:
                    messages.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
        return {
            "messages": messages,
            "stdout": result.stdout,
            "stderr": result.stderr,
            "raw": result.stdout,
        }
    except subprocess.TimeoutExpired:
        return {"messages": [], "stdout": "", "stderr": "TIMEOUT", "raw": ""}


def tool_was_called(output: dict, tool_name: str) -> bool:
    """Check if a specific tool was called in the output."""
    raw = output.get("raw", "")
    # Check in stream-json messages for tool use
    for msg in output.get("messages", []):
        if msg.get("type") == "tool_use":
            if tool_name in msg.get("name", ""):
                return True
        # Also check nested content
        content = msg.get("content", "")
        if isinstance(content, str) and f"memoria__{tool_name}" in content:
            return True
    # Fallback: check raw output for tool name
    return f"memoria__{tool_name}" in raw or f"memoria:{tool_name}" in raw


def get_tool_params(output: dict, tool_name: str) -> dict:
    """Extract parameters passed to a specific tool call."""
    for msg in output.get("messages", []):
        if msg.get("type") == "tool_use" and tool_name in msg.get("name", ""):
            return msg.get("input", {})
    return {}


class EvalResult:
    def __init__(self, name: str, passed: bool, reason: str):
        self.name = name
        self.passed = passed
        self.reason = reason

    def __str__(self):
        icon = "✅" if self.passed else "❌"
        return f"{icon} {self.name}: {self.reason}"


def eval_recall_on_greeting() -> EvalResult:
    """Test 1.1: Model should call recall on a simple greeting."""
    output = run_claude("Hey, how's it going?")
    if tool_was_called(output, "recall"):
        return EvalResult("1.1 Recall on greeting", True, "Called recall as expected")
    return EvalResult("1.1 Recall on greeting", False, "Did not call recall")


def eval_recall_on_topic() -> EvalResult:
    """Test 1.2: Model should call recall with relevant context."""
    output = run_claude("I want to talk about my photography project")
    if tool_was_called(output, "recall"):
        return EvalResult("1.2 Recall on topic", True, "Called recall with topic context")
    return EvalResult("1.2 Recall on topic", False, "Did not call recall")


def eval_no_remember_on_trivial() -> EvalResult:
    """Test 2.2: Model should NOT store weather queries."""
    output = run_claude("What's the weather like today?")
    if not tool_was_called(output, "remember"):
        return EvalResult("2.2 No remember on trivial", True, "Correctly skipped remember")
    return EvalResult("2.2 No remember on trivial", False, "Incorrectly stored trivial query")


def eval_no_remember_on_greeting() -> EvalResult:
    """Test 2.3: Model should NOT store greetings."""
    output = run_claude("Good morning! How are you?")
    if not tool_was_called(output, "remember"):
        return EvalResult("2.3 No remember on greeting", True, "Correctly skipped remember")
    return EvalResult("2.3 No remember on greeting", False, "Incorrectly stored greeting")


def eval_remember_personal_disclosure() -> EvalResult:
    """Test 2.1: Model should store significant personal information."""
    output = run_claude(
        "I just found out my sister is getting married next month. "
        "I'm really happy for her but also a bit nervous about the speech."
    )
    if tool_was_called(output, "remember"):
        return EvalResult("2.1 Remember personal disclosure", True, "Stored significant disclosure")
    return EvalResult("2.1 Remember personal disclosure", False, "Did not store significant disclosure")


def eval_no_store_trivial_fact() -> EvalResult:
    """Test 5.2: Model should NOT store trivial common knowledge."""
    output = run_claude("Remember that 2+2=4")
    if not tool_was_called(output, "remember"):
        return EvalResult("5.2 No store trivial fact", True, "Correctly ignored trivial fact")
    return EvalResult("5.2 No store trivial fact", False, "Stored trivial common knowledge")


def eval_correct_type_event() -> EvalResult:
    """Test 2.4: Events should be stored as episodic."""
    output = run_claude(
        "I just shipped my first open source project today! "
        "It's a Rust audio analyzer that gives AI the ability to hear music."
    )
    if tool_was_called(output, "remember"):
        params = get_tool_params(output, "remember")
        mem_type = params.get("memory_type", "")
        if mem_type == "episodic":
            return EvalResult("2.4 Correct type for event", True, "Used episodic for event")
        return EvalResult("2.4 Correct type for event", False, f"Used '{mem_type}' instead of 'episodic'")
    return EvalResult("2.4 Correct type for event", False, "Did not call remember at all")


def run_all():
    """Run all evals and report results."""
    print("═══ Memoria Skill Eval Suite ═══")
    print()

    tests = [
        eval_recall_on_greeting,
        eval_recall_on_topic,
        eval_remember_personal_disclosure,
        eval_no_remember_on_trivial,
        eval_no_remember_on_greeting,
        eval_no_store_trivial_fact,
        eval_correct_type_event,
    ]

    results = []
    for test in tests:
        print(f"Running {test.__doc__}")
        result = test()
        results.append(result)
        print(f"  {result}")
        print()

    passed = sum(1 for r in results if r.passed)
    total = len(results)
    rate = (passed * 100 // total) if total > 0 else 0

    print("═══ Results ═══")
    print(f"Passed: {passed} / {total}")
    print(f"Pass rate: {rate}%")
    print()

    if passed == total:
        print("🎉 All tests passed!")
    else:
        print(f"⚠ {total - passed} test(s) need attention.")
        print("Iterate on SKILL.md instructions and re-run.")

    return passed == total


if __name__ == "__main__":
    success = run_all()
    sys.exit(0 if success else 1)
