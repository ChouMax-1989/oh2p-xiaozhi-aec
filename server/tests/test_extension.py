"""Real compiled Rust extension against loopback synthetic client, no hardware."""
import asyncio
import json
import time
import unittest
import websockets
import open_xiaoai_server as extension


class Extension(unittest.IsolatedAsyncioTestCase):
    async def test_rpc_real_extension(self):
        task=asyncio.ensure_future(extension.start_server())
        await asyncio.sleep(0.2)
        commands=[]
        mode={"stop":"ok"}
        async with websockets.connect("ws://127.0.0.1:4399") as client:
            async def respond():
                async for raw in client:
                    if not isinstance(raw,str): continue
                    request=json.loads(raw).get("Request")
                    if not request: continue
                    commands.append(request)
                    action=mode.get("stop") if request["command"]=="stop_play" else "ok"
                    if action=="ignore": continue
                    code=-1 if action=="reject" else 0
                    await client.send(json.dumps({"Response":{
                        "id":request["id"],"code":code,"msg":"synthetic result",
                        "data":{"exit_code":0,"stdout":"","stderr":""} if request["command"]=="run_shell" else None}}))
            reader=asyncio.create_task(respond())
            await extension.playback_control("stop")
            await extension.playback_control("start")
            self.assertEqual([x["command"] for x in commands[:2]],["stop_play","start_play"])
            self.assertEqual(commands[1]["payload"]["sample_rate"],24000)
            with self.assertRaises(ValueError): extension.playback_control("unsupported")
            mode["stop"]="reject"
            with self.assertRaises(RuntimeError): await extension.playback_control("stop")
            mode["stop"]="ignore"
            start=time.monotonic()
            with self.assertRaises(RuntimeError): await extension.playback_control("stop")
            elapsed=time.monotonic()-start
            self.assertGreaterEqual(elapsed,2.8)
            self.assertLess(elapsed,3.6)
            reader.cancel()
            await asyncio.gather(reader,return_exceptions=True)
        task.cancel()
        await asyncio.gather(task,return_exceptions=True)


if __name__=="__main__": unittest.main()
