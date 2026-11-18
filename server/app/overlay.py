"""Opt-in pinned app wiring. Invoke install after audio init, before callbacks."""
import asyncio
import json
import os
import time


def install(app):
    if os.getenv("OH2P_REALTIME") != "1":
        return None
    if any(os.getenv(k) != "1" for k in ("OH2P_AEC_READY", "OH2P_CANCEL_READY")):
        raise RuntimeError("realtime requires verified device AEC and queue cancellation")
    import open_xiaoai_server as server
    from xiaozhi.realtime import RealtimeSession
    from xiaozhi.event import EventManager
    from xiaozhi.services.protocols.typing import DeviceState
    from xiaozhi.ref import get_vad, set_speech_frames
    if not hasattr(server, "playback_control"):
        raise RuntimeError("realtime requires pinned playback_control extension")

    class Playback:
        async def cancel(self): await server.playback_control("stop")
        async def start(self):
            import opuslib_next as opuslib
            app.audio_codec.opus_decoder = opuslib.Decoder(
                fs=app.protocol.server_sample_rate, channels=1)
            await server.playback_control("start")
        async def write(self, opus):
            await server.on_output_data(app.audio_codec.decode_audio(opus))

    state = RealtimeSession(app.protocol, Playback(), enabled=True,
                            aec_ready=True, cancel_ready=True)
    vad = get_vad()
    up = asyncio.Queue(maxsize=8)
    down = asyncio.Queue(maxsize=64)
    workers = []
    controls = asyncio.Lock()
    session_open = False
    wake_generation = 0  # One cloud connection can serve several local wakes.
    wake_task = None
    connection_lock = asyncio.Lock()

    async def play_cue(kind):
        if kind == "ready" and not state._current():
            return
        # Reuse the Saturday device-local qplayer path; no TTS or PCM upload.
        asset = {"ready": "multirounds_tone.opus", "end": "voip_off.opus"}[kind]
        command = "ubus -t 1 call qplayer play '" + json.dumps(
            {"play": "/usr/share/common_sound/" + asset}, separators=(",", ":")
        ) + "'"
        try:
            result = json.loads(await asyncio.wait_for(server.run_shell(command, 1200), 1.5))
            if result.get("exit_code") != 0:
                raise RuntimeError("cue command failed")
            print("OH2P cue requested: " + kind, flush=True)
        except Exception:
            # Optional feedback must not break capture or prevent reawakening.
            print("OH2P cue unavailable: " + kind, flush=True)
    # Silero upstream utils_vad.py defaults; do not inherit legacy 0.01.
    original_threshold = vad.threshold
    vad.threshold = 0.5
    speech_samples = 0
    speech_epoch = 0
    barge_pending = False

    def clear(queue):
        while not queue.empty(): queue.get_nowait()

    async def quiesce():
        state.active = False
        for task in workers: task.cancel()
        await asyncio.gather(*workers, return_exceptions=True)
        workers.clear()
        clear(up)
        clear(down)

    async def stop(channel_open=True, expected_session=None, notify=True, expected_generation=None):
        nonlocal session_open
        if (expected_generation is None and expected_session is None and wake_task
                and wake_task is not asyncio.current_task() and not wake_task.done()):
            wake_task.cancel()
            await asyncio.gather(wake_task, return_exceptions=True)
        async with controls:
            if expected_generation is not None and (
                expected_generation != wake_generation or not session_open
            ):
                return
            if expected_session is not None and (
                state.session_id != expected_session or app.protocol.is_audio_channel_opened()
            ):
                return
            ended = session_open
            session_open = False
            await quiesce()
            try:
                await state.interrupt(channel_open=channel_open)
            finally:
                app.set_device_state(DeviceState.IDLE)
                app.audio_codec.temp_frames = b""
                set_speech_frames([])
                if ended and notify:
                    await play_cue("end")

    async def sender(generation):
        try:
            while True: await state.audio_input(await up.get())
        except asyncio.CancelledError: raise
        except Exception: request_stop(generation)

    async def receiver(generation):
        try:
            while True:
                fn, args = await down.get()
                await fn(*args)
        except asyncio.CancelledError: raise
        except Exception: request_stop(generation)

    async def wake():
        nonlocal session_open, wake_generation, speech_samples, barge_pending
        started = time.monotonic()
        async with controls:
            wake_generation += 1
            speech_samples = 0
            barge_pending = False
            await quiesce()
            try:
                app.audio_codec.temp_frames = b""
                set_speech_frames([])
                if await state.wakeup():
                    session_open = True
                    app.set_device_state(DeviceState.LISTENING)
                    vad.resume("realtime_activity")
                    workers.extend((app.loop.create_task(sender(wake_generation)),
                                    app.loop.create_task(receiver(wake_generation)),
                                    # Cancel unsent feedback along with this wake's audio.
                                    app.loop.create_task(play_cue("ready"))))
                    print(f"OH2P wake ready total_ms={(time.monotonic()-started)*1000:.1f}", flush=True)
            except Exception:
                state.active = False
                app.set_device_state(DeviceState.IDLE)
                raise

    async def barge(epoch, generation):
        nonlocal barge_pending
        async with controls:
            if generation != wake_generation:
                return
            try:
                if not state._current() or epoch != state.response_epoch:
                    return
                # Leave uplink alive. Cancel only the downlink worker so a
                # stalled send to the speaker cannot hold the control lock.
                if len(workers) > 1:
                    workers[1].cancel()
                    await asyncio.gather(workers[1], return_exceptions=True)
                clear(down)
                await asyncio.wait_for(state.barge_in(epoch), timeout=4)
                if state.active and len(workers) > 1:
                    workers[1] = app.loop.create_task(receiver(wake_generation))
            except Exception:
                await quiesce()
                app.set_device_state(DeviceState.IDLE)
                raise
            finally:
                barge_pending = False

    def report_failure(task):
        if not task.cancelled() and task.exception() is not None:
            # The failing operation cleans up its own generation under controls.
            print("OH2P realtime control failed", flush=True)

    # Controls bypass stalled PCM sends and first cancel all data workers.
    def request_stop(generation=None):
        def dispatch():
            if generation is not None and (generation != wake_generation or not state.active):
                return
            state.active = False
            task = app.loop.create_task(stop(expected_generation=generation))
            task.add_done_callback(report_failure)
        app.loop.call_soon_threadsafe(dispatch)

    def request_wake():
        def dispatch():
            nonlocal wake_task
            if (wake_task is not None and not wake_task.done()) or state._current():
                print("OH2P wake duplicate ignored", flush=True)
                return
            wake_task = app.loop.create_task(wake())
            wake_task.add_done_callback(report_failure)
        app.loop.call_soon_threadsafe(dispatch)

    def enqueue(queue, item, generation):
        def put():
            if generation != wake_generation or not state._current(): return
            try: queue.put_nowait(item)
            except asyncio.QueueFull: request_stop(generation)
        app.loop.call_soon_threadsafe(put)

    # Reuse Silero and its shared GlobalStream reader. Count S16 mono samples,
    # not bytes: 8*512 samples at 16kHz = 256ms confirmed speech. No PCM copy.
    old_speech = vad._handle_speech_frame
    old_silence = vad._handle_silence_frame
    def activity(byte_count, generation):
        nonlocal speech_samples, speech_epoch, barge_pending
        if generation != wake_generation:
            return
        state.speech_activity()
        if not state._current():
            speech_samples = 0
            return
        if speech_epoch != state.response_epoch:
            speech_epoch = state.response_epoch
            speech_samples = 0
        speech_samples += byte_count // 2
        if (speech_samples >= vad.sample_rate * 250 / 1000 and
            state.response_available and not state.response_aborted and not barge_pending):
            barge_pending = True
            task = app.loop.create_task(barge(state.response_epoch, generation))
            task.add_done_callback(report_failure)
    def reset_speech(generation):
        nonlocal speech_samples
        if generation == wake_generation:
            speech_samples = 0
    def speech(frames):
        if vad.target == "realtime_activity":
            app.loop.call_soon_threadsafe(activity, len(frames), wake_generation)
        else: old_speech(frames)
    def silence(frames):
        if vad.target == "realtime_activity":
            app.loop.call_soon_threadsafe(reset_speech, wake_generation)
        else: old_silence(frames)
    vad._handle_speech_frame = speech
    vad._handle_silence_frame = silence

    old_hello = app.protocol._handle_server_hello
    async def hello(data):
        if data.get("transport") == "websocket":
            app.protocol.session_id = data.get("session_id", "")
        await old_hello(data)
    app.protocol._handle_server_hello = hello
    old_connect = app.protocol.connect
    async def connect():
        async with connection_lock:
            if app.protocol.is_audio_channel_opened() and app.protocol.session_id:
                return True
            app.protocol.session_id = ""
            started = time.monotonic()
            try:
                return await old_connect()
            finally:
                print(f"OH2P connect total_ms={(time.monotonic()-started)*1000:.1f} ready={app.protocol.is_audio_channel_opened()}", flush=True)
    app.protocol.connect = connect

    async def heartbeat():
        # Repair a closed idle connection before the next wake. No listening or
        # microphone upload here; serialize with foreground/startup connection.
        while True:
            delay = 1
            try:
                if not state.active and not (wake_task and not wake_task.done()):
                    if not app.protocol.is_audio_channel_opened():
                        await app.protocol.connect()
                        if not app.protocol.is_audio_channel_opened():
                            delay = 30
                    else:
                        await app.protocol.send_text(json.dumps({"session_id":"", "type":"ping"}))
            except asyncio.CancelledError:
                raise
            except Exception:
                print("OH2P idle connection check failed", flush=True)
                delay = 30
            await asyncio.sleep(delay)
    app.protocol.heartbeat = heartbeat

    old_tts = app._handle_tts_message
    def tts(data):
        if data.get("state") in ("start", "stop"):
            enqueue(down, (state.incoming_json, (dict(data),)), wake_generation)
        elif state._current() and data.get("session_id") == state.session_id:
            old_tts(data)
    app._handle_tts_message = tts
    old_stt = app._handle_stt_message
    def stt(data):
        enqueue(down, (state.incoming_json, (dict(data),)), wake_generation)
        old_stt(data)
    app._handle_stt_message = stt
    app._on_incoming_audio = lambda data: enqueue(down, (state.audio_output, (data,)), wake_generation)
    def input_audio():
        generation = wake_generation
        if state._current():
            data = app.audio_codec.read_audio()
            if data: enqueue(up, data, generation)
    app._handle_input_audio = input_audio
    EventManager.on_wakeup = request_wake
    EventManager.on_interrupt = request_stop
    old_closed = app._on_audio_channel_closed
    async def closed():
        was_active = state.active
        session = state.session_id
        state.active = False
        if was_active:
            task = app.loop.create_task(stop(False, session, expected_generation=wake_generation))
            task.add_done_callback(report_failure)
        await old_closed()
    app._on_audio_channel_closed = closed
    # Upstream does not signal a clean websocket close. Retain its parser and
    # add the missing EOF notification, without reopening from this callback.
    old_handler = app.protocol._message_handler
    async def message_handler():
        socket = app.protocol.websocket
        try:
            await old_handler()
        finally:
            if app.protocol.websocket is socket and app.protocol.connected:
                app.protocol.connected = False
                await closed()
    app.protocol._message_handler = message_handler

    async def tick():
        if state._current() and not state.speaking:
            if state.clock() - state.last_activity >= state.timeout:
                await stop(expected_generation=wake_generation)
    async def timer():
        while True:
            await asyncio.sleep(1)
            await tick()
    timer_task = app.loop.create_task(timer())

    async def cleanup():
        timer_task.cancel()
        await asyncio.gather(timer_task, return_exceptions=True)
        await stop(False, notify=False)
        vad.threshold = original_threshold
    state.cleanup = cleanup
    state.wake = wake
    state.stop = stop
    state.tick = tick
    return state
