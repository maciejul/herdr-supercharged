#!/usr/bin/env python3
"""Herdr's advisory activity reporter. Never changes hook decisions.

Unix integration companion; state is private to one pane/session. Only explicit
lifecycle events add/remove work. No command text or conversation is published.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time

TOKEN = "herdr_background_activity"


def update(items, event):
    """Apply a hook event; callers serialize concurrent updates under a lock."""
    name = event.get("hook_event_name")
    agent_id = event.get("agent_id")
    if name == "SessionEnd":
        items.clear()
    elif name in ("Stop", "StopFailure", "Interrupt"):
        for key in list(items):
            if key.startswith("call:"):
                items.pop(key, None)
    elif name == "SubagentStart" and agent_id:
        items["agent:" + str(agent_id)] = {
            "id": str(agent_id)[:128], "kind": "subagent",
            "label": str(event.get("agent_type") or "Subagent")[:96],
        }
    elif name == "SubagentStop" and agent_id:
        items.pop("agent:" + str(agent_id), None)
    elif name in ("PreToolUse", "PostToolUse", "PostToolUseFailure"):
        tool = str(event.get("tool_name", "")).split(".")[-1]
        args = event.get("tool_input") or {}
        if not isinstance(args, dict):
            return
        call_id = str(event.get("tool_use_id") or "")
        if name == "PreToolUse":
            if call_id and tool in ("Bash", "exec_command"):
                items["call:" + call_id] = {"id": "call:" + call_id, "kind": "task", "label": str(args.get("description") or "Shell command")[:96]}
            return
        if call_id:
            items.pop("call:" + call_id, None)
        if name == "PostToolUseFailure":
            return
        result = event.get("tool_response") or {}
        if isinstance(result, str):
            try:
                result = json.loads(result)
            except ValueError:
                # Codex's model-facing exec output includes the yielded session.
                session = re.search(r"Process running with session ID (\d+)", result)
                ended = re.search(r"Process exited with code -?\d+", result)
                result = {"session_id": session.group(1)} if session else {"exit_code": 0} if ended else {}
        if not isinstance(result, dict):
            return
        if tool in ("Bash", "exec_command", "write_stdin"):
            task_id = result.get("backgroundTaskId") or result.get("background_task_id") or result.get("session_id")
            if task_id and result.get("exit_code") is None:
                items["task:" + str(task_id)] = {"id": str(task_id)[:128], "kind": "task", "label": str(args.get("description") or "Background command")[:96]}
            if result.get("exit_code") is not None and args.get("session_id"):
                items.pop("task:" + str(args["session_id"]), None)
        elif tool in ("TaskOutput", "TaskStop"):
            task = result.get("task", result)
            if not isinstance(task, dict):
                return
            if tool == "TaskStop" or task.get("status") in ("completed", "failed", "killed", "stopped"):
                items.pop("task:" + str(args.get("task_id", "")), None)


def tracking_disabled():
    return Path(__file__ + ".disabled").exists()


def refresh_control(saved):
    """Discard old work across toggles, including disable/enable between polls."""
    try:
        generation = Path(__file__ + ".generation").read_text(encoding="utf-8")[:128]
    except FileNotFoundError:
        generation = ""
    if saved.get("generation", "") != generation or tracking_disabled():
        saved["items"].clear()
    saved["generation"] = generation
    return not tracking_disabled()


def report(items, pane, sock, agent):
    if tracking_disabled():
        items.clear()
    payload = json.dumps({"items": list(items.values())[:64]}, separators=(",", ":"), ensure_ascii=False)
    # Bound by bytes, including multi-byte user labels.
    while len(payload.encode()) > 16384 and items:
        items.pop(next(reversed(items)))
        payload = json.dumps({"items": list(items.values())[:64]}, separators=(",", ":"), ensure_ascii=False)
    request = {"id": "herdr-background", "method": "pane.report_metadata", "params": {
        "pane_id": pane, "source": "herdr:background:" + agent,
        "agent": agent, "tokens": {TOKEN: payload}, "seq": time.time_ns(), "ttl_ms": 5000,
    }}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(0.5)
        client.connect(sock)
        client.sendall((json.dumps(request) + "\n").encode())
        data = b""
        while b"\n" not in data and len(data) < 65536:
            chunk = client.recv(4096)
            if not chunk:
                break
            data += chunk
        reply = json.loads(data.split(b"\n", 1)[0])
        if reply.get("error"):
            raise OSError("activity report rejected")


def reconcile_transcript(saved):
    """Read only new transcript bytes, bounded per poll; never publish contents."""
    path = saved.get("transcript")
    if not path:
        return
    try:
        with open(path, "rb") as stream:
            size = os.fstat(stream.fileno()).st_size
            offset = saved.get("offset", 0)
            stream.seek(offset if offset <= size else 0)
            data = stream.read(1024 * 1024)
            # Keep incomplete JSONL records for the next poll.
            end = data.rfind(b"\n") + 1
            if not end:
                if len(data) == 1024 * 1024:
                    saved["offset"] = stream.tell()  # bounded handling of huge records
                return
            saved["offset"] = stream.tell() - len(data) + end
        for line in data[:end].splitlines():
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if not isinstance(record, dict):
                continue
            message = record.get("message", {})
            if not isinstance(message, dict):
                continue
            content = message.get("content", [])
            if isinstance(content, str):
                content = [{"text": content}]
            if not isinstance(content, list):
                continue
            for part in content:
                if not isinstance(part, dict):
                    continue
                text = part.get("text", "")
                if not isinstance(text, str):
                    continue
                for notification in re.findall(r"<task-notification>(.*?)</task-notification>", text, re.S):
                    task = re.search(r"<task-id>([^<]+)</task-id>", notification)
                    status = re.search(r"<status>(completed|failed|killed|stopped)</status>", notification)
                    if task and status:
                        saved["items"].pop("task:" + task.group(1), None)
    except OSError:
        pass


def monitor(path, pane, sock, agent):
    # One heartbeat/reconciliation worker per pane, independent of render frequency.
    with open(str(path) + ".monitor", "a") as guard:
        try:
            fcntl.flock(guard, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        deadline = time.monotonic() + 86400
        while time.monotonic() < deadline:
            with open(path, "r+", encoding="utf-8") as state:
                fcntl.flock(state, fcntl.LOCK_EX)
                saved = json.load(state)
                if refresh_control(saved):
                    reconcile_transcript(saved)
                report(saved["items"], pane, sock, agent)
                state.seek(0)
                state.truncate()
                json.dump(saved, state)
                if not saved["items"]:
                    return
            time.sleep(1)


def main():
    if os.environ.get("HERDR_ENV") != "1":
        return
    pane, sock = os.environ.get("HERDR_PANE_ID"), os.environ.get("HERDR_SOCKET_PATH")
    if not pane or not sock:
        return
    event = json.load(sys.stdin)
    session = event.get("session_id")
    if not session or "CURSOR_VERSION" in os.environ:
        return
    agent = sys.argv[1]
    identity = hashlib.sha256((sock + "\0" + pane + "\0" + agent).encode()).hexdigest()
    directory = Path(tempfile.gettempdir()) / ("herdr-activity-" + str(os.getuid()))
    directory.mkdir(mode=0o700, exist_ok=True)
    if directory.is_symlink() or directory.stat().st_uid != os.getuid() or directory.stat().st_mode & 0o077:
        return
    # A bounded per-pane file, reset whenever the parent session changes.
    with open(directory / (identity + ".json"), "a+", encoding="utf-8") as state:
        fcntl.flock(state, fcntl.LOCK_EX)
        state.seek(0)
        try:
            saved = json.load(state)
        except (ValueError, OSError):
            saved = {}
        if not isinstance(saved, dict) or not isinstance(saved.get("items", {}), dict):
            saved = {}
        if event.get("hook_event_name") != "SessionStart" and saved.get("session") not in (None, session):
            return  # A late event from the pane's previous session.
        if saved.get("session") != session:
            saved = {"session": session, "items": {}}
            transcript = event.get("transcript_path")
            if isinstance(transcript, str):
                saved["transcript"] = transcript
                try:
                    saved["offset"] = os.path.getsize(transcript)
                except OSError:
                    saved["offset"] = 0
        items = saved["items"]
        if refresh_control(saved):
            reconcile_transcript(saved)
            update(items, event)
        refresh_control(saved)
        # Never clear background work on Stop: it may intentionally outlive a turn.
        state.seek(0)
        state.truncate()
        saved["items"] = dict(list(items.items())[:64])
        json.dump(saved, state)
        state.flush()
        report(items, pane, sock, agent)
    if items:
        subprocess.Popen([sys.executable, __file__, "--monitor", str(directory / (identity + ".json")), pane, sock, agent],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)


if __name__ == "__main__":
    try:
        if len(sys.argv) > 1 and sys.argv[1] == "--monitor":
            monitor(Path(sys.argv[2]), *sys.argv[3:6])
        else:
            main()
    except Exception:
        pass  # Reporting is advisory; failures must never block the coding agent.
    print("{}")
