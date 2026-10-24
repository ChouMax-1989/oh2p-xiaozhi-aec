"""Small, opt-in session adapter; no credentials, DSP, device or network startup.

All calls run on XiaoZhi.loop. Playback methods MUST implement acknowledged
device-side queue cancellation, and preserve FIFO ordering with audio writes.
See INTEGRATION.md before wiring into the pinned open-xiaoai application.
"""
import asyncio
import time


class RealtimeSession:
    def __init__(self, protocol, playback, *, enabled=False, aec_ready=False,
                 cancel_ready=False, timeout=20, clock=time.monotonic):
        self.protocol = protocol
        self.playback = playback
        self.enabled = enabled and aec_ready and cancel_ready
        self.timeout = timeout
        self.clock = clock
        self.active = False
        self.speaking = False
        self.session_id = None
        self.last_activity = 0
        self.lock = asyncio.Lock()
        self.response_epoch = 0
        self.response_available = False
        self.response_aborted = False

    def _current(self):
        return self.active and self.session_id == self.protocol.session_id

    async def wakeup(self):
        if not self.enabled:
            return False  # caller keeps the original manual/VAD path
        async with self.lock:
            self.active = False
            self.speaking = False
            self.response_available = False
            self.response_aborted = False
            await self.playback.cancel()  # stop queued and physical playback
            if not self.protocol.is_audio_channel_opened():
                await self.protocol.open_audio_channel()
            if not self.protocol.is_audio_channel_opened():
                return False
            if not self.protocol.session_id:
                return False
            await self.protocol.send_abort_speaking("abort")
            # Upstream ListeningMode.ALWAYS_ON maps to wire mode realtime.
            await self.protocol.send_start_listening("always_on")
            self.session_id = self.protocol.session_id
            self.active = True
            self.last_activity = self.clock()
            return True

    async def audio_input(self, encoded_frames):
        if not self._current():
            return False
        # The bounded uplink worker never owns the control/playback lock.
        await self.protocol.send_audio(encoded_frames)
        return True

    def speech_activity(self):
        if self._current():
            self.last_activity = self.clock()

    async def incoming_json(self, data):
        async with self.lock:
            if not self._current() or data.get("session_id") != self.session_id:
                return False
            if data.get("type") == "stt":
                self.last_activity = self.clock()
            if data.get("type") != "tts":
                return True
            state = data.get("state")
            if state == "start":
                # Repeated starts also discard the previous response locally.
                self.speaking = False
                await self.playback.cancel()
                await self.playback.start()
                self.speaking = True
                self.response_epoch += 1
                self.response_available = True
                self.response_aborted = False
                self.last_activity = self.clock()
            elif state == "stop":
                # Server finished sending; physical playback may still drain.
                # A normal stop is not proof of interruption: do not flush.
                self.speaking = False
                self.last_activity = self.clock()
            # Never restart listen or stop microphone on TTS transitions.
            return True

    async def audio_output(self, data):
        async with self.lock:
            if not self._current() or not self.speaking:
                return False
            await self.playback.write(data)
            return True

    async def interrupt(self, *, channel_open=True):
        async with self.lock:
            self.active = False  # fail closed before any awaited RPC
            self.speaking = False
            self.response_available = False
            await self.playback.cancel()
            if channel_open and self.protocol.is_audio_channel_opened():
                await self.protocol.send_abort_speaking("abort")
                await self.protocol.send_stop_listening()
            self.session_id = None

    async def barge_in(self, expected_epoch):
        async with self.lock:
            if (not self._current() or self.response_aborted or
                not self.response_available or expected_epoch != self.response_epoch):
                return False
            self.response_aborted = True
            self.speaking = False
            try:
                # Stop physical playback before cloud cancellation can block.
                await self.playback.cancel()
                await self.protocol.send_abort_speaking("abort")
            except Exception:
                self.active = False
                raise
            self.last_activity = self.clock()
            return True

    async def expire(self):
        # Call from a periodic task on the same loop, not the PCM callback.
        if self._current() and not self.speaking:
            if self.clock() - self.last_activity >= self.timeout:
                await self.interrupt()
                return True
        return False
