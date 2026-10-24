"""Apply narrow Rust changes to a disposable pinned upstream checkout only."""
import argparse
from pathlib import Path

CONTROL = '''
#[pyfunction]
fn playback_control(py: Python, action: String) -> PyResult<Bound<PyAny>> {
    let (command, payload) = match action.as_str() {
        "stop" => ("stop_play", None),
        "start" => ("start_play", Some(json!({
            "pcm": "noop", "channels": 1, "bits_per_sample": 16,
            "sample_rate": 24000, "period_size": 360, "buffer_size": 1440
        }))),
        _ => return Err(pyo3::exceptions::PyValueError::new_err("invalid playback action")),
    };
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let res = RPC::instance().call_remote(command, payload, Some(3000)).await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        if res.code.unwrap_or(0) != 0 {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                res.msg.unwrap_or_else(|| "playback RPC failed".into())));
        }
        Ok(())
    })
}

'''


def replace_once(text, old, new):
    if text.count(old) != 1:
        raise RuntimeError("source mismatch; refuse patch")
    return text.replace(old, new, 1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("checkout", type=Path)
    args = parser.parse_args()
    lib = args.checkout / "examples/xiaozhi/src/lib.rs"
    rpc = args.checkout / "packages/client-rust/src/services/connect/rpc.rs"
    s = lib.read_text(encoding="utf-8")
    s = replace_once(s, "#[pymodule]", CONTROL + "#[pymodule]")
    s = replace_once(s, "    crate::python::init_module(&m)?;",
                    "    m.add_function(wrap_pyfunction!(playback_control, &m)?)?;\n    crate::python::init_module(&m)?;")
    s = replace_once(s, '        let _ = MessageManager::instance()', '        MessageManager::instance()')
    s = replace_once(s, '.send_stream("play", bytes, None)\n            .await;',
                    '.send_stream("play", bytes, None)\n            .await.map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;')
    lib.write_text(s, encoding="utf-8", newline="\n")
    s = rpc.read_text(encoding="utf-8")
    s = replace_once(s, "        send_request(request).await?;\n\n", "")
    anchor = "        let timeout_duration = Duration::from_millis(timeout_millis.unwrap_or(10 * 1000));"
    s = replace_once(s, anchor, '''        // One deadline covers sending and receiving. Store String, not
        // non-Send AppError, across the cleanup await.
        let timeout_duration = Duration::from_millis(timeout_millis.unwrap_or(10 * 1000));
        let deadline = tokio::time::Instant::now() + timeout_duration;
        let send_error = match tokio::time::timeout_at(deadline, send_request(request)).await {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(_) => Some("request timeout".to_string()),
        };
        if let Some(error) = send_error {
            self.pending_requests.lock().await.remove(&uid);
            return Err(error.into());
        }

''')
    s = replace_once(s, "match timeout(timeout_duration, rx).await", "match tokio::time::timeout_at(deadline, rx).await")
    s = s.replace("use tokio::time::{timeout, Duration};", "use tokio::time::Duration;")
    rpc.write_text(s, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
