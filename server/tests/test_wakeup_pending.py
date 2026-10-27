"""Regression tests for wakeup dispatch while a connection is still opening."""
import asyncio
import json
import unittest

import test_overlay
from xiaozhi.event import EventManager


class PendingWakeupTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        # Keep Integration's hardware-free fixture without inheriting its tests.
        self.fixture = test_overlay.Integration(
            "test_failed_wakeup_has_no_ready_cue")
        await self.fixture.asyncSetUp()
        self.state = self.fixture.state
        self.app = self.fixture.app
        self.gates = []

    async def asyncTearDown(self):
        for gate in self.gates:
            gate.set()
        await self.fixture.asyncTearDown()

    async def until(self, predicate):
        async def poll():
            while not predicate():
                await asyncio.sleep(0)
        await asyncio.wait_for(poll(), 0.5)

    def block_connect(self):
        protocol = self.app.protocol
        protocol.connected = False
        gate = asyncio.Event()
        entered = asyncio.Event()
        self.gates.append(gate)
        original_connect = protocol.connect
        attempts = []

        async def connect():
            attempts.append(True)
            entered.set()
            await gate.wait()
            return await original_connect()

        protocol.connect = connect
        return gate, entered, attempts

    def listen_starts(self):
        return sum(
            isinstance(item, str) and json.loads(item).get("type") == "listen"
            and json.loads(item).get("state") == "start"
            for item in self.fixture.sent
        )

    async def test_duplicate_events_during_pending_open_start_once(self):
        gate, entered, attempts = self.block_connect()
        EventManager.on_wakeup()
        await asyncio.wait_for(entered.wait(), 0.5)
        for _ in range(5):
            EventManager.on_wakeup()
        await self.fixture.settle()
        self.assertEqual(len(attempts), 1)
        self.assertEqual(self.fixture.calls, ["stop"])
        self.assertEqual(self.fixture.cues, [])

        gate.set()
        await self.until(lambda: self.state.active)
        await self.fixture.settle()
        self.assertEqual(len(attempts), 1)
        self.assertEqual(self.listen_starts(), 1)
        self.assertEqual(len(self.fixture.cues), 1)

    async def test_failed_open_can_retry_on_next_event(self):
        protocol = self.app.protocol
        protocol.connected = False
        original_connect = protocol.connect
        attempts = []

        async def connect():
            attempts.append(True)
            if len(attempts) == 1:
                raise RuntimeError("synthetic open failure")
            return await original_connect()

        protocol.connect = connect
        EventManager.on_wakeup()
        await self.until(lambda: len(attempts) == 1)
        await self.fixture.settle()
        self.assertFalse(self.state.active)
        self.assertEqual(self.fixture.cues, [])

        EventManager.on_wakeup()
        await self.until(lambda: self.state.active)
        await self.fixture.settle()
        self.assertEqual(len(attempts), 2)
        self.assertEqual(len(self.fixture.cues), 1)

    async def test_stop_cancels_pending_open_without_late_revival(self):
        gate, entered, attempts = self.block_connect()
        EventManager.on_wakeup()
        await asyncio.wait_for(entered.wait(), 0.5)
        await asyncio.wait_for(self.state.stop(), 0.5)
        gate.set()
        await self.fixture.settle()
        self.assertFalse(self.state.active)
        self.assertEqual(len(attempts), 1)
        self.assertEqual(self.fixture.cues, [])
        self.assertEqual(self.listen_starts(), 0)

    async def test_active_duplicate_does_not_restart_or_cancel_session(self):
        EventManager.on_wakeup()
        await self.until(lambda: self.state.active)
        await self.fixture.settle()
        session = self.state.session_id
        calls = list(self.fixture.calls)
        cues = list(self.fixture.cues)
        for _ in range(4):
            EventManager.on_wakeup()
        await self.fixture.settle()
        self.assertTrue(self.state.active)
        self.assertEqual(self.state.session_id, session)
        self.assertEqual(self.fixture.calls, calls)
        self.assertEqual(self.fixture.cues, cues)
        self.assertEqual(self.listen_starts(), 1)

    async def test_stop_then_next_event_wakes_normally(self):
        EventManager.on_wakeup()
        await self.until(lambda: self.state.active)
        await self.fixture.settle()
        await self.state.stop()
        self.assertFalse(self.state.active)
        self.assertEqual(len(self.fixture.cues), 2)

        EventManager.on_wakeup()
        await self.until(lambda: self.state.active)
        await self.fixture.settle()
        self.assertEqual(self.listen_starts(), 2)
        self.assertEqual(len(self.fixture.cues), 3)

    async def test_idle_heartbeat_reconnects_without_listening_or_cue(self):
        protocol = self.app.protocol
        protocol.connected = False
        task = self.app.loop.create_task(protocol.heartbeat())
        try:
            await self.until(lambda: protocol.opens == 1)
            await self.fixture.settle()
            self.assertTrue(protocol.connected)
            self.assertFalse(self.state.active)
            self.assertEqual(self.listen_starts(), 0)
            self.assertEqual(self.fixture.cues, [])
        finally:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)

    async def test_active_heartbeat_does_not_reconnect(self):
        EventManager.on_wakeup()
        await self.until(lambda: self.state.active)
        await self.fixture.settle()
        protocol = self.app.protocol
        protocol.connected = False
        opens = protocol.opens
        cues = list(self.fixture.cues)
        task = self.app.loop.create_task(protocol.heartbeat())
        try:
            await self.fixture.settle()
            self.assertEqual(protocol.opens, opens)
            self.assertEqual(self.listen_starts(), 1)
            self.assertEqual(self.fixture.cues, cues)
        finally:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)


if __name__ == "__main__":
    unittest.main()
