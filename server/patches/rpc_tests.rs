// Append to patched packages/client-rust/src/services/connect/rpc.rs.
#[cfg(test)]
mod pending_order_tests {
    use super::*;

    #[tokio::test]
    async fn immediate_response_is_not_lost() {
        let rpc = Arc::new(RPC::new());
        let receiver = rpc.clone();
        rpc.init(move |request| {
            let receiver = receiver.clone();
            async move {
                receiver.on_response(Response { id: request.id, ..Response::success() }).await;
                Ok(())
            }
        }).await;
        let result = rpc.call_remote("stop_play", None, Some(100)).await;
        assert!(result.is_ok());
        assert!(rpc.pending_requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn failed_send_removes_pending() {
        let rpc = RPC::new();
        rpc.init(|_| async { Err("synthetic send failure".into()) }).await;
        assert!(rpc.call_remote("stop_play", None, Some(100)).await.is_err());
        assert!(rpc.pending_requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stalled_send_has_deadline_and_cleans_pending() {
        let rpc = RPC::new();
        rpc.init(|_| async { std::future::pending::<Result<(), AppError>>().await }).await;
        let result = tokio::time::timeout(Duration::from_millis(200),
            rpc.call_remote("stop_play", None, Some(20))).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        assert!(rpc.pending_requests.lock().await.is_empty());
    }
}
