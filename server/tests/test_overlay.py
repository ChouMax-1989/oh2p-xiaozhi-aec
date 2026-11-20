"""NAS integration: real app state methods, VAD class, protocol hello; fake IO."""
import asyncio
import importlib
import json
import os
import sys
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, "/app")
sys.modules["config"] = types.SimpleNamespace(APP_CONFIG={"vad":{}, "wakeup":{"timeout":20}})
import realtime
sys.modules["xiaozhi.realtime"] = realtime
# Avoid loading/starting hardware or downloading model files in network-none test.
sys.modules["xiaozhi.services.audio.vad.silero"] = types.SimpleNamespace(Silero=None)
sys.modules["xiaozhi.services.audio.kws"] = types.SimpleNamespace(KWS=None)
from xiaozhi.services.audio.vad import VAD
from xiaozhi.services.audio.stream import MyStream
from xiaozhi.services.protocols.protocol import Protocol
from xiaozhi.services.protocols.websocket_protocol import WebsocketProtocol
from xiaozhi.services.protocols.typing import DeviceState
from xiaozhi.xiaozhi import XiaoZhi
from xiaozhi.event import EventManager
from xiaozhi.ref import set_xiaozhi
import overlay


class Integration(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.env = patch.dict(os.environ, {"CLI":"1", "OH2P_REALTIME":"1",
            "OH2P_AEC_READY":"1", "OH2P_CANCEL_READY":"1"})
        self.env.start()
        self.saved = (VAD._handle_speech_frame, VAD._handle_silence_frame,
                      EventManager.on_wakeup, EventManager.on_interrupt)
        self.calls = []; self.rendered = []; self.sent = []
        self.cues = []
        self.fail_cue = False
        async def run_shell(command, timeout):
            self.cues.append(command)
            if self.fail_cue: raise RuntimeError("synthetic cue failure")
            return json.dumps({"exit_code":0,"stdout":"{}","stderr":""})
        self.fail_cancel = False
        async def control(action):
            self.calls.append(action)
            if action == "stop" and self.fail_cancel: raise RuntimeError("synthetic RPC failure")
            if action == "stop": self.rendered.clear()
        async def output(data): self.rendered.append(data)
        self.server = patch.dict(sys.modules, {"open_xiaoai_server":
            types.SimpleNamespace(playback_control=control, on_output_data=output, run_shell=run_shell)})
        self.server.start()
        self.app = XiaoZhi.__new__(XiaoZhi)
        self.app.loop = asyncio.get_running_loop()
        self.app.running = True
        self.app.device_state = DeviceState.IDLE
        self.app.on_state_changed_callbacks = []
        self.app.display = types.SimpleNamespace(update_status=lambda _:None,
            update_emotion=lambda _:None)
        stream = lambda: MyStream(16000,1,8,input=True,start=False)
        self.app.audio_codec = types.SimpleNamespace(input_stream=stream(),
            output_stream=MyStream(24000,1,8,output=True,start=False),
            temp_frames=b"", decode_audio=lambda x:x, read_audio=lambda:[b"encoded"])
        self.app.audio_codec.stop_streams = lambda: (
            self.app.audio_codec.input_stream.stop_stream(),
            self.app.audio_codec.output_stream.stop_stream())
        VAD.stream = stream()
        p = WebsocketProtocol.__new__(WebsocketProtocol)
        Protocol.__init__(p)
        p.websocket = object(); p.connected = True; p.session_id = "session-1"
        p.server_sample_rate = 24000
        p.hello_received = asyncio.Event(); p.opens = 0
        async def connect():
            p.opens += 1
            await p._handle_server_hello({"transport":"websocket", "session_id":"session-new"})
            p.connected = True
            return True
        async def open_channel(): await p.connect()
        async def send_text(data): self.sent.append(data)
        async def send_audio(data): self.sent.append(data)
        p.connect = connect; p.open_audio_channel = open_channel
        p.send_text = send_text; p.send_audio = send_audio
        self.app.protocol = p
        set_xiaozhi(self.app)
        self.state = overlay.install(self.app)
        self.now = 0; self.state.clock = lambda:self.now

    async def asyncTearDown(self):
        self.fail_cancel = False
        await self.state.cleanup()
        (VAD._handle_speech_frame, VAD._handle_silence_frame,
         EventManager.on_wakeup, EventManager.on_interrupt) = self.saved
        self.server.stop(); self.env.stop()

    async def settle(self):
        for _ in range(8): await asyncio.sleep(0)

    async def test_cues_once_per_session_not_each_tts(self):
        await self.state.wake()
        await self.settle()
        self.assertEqual(len(self.cues), 1)
        self.assertIn('multirounds_tone.opus', self.cues[0])
        self.tts('start'); await self.settle()
        self.tts('stop'); await self.settle()
        self.assertEqual(len(self.cues), 1)
        await self.state.stop(); await self.state.stop()
        await self.settle()
        self.assertEqual(len(self.cues), 2)
        self.assertIn('voip_off.opus', self.cues[1])
        await self.state.wake()
        await self.settle()
        self.assertEqual(len(self.cues), 3)
        await self.state.cleanup()
        self.assertEqual(len(self.cues), 3)

    async def test_idle_timeout_end_cue_and_failure_does_not_break_wake(self):
        self.fail_cue = True
        await self.state.wake()
        await self.settle()
        self.assertTrue(self.state.active)
        self.fail_cue = False
        self.now = 20
        await self.state.tick()
        await self.settle()
        self.assertFalse(self.state.active)
        self.assertEqual(len(self.cues), 2)
        await self.state.tick()
        self.assertEqual(len(self.cues), 2)

    async def test_slow_ready_cue_allows_first_audio_and_is_cancelled_on_stop(self):
        entered, cancelled = asyncio.Event(), asyncio.Event()
        original = sys.modules["open_xiaoai_server"].run_shell

        async def slow_ready(command, timeout):
            if "multirounds_tone" in command:
                entered.set()
                try:
                    await asyncio.Future()
                finally:
                    cancelled.set()
            return await original(command, timeout)

        with patch.object(sys.modules["open_xiaoai_server"], "run_shell", slow_ready):
            EventManager.on_wakeup()
            await asyncio.wait_for(entered.wait(), 0.5)
            self.assertTrue(self.app.audio_codec.input_stream.is_active())
            self.assertEqual(VAD.target, "realtime_activity")
            self.app._handle_input_audio()
            await self.settle()
            self.assertIn([b"encoded"], self.sent)
            await asyncio.wait_for(self.state.stop(), 0.5)
            await self.settle()
            self.assertTrue(cancelled.is_set())
            self.assertEqual(len(self.cues), 1)
            self.assertIn("voip_off", self.cues[0])

    async def test_slow_end_cue_does_not_block_next_wake(self):
        entered, cancelled = asyncio.Event(), asyncio.Event()
        original = sys.modules["open_xiaoai_server"].run_shell

        async def slow_end(command, timeout):
            if "voip_off" in command:
                entered.set()
                try:
                    await asyncio.Future()
                finally:
                    cancelled.set()
            return await original(command, timeout)

        with patch.object(sys.modules["open_xiaoai_server"], "run_shell", slow_end):
            await self.state.wake()
            await self.settle()
            stop = asyncio.create_task(self.state.stop())
            try:
                await asyncio.wait_for(entered.wait(), 0.5)
                await asyncio.wait_for(asyncio.shield(stop), 0.2)
                EventManager.on_wakeup()
                await asyncio.wait_for(cancelled.wait(), 0.5)
                await self.settle()
                self.assertTrue(self.app.audio_codec.input_stream.is_active())
                self.app._handle_input_audio()
                await self.settle()
                self.assertIn([b"encoded"], self.sent)
            finally:
                stop.cancel()
                await asyncio.gather(stop, return_exceptions=True)

    async def test_stale_input_callbacks_cannot_fill_next_wakes_queue(self):
        await self.state.wake()
        callbacks = []
        with patch.object(self.app.loop, "call_soon_threadsafe", side_effect=callbacks.append):
            for _ in range(12):  # The uplink holds eight packets.
                self.app._handle_input_audio()
        await self.state.stop()
        await self.state.wake()
        for callback in callbacks:
            callback()
        await self.settle()
        self.assertTrue(self.state.active)
        self.assertNotIn([b"encoded"], self.sent)

    async def test_failed_worker_cannot_stop_later_wake_on_same_connection(self):
        for direction in ("upload", "playback"):
            with self.subTest(direction=direction):
                await self.state.wake()
                if direction == "playback":
                    self.tts("start")
                    await self.settle()
                entered, release = asyncio.Event(), asyncio.Event()

                async def fail(_):
                    entered.set()
                    await release.wait()
                    raise OSError("synthetic worker failure")

                target, method = ((self.app.protocol, "send_audio") if direction == "upload"
                                  else (sys.modules["open_xiaoai_server"], "on_output_data"))
                with patch.object(target, method, fail):
                    if direction == "upload":
                        self.app._handle_input_audio()
                    else:
                        self.app._on_incoming_audio(b"old")
                    await asyncio.wait_for(entered.wait(), 0.5)
                    delayed = []
                    with patch.object(self.app.loop, "call_soon_threadsafe", side_effect=delayed.append):
                        release.set()
                        await self.settle()
                    self.assertEqual(len(delayed), 1)
                previous = self.state.session_id
                if direction == "playback":
                    # Stop was dispatched, but its coroutine has not taken the lock.
                    delayed[0]()
                    await self.state.wake()
                else:
                    await self.state.stop()
                    EventManager.on_wakeup()
                    await self.settle()
                    delayed[0]()
                await self.settle()
                self.assertEqual(self.state.session_id, previous)
                self.assertTrue(self.state.active)
                self.sent.clear()
                self.app._handle_input_audio()
                self.tts("start")
                self.app._on_incoming_audio(b"new")
                await self.settle()
                self.assertIn([b"encoded"], self.sent)
                self.assertEqual(self.rendered, [b"new"])
                await self.state.stop()

    async def test_full_queue_stops_current_wake(self):
        await self.state.wake()
        for _ in range(12):
            self.app._handle_input_audio()
        await self.settle()
        self.assertFalse(self.state.active)
        await self.settle()
        self.assertEqual(self.app.device_state, DeviceState.IDLE)

    async def test_delayed_barge_cannot_discard_next_wakes_first_reply(self):
        await self.state.wake()
        self.tts("start")
        await self.settle()
        entered, release = asyncio.Event(), asyncio.Event()
        create_task = self.app.loop.create_task

        def hold_task(coro):
            async def delayed():
                entered.set()
                await release.wait()
                await coro
            return create_task(delayed())

        with patch.object(self.app.loop, "create_task", side_effect=hold_task):
            for _ in range(8):
                VAD._handle_speech_frame(bytes(1024))
            await self.settle()
        self.assertTrue(entered.is_set())
        await self.state.stop()
        await self.state.wake()
        self.tts("start")
        self.app._on_incoming_audio(b"next reply")
        release.set()
        await self.settle()
        self.assertEqual(self.rendered, [b"next reply"])

    async def test_failed_wakeup_has_no_ready_cue(self):
        self.fail_cancel = True
        with self.assertRaises(RuntimeError): await self.state.wake()
        self.assertEqual(self.cues, [])

    def tts(self, state):
        self.app._handle_tts_message(dict(type="tts", state=state,
                                        session_id=self.state.session_id))

    async def test_real_state_keeps_input_through_tts(self):
        await self.state.wake(); self.tts("start"); await self.settle()
        self.assertTrue(self.app.audio_codec.input_stream.is_active())
        self.assertEqual(VAD.target,"realtime_activity")
        self.app._handle_input_audio(); await self.settle()
        self.assertIn([b"encoded"], self.sent)
        self.app._on_incoming_audio(b"synthetic"); await self.settle()
        self.tts("stop"); await self.settle()
        self.assertEqual(self.rendered,[b"synthetic"])
        await self.state.stop(); self.assertEqual(self.rendered,[])

    async def test_vad_activity_60s_and_real_silence_20s(self):
        await self.state.wake()
        for second in range(1,61):
            self.now=second
            VAD._handle_speech_frame(b"synthetic")
            await self.settle(); await self.state.tick()
            self.assertTrue(self.state.active)
        self.now=79; VAD._handle_silence_frame(b"synthetic")
        await self.state.tick(); self.assertTrue(self.state.active)
        self.now=80; await self.state.tick()
        self.assertFalse(self.state.active)

    async def test_disconnect_and_rewake_stores_new_hello_session(self):
        await self.state.wake()
        self.app.protocol.connected=False
        await self.app._on_audio_channel_closed(); await self.settle()
        await self.state.wake()
        self.assertEqual(self.app.protocol.opens,1)
        self.assertEqual(self.state.session_id,"session-new")
        self.assertTrue(self.state.active)

    async def test_stalled_uplink_does_not_block_stop_rpc(self):
        await self.state.wake()
        entered=asyncio.Event()
        async def stalled(_): entered.set(); await asyncio.Future()
        self.app.protocol.send_audio=stalled
        self.app._handle_input_audio(); await entered.wait()
        await asyncio.wait_for(self.state.stop(),0.2)
        self.assertEqual(self.calls[-1],"stop")
        self.assertFalse(self.state.active)

    async def test_clean_websocket_eof_exits(self):
        class ClosedSocket:
            def __aiter__(self): return self
            async def __anext__(self): raise StopAsyncIteration
        await self.state.wake()
        self.app.protocol.websocket=ClosedSocket()
        await self.app.protocol._message_handler()
        await self.settle()
        self.assertFalse(self.state.active)
        self.assertFalse(self.app.protocol.connected)

    async def test_close_then_immediate_rewake_not_cancelled_by_old_cleanup(self):
        await self.state.wake()
        self.app.protocol.connected=False
        await self.app._on_audio_channel_closed()
        await self.state.wake()
        await self.settle()
        self.assertTrue(self.state.active)
        self.assertEqual(self.state.session_id,"session-new")

    async def test_short_noise_does_not_barge_and_silence_resets_count(self):
        await self.state.wake(); self.tts("start"); await self.settle()
        before=len(self.calls)
        for _ in range(7): VAD._handle_speech_frame(bytes(1024))
        await self.settle()
        self.assertEqual(len(self.calls),before)  # 224ms, not 448ms
        VAD._handle_silence_frame(bytes(1024))
        VAD._handle_speech_frame(bytes(1024)); await self.settle()
        self.assertEqual(len(self.calls),before)

    async def test_confirmed_speech_aborts_once_keeps_uplink(self):
        await self.state.wake(); self.tts("start"); await self.settle()
        before=sum(json.loads(x).get("type")=="abort" for x in self.sent if isinstance(x,str))
        for _ in range(8): VAD._handle_speech_frame(bytes(1024))
        await self.settle(); await self.settle()
        for _ in range(20): VAD._handle_speech_frame(bytes(1024))
        await self.settle()
        after=sum(json.loads(x).get("type")=="abort" for x in self.sent if isinstance(x,str))
        self.assertEqual(after-before,1)
        self.assertTrue(self.state.active)
        self.assertFalse(self.state.speaking)
        self.app._handle_input_audio(); await self.settle()
        self.assertIn([b"encoded"],self.sent)
        self.tts("start"); await self.settle()
        self.assertFalse(self.state.response_aborted)

    async def test_normal_followup_after_playback_tail_does_not_abort(self):
        await self.state.wake()
        self.tts("start")
        self.app._on_incoming_audio(bytes(2880))  # 60ms of synthetic S16 mono.
        self.tts("stop")
        await self.settle()
        before = list(self.sent)
        self.now = 10
        for _ in range(8):
            VAD._handle_speech_frame(bytes(1024))
        await self.settle()
        self.assertEqual(self.sent, before)
        self.assertFalse(self.state.response_aborted)
        self.assertTrue(self.state.active)

    async def test_barge_in_queue_tail_stops_device_without_cloud_abort(self):
        await self.state.wake()
        self.tts("start")
        for _ in range(3):
            self.app._on_incoming_audio(bytes(2880))
        self.tts("stop")
        await self.settle()
        self.assertEqual(len(self.rendered), 3)
        before = list(self.sent)
        self.now = 0.26  # Beyond a single frame plus margin, within accumulated tail.
        for _ in range(8):
            VAD._handle_speech_frame(bytes(1024))
        await self.settle(); await self.settle()
        self.assertEqual(self.rendered, [])
        self.assertEqual(self.sent, before)
        self.assertTrue(self.state.active)

    async def test_barge_keeps_next_response_already_in_queue(self):
        entered = asyncio.Event()
        original = sys.modules["open_xiaoai_server"].on_output_data

        async def stalled_old(data):
            if data == b"blocked-old":
                entered.set()
                await asyncio.Future()
            await original(data)

        with patch.object(sys.modules["open_xiaoai_server"], "on_output_data", stalled_old):
            await self.state.wake()
            self.tts("start")
            self.app._on_incoming_audio(b"blocked-old")
            await asyncio.wait_for(entered.wait(), 0.5)
            before = list(self.sent)
            self.app._on_incoming_audio(b"queued-old")
            self.tts("stop")
            self.tts("start")
            self.app._on_incoming_audio(b"new")
            self.tts("stop")
            for _ in range(8):
                VAD._handle_speech_frame(bytes(1024))
            await self.settle(); await self.settle()
            self.assertEqual(self.rendered, [b"new"])
            self.assertEqual(self.sent, before)
            self.assertTrue(self.state.active)

    async def test_barge_cannot_cancel_next_response_during_device_start(self):
        entered, release = asyncio.Event(), asyncio.Event()
        original = sys.modules["open_xiaoai_server"].playback_control
        starts = 0

        async def delayed_start(action):
            nonlocal starts
            if action == "start":
                starts += 1
                if starts == 2:
                    entered.set()
                    await release.wait()
            await original(action)

        with patch.object(sys.modules["open_xiaoai_server"], "playback_control", delayed_start):
            await self.state.wake()
            self.tts("start")
            self.app._on_incoming_audio(bytes(2880))
            self.tts("stop")
            self.tts("start")
            self.app._on_incoming_audio(b"new")
            await asyncio.wait_for(entered.wait(), 0.5)
            before = list(self.sent)
            for _ in range(8):
                VAD._handle_speech_frame(bytes(1024))
            await self.settle()
            release.set()
            await self.settle()
            self.assertEqual(self.rendered, [b"new"])
            self.assertEqual(self.sent, before)
            self.assertTrue(self.state.active)

    async def test_barge_cancel_failure_fails_closed(self):
        await self.state.wake(); self.tts("start"); await self.settle()
        self.fail_cancel=True
        for _ in range(8): VAD._handle_speech_frame(bytes(1024))
        await self.settle(); await self.settle(); await self.settle()
        self.assertFalse(self.state.active)

    async def test_new_tts_resets_decoder(self):
        await self.state.wake(); self.tts("start"); await self.settle()
        first=self.app.audio_codec.opus_decoder
        self.tts("stop"); self.tts("start"); await self.settle()
        self.assertIsNot(first,self.app.audio_codec.opus_decoder)


if __name__ == "__main__": unittest.main()
