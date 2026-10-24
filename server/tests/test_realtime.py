import asyncio
import unittest
from realtime import RealtimeSession


class Protocol:
    session_id = "hello-session"
    opened = True
    def __init__(self): self.events = []
    def is_audio_channel_opened(self): return self.opened
    async def send_abort_speaking(self, reason): self.events.append(("abort", self.session_id))
    async def send_start_listening(self, mode): self.events.append(("listen", mode, self.session_id))
    async def send_stop_listening(self): self.events.append(("stop", self.session_id))
    async def send_audio(self, data): self.events.append(("audio", data))
    async def open_audio_channel(self):
        self.opened = True
        self.session_id = "reconnected-session"


class Playback:
    def __init__(self): self.events = []; self.queue = []
    async def cancel(self): self.queue.clear(); self.events.append("cancel")
    async def start(self): self.events.append("start")
    async def write(self, data): self.queue.append(data)


class Tests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.p = Protocol(); self.out = Playback(); self.now = 0
        self.s = RealtimeSession(self.p, self.out, enabled=True, aec_ready=True,
                                 cancel_ready=True, clock=lambda: self.now)
    def tts(self, state, session="hello-session"):
        return self.s.incoming_json(dict(type="tts", state=state, session_id=session))
    async def test_three_gates(self):
        for opts in ({}, {"enabled":True}, {"enabled":True,"aec_ready":True}):
            s = RealtimeSession(self.p, self.out, **opts)
            self.assertFalse(await s.wakeup())
        self.assertEqual(self.p.events, [])
    async def test_continuous_uplink_and_one_listen(self):
        await self.s.wakeup(); await self.tts("start")
        self.assertTrue(await self.s.audio_input([b"synthetic-opus"]))
        await self.tts("stop")
        self.assertTrue(await self.s.audio_input([b"synthetic-silence"]))
        self.assertEqual(sum(e[0]=="listen" for e in self.p.events),1)
    async def test_normal_stop_drains_and_interrupt_flushes(self):
        await self.s.wakeup(); await self.tts("start")
        await self.s.audio_output(b"synthetic")
        await self.tts("stop")
        self.assertEqual(self.out.queue,[b"synthetic"])
        self.assertFalse(await self.s.audio_output(b"late"))
        await self.s.interrupt()
        self.assertEqual(self.out.queue,[])
    async def test_wrong_session_tts(self):
        await self.s.wakeup()
        self.assertFalse(await self.tts("start","old"))
        self.assertFalse(self.s.speaking)
    async def test_reconnect_invalidates_old_state(self):
        await self.s.wakeup(); self.p.session_id="new"
        self.assertFalse(await self.s.audio_input([b"data"]))
        self.assertFalse(await self.tts("start"))
    async def test_exit_and_rewake(self):
        await self.s.wakeup(); await self.tts("start"); await self.s.interrupt()
        self.assertFalse(await self.s.audio_output(b"late"))
        self.assertFalse(await self.tts("start"))
        self.assertTrue(await self.s.wakeup())
        self.assertEqual(self.p.events[-1],("listen","always_on","hello-session"))
    async def test_idle_timeout_not_audio_arrival(self):
        await self.s.wakeup(); self.now=21
        await self.s.audio_input([b"silence"])
        self.assertTrue(await self.s.expire())
        self.assertFalse(self.s.active)
    async def test_no_timeout_while_speaking(self):
        await self.s.wakeup(); await self.tts("start"); self.now=50
        self.assertFalse(await self.s.expire())
        await self.tts("stop"); self.now=71
        self.assertTrue(await self.s.expire())
    async def test_network_close_no_protocol_send(self):
        await self.s.wakeup(); count=len(self.p.events)
        await self.s.interrupt(channel_open=False)
        self.assertEqual(len(self.p.events),count)
        self.assertFalse(self.s.active)
    async def test_cancel_failure_disables_capture(self):
        await self.s.wakeup()
        async def fail(): raise RuntimeError("RPC failed")
        self.out.cancel=fail
        with self.assertRaises(RuntimeError): await self.s.interrupt()
        self.assertFalse(self.s.active)
    async def test_speech_activity_prevents_long_sentence_timeout(self):
        await self.s.wakeup()
        for second in range(1, 61):
            self.now = second
            self.s.speech_activity()
            self.assertFalse(await self.s.expire())
        self.now = 80
        self.assertTrue(await self.s.expire())
    async def test_wakeup_reconnects_closed_channel(self):
        self.p.opened = False
        self.assertTrue(await self.s.wakeup())
        self.assertEqual(self.s.session_id, "reconnected-session")


if __name__ == "__main__": unittest.main()
