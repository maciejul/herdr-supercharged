"""Lifecycle and real Unix-socket tests for the advisory hook companion."""
import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest

SCRIPT = Path(__file__).with_name("herdr-background-activity.py")
spec = importlib.util.spec_from_file_location("activity", SCRIPT)
activity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(activity)


class LifecycleTests(unittest.TestCase):
    def test_foreground_command_transitions_to_background_without_double_count(self):
        items = {}
        base = {"tool_use_id": "c1", "tool_name": "Bash", "tool_input": {"description": "Build tests"}}
        activity.update(items, dict(base, hook_event_name="PreToolUse"))
        self.assertEqual(items["call:c1"]["label"], "Build tests")
        activity.update(items, dict(base, hook_event_name="PostToolUse", tool_response={"backgroundTaskId": "b1"}))
        self.assertEqual(set(items), {"task:b1"})
        activity.update(items, dict(base, hook_event_name="PreToolUse"))
        activity.update(items, dict(base, hook_event_name="PostToolUseFailure"))
        self.assertEqual(set(items), {"task:b1"})
        activity.update(items, dict(base, hook_event_name="PreToolUse"))
        activity.update(items, {"hook_event_name": "Interrupt"})
        self.assertEqual(set(items), {"task:b1"})

    def test_agents_and_commands_have_independent_lifetimes(self):
        items = {}
        activity.update(items, {"hook_event_name": "SubagentStart", "agent_id": "a", "agent_type": "reviewer"})
        activity.update(items, {"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {}, "tool_response": {"backgroundTaskId": "b"}})
        activity.update(items, {"hook_event_name": "Stop"})
        self.assertEqual(set(items), {"agent:a", "task:b"})
        activity.update(items, {"hook_event_name": "SubagentStop", "agent_id": "a"})
        self.assertEqual(set(items), {"task:b"})
        activity.update(items, {"hook_event_name": "PostToolUse", "tool_name": "TaskOutput", "tool_input": {"task_id": "b"}, "tool_response": {"task": {"status": "running"}}})
        self.assertIn("task:b", items)
        activity.update(items, {"hook_event_name": "PostToolUse", "tool_name": "TaskOutput", "tool_input": {"task_id": "b"}, "tool_response": {"task": {"status": "completed"}}})
        self.assertEqual(items, {})

    def test_codex_yield_and_completion(self):
        items = {}
        activity.update(items, {"hook_event_name": "PostToolUse", "tool_name": "functions.exec_command", "tool_input": {}, "tool_response": "Process running with session ID 123"})
        self.assertIn("task:123", items)
        activity.update(items, {"hook_event_name": "PostToolUse", "tool_name": "functions.write_stdin", "tool_input": {"session_id": 123}, "tool_response": "Process exited with code 1"})
        self.assertEqual(items, {})

    def test_malformed_tool_payload_does_not_invent_activity(self):
        for value in (None, [], 12, "unstructured output", "[]"):
            items = {}
            activity.update(items, {"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {}, "tool_response": value})
            self.assertEqual(items, {})

    def test_transcript_reconciliation_handles_progress_append_and_partial_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "session.jsonl"
            saved = {"items": {"task:job": {"id": "job", "kind": "task"}}, "transcript": str(path), "offset": 0}
            def record(text):
                return json.dumps({"type": "api-request-blob", "message": {"role": "user", "content": [{"type": "text", "text": text}]}}) + "\n"
            path.write_text(record("<task-notification><task-id>job</task-id><event>progress</event></task-notification>"))
            activity.reconcile_transcript(saved)
            self.assertIn("task:job", saved["items"])
            offset = saved["offset"]
            completed = record("<task-notification><task-id>job</task-id><status>completed</status></task-notification>")
            with path.open("a") as stream:
                stream.write(completed[:20])
            activity.reconcile_transcript(saved)
            self.assertEqual(saved["offset"], offset)
            self.assertIn("task:job", saved["items"])
            with path.open("a") as stream:
                stream.write(completed[20:])
            activity.reconcile_transcript(saved)
            self.assertEqual(saved["items"], {})
            self.assertEqual(saved["offset"], path.stat().st_size)

    def test_transcript_reconciliation_ignores_non_message_json(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "session.jsonl"
            path.write_text('null\n[]\n{"message":{"content":[{"text":null}]}}\n')
            saved = {"items": {}, "transcript": str(path), "offset": 0}
            activity.reconcile_transcript(saved)
            self.assertEqual(saved["items"], {})


class HookSocketTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="herdr-hook-test-")
        self.addCleanup(self.tmp.cleanup)
        self.script = Path(self.tmp.name) / SCRIPT.name
        self.script.write_text(SCRIPT.read_text())
        self.path = str(Path(self.tmp.name) / "api.sock")
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(self.path)
        self.server.listen(64)
        self.server.settimeout(0.05)
        self.stopping = threading.Event()
        self.requests = []
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()
        self.addCleanup(self.shutdown)
        self.env = dict(os.environ, HERDR_ENV="1", HERDR_PANE_ID="test-pane", HERDR_SOCKET_PATH=self.path, TMPDIR=self.tmp.name)
        self.env.pop("CURSOR_VERSION", None)

    def shutdown(self):
        self.stopping.set()
        self.thread.join(timeout=2)
        self.server.close()

    def serve(self):
        while not self.stopping.is_set():
            try:
                client, _ = self.server.accept()
            except socket.timeout:
                continue
            with client:
                line = client.makefile("rb").readline()
                self.requests.append(json.loads(line))
                client.sendall(b'{"id":"herdr-background","result":{}}\n')

    def hook(self, event, **environment):
        result = subprocess.run([sys.executable, str(self.script), "claude"], input=json.dumps(dict(session_id="parent", **event)), text=True, capture_output=True, env=dict(self.env, **environment), timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {})
        return result

    def last_items(self):
        return json.loads(self.requests[-1]["params"]["tokens"][activity.TOKEN])["items"]

    def test_parallel_subagents_transport_full_json_and_stop_individually(self):
        self.hook({"hook_event_name": "SessionStart"})
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(lambda n: self.hook({"hook_event_name": "SubagentStart", "agent_id": str(n), "agent_type": "reviewer"}), range(12)))
        self.assertEqual(len(self.last_items()), 12)
        self.assertGreater(len(self.requests[-1]["params"]["tokens"][activity.TOKEN]), 80)
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(lambda n: self.hook({"hook_event_name": "SubagentStop", "agent_id": str(n)}), range(11)))
        self.assertEqual([item["id"] for item in self.last_items()], ["11"])
        self.hook({"hook_event_name": "SessionEnd"})
        self.assertEqual(self.last_items(), [])

    def test_monitor_clears_completed_background_task_without_another_hook(self):
        transcript = Path(self.tmp.name) / "session.jsonl"
        transcript.write_text("")
        self.hook({"hook_event_name": "SessionStart", "transcript_path": str(transcript)})
        self.hook({"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {}, "tool_response": {"backgroundTaskId": "job"}})
        self.assertEqual([item["id"] for item in self.last_items()], ["job"])
        transcript.write_text(json.dumps({"message": {"content": [{"type": "text", "text": "<task-notification><task-id>job</task-id><status>completed</status></task-notification>"}]}}) + "\n")
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline and self.last_items():
            time.sleep(0.05)
        self.assertEqual(self.last_items(), [])

    def wait_for_empty(self):
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline and self.last_items():
            time.sleep(0.05)
        self.assertEqual(self.last_items(), [])

    def test_disabling_stops_live_monitor_and_old_hooks_cannot_restart_activity(self):
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "old"})
        self.assertEqual([item["id"] for item in self.last_items()], ["old"])
        Path(str(self.script) + ".disabled").write_text("disabled")
        Path(str(self.script) + ".generation").write_text("disable-1")
        self.wait_for_empty()
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "ignored"})
        self.assertEqual(self.last_items(), [])
        # Updating the companion on reinstall must not remove the persistent marker.
        self.script.write_text(SCRIPT.read_text())
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "still-ignored"})
        self.assertEqual(self.last_items(), [])

    def test_fast_disable_reenable_discards_saved_work_but_allows_new_work(self):
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "old"})
        # Simulate both toggles between monitor polls: persistent generation is decisive.
        Path(str(self.script) + ".generation").write_text("enable-after-disable")
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "new"})
        self.assertEqual([item["id"] for item in self.last_items()], ["new"])

    def test_session_change_rejects_late_events(self):
        self.hook({"hook_event_name": "SessionStart"})
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "old"})
        data = {"session_id": "new", "hook_event_name": "SessionStart"}
        result = subprocess.run([sys.executable, str(self.script), "claude"], input=json.dumps(data), text=True, capture_output=True, env=self.env, timeout=5)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.last_items(), [])
        count = len(self.requests)
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "late"})
        self.assertEqual(len(self.requests), count)

    def test_missing_server_and_invalid_json_never_fail_hook(self):
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "a"}, HERDR_SOCKET_PATH=str(Path(self.tmp.name) / "missing.sock"))
        result = subprocess.run([sys.executable, str(self.script), "claude"], input="not json", text=True, capture_output=True, env=self.env, timeout=5)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout), {})

    def test_outside_herdr_and_cursor_do_not_report(self):
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "a"}, HERDR_ENV="0")
        self.hook({"hook_event_name": "SubagentStart", "agent_id": "a"}, CURSOR_VERSION="test")
        self.assertEqual(self.requests, [])


if __name__ == "__main__":
    unittest.main()
