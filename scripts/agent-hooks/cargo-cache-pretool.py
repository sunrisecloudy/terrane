#!/usr/bin/env python3
"""Rewrite agent Bash cargo builds/tests to use Terrane's shared cache env."""

from __future__ import annotations

import json
import re
import shlex
import sys
from pathlib import PurePosixPath
from typing import Any


BUILD_SUBCOMMANDS = {
    "bench",
    "build",
    "check",
    "clippy",
    "doc",
    "install",
    "llvm-cov",
    "nextest",
    "run",
    "test",
}

GLOBAL_OPTIONS_WITH_VALUE = {
    "--color",
    "--config",
    "--manifest-path",
    "--message-format",
    "--target-dir",
    "-C",
    "-Z",
}

CACHE_MARKERS = (
    "cargo-cache-env.sh",
    "with-cargo-cache.sh",
    "CARGO_TARGET_DIR",
    "RUSTC_WRAPPER",
)


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except json.JSONDecodeError:
        return 0

    tool_input = find_tool_input(payload)
    if not isinstance(tool_input, dict):
        return 0

    command = tool_input.get("command")
    if not isinstance(command, str):
        return 0

    if not needs_cache(command) or already_uses_cache(command):
        return 0

    updated = dict(tool_input)
    updated["command"] = wrap_command(command)

    json.dump(
        {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": (
                    "Terrane Cargo command rewritten to use the shared "
                    "CARGO_TARGET_DIR and sccache."
                ),
                "updatedInput": updated,
            }
        },
        sys.stdout,
    )
    return 0


def find_tool_input(payload: dict[str, Any]) -> Any:
    for key in ("tool_input", "toolInput"):
        if key in payload:
            return payload[key]

    # A small compatibility net for hook payloads that wrap command args.
    tool_call = payload.get("tool_call")
    if isinstance(tool_call, dict):
        args = tool_call.get("arguments")
        if isinstance(args, dict):
            return args

    return None


def already_uses_cache(command: str) -> bool:
    return any(marker in command for marker in CACHE_MARKERS)


def needs_cache(command: str) -> bool:
    for segment in command_segments(command):
        tokens = shell_tokens(segment)
        if not tokens:
            continue
        if cargo_subcommand(tokens) in BUILD_SUBCOMMANDS:
            return True
    return False


def command_segments(command: str) -> list[str]:
    return [
        part.strip()
        for part in re.split(r"(?:&&|\|\||[;|\n])", command)
        if part.strip()
    ]


def shell_tokens(segment: str) -> list[str]:
    segment = segment.strip().strip("()")
    if not segment:
        return []
    try:
        return [token.strip("()") for token in shlex.split(segment)]
    except ValueError:
        return []


def cargo_subcommand(tokens: list[str]) -> str | None:
    index = 0

    if tokens and tokens[0] == "env":
        index = 1
        while index < len(tokens) and is_env_assignment(tokens[index]):
            index += 1

    while index < len(tokens) and is_env_assignment(tokens[index]):
        index += 1

    if index < len(tokens) and tokens[index] in {"time", "command"}:
        index += 1

    if index >= len(tokens) or PurePosixPath(tokens[index]).name != "cargo":
        return None
    index += 1

    if index < len(tokens) and tokens[index].startswith("+"):
        index += 1

    while index < len(tokens) and tokens[index].startswith("-"):
        option = tokens[index]
        index += 1
        if option in GLOBAL_OPTIONS_WITH_VALUE and index < len(tokens):
            index += 1

    if index >= len(tokens):
        return None
    return tokens[index]


def is_env_assignment(token: str) -> bool:
    if "=" not in token or token.startswith("-"):
        return False
    name = token.split("=", 1)[0]
    return bool(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name))


def wrap_command(command: str) -> str:
    return (
        'ROOT="$(git rev-parse --show-toplevel)" && '
        '. "$ROOT/scripts/cargo-cache-env.sh" && '
        f"( {command} )"
    )


if __name__ == "__main__":
    raise SystemExit(main())
