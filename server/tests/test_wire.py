"""Load production image's protocol implementation without starting its app."""
import importlib.util
import json
import sys
import types
import unittest

sys.path.insert(0, "/app")
from xiaozhi.services.protocols.protocol import Protocol
from xiaozhi.services.protocols.typing import ListeningMode


class Wire(unittest.IsolatedAsyncioTestCase):
    async def test_pinned_protocol_realtime_abort_stop(self):
        p = Protocol()
        p.session_id = "synthetic-hello-session"
        sent = []
        async def capture(text): sent.append(json.loads(text))
        p.send_text = capture
        await p.send_start_listening(ListeningMode.ALWAYS_ON)
        await p.send_abort_speaking("abort")
        await p.send_stop_listening()
        self.assertEqual(sent, [
            {"session_id": p.session_id, "type": "listen", "state": "start", "mode": "realtime"},
            {"session_id": p.session_id, "type": "abort"},
            {"session_id": p.session_id, "type": "listen", "state": "stop"},
        ])


if __name__ == "__main__": unittest.main()
