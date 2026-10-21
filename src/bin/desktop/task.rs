//! A background job on the tokio runtime whose result the UI polls each frame.

use std::future::Future;

use tokio::runtime::Handle;
use tokio::sync::oneshot;

pub struct Task<T> {
    rx: Option<oneshot::Receiver<Result<T, String>>>,
    value: Option<Result<T, String>>,
}

impl<T: Send + 'static> Task<T> {
    /// Runs `fut` on `rt` and repaints the UI when it finishes.
    pub fn spawn<F>(rt: &Handle, ctx: &egui::Context, fut: F) -> Self
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let ctx = ctx.clone();
        rt.spawn(async move {
            let _ = tx.send(fut.await.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        Task { rx: Some(rx), value: None }
    }

    pub fn get(&mut self) -> Option<&mut Result<T, String>> {
        if let Some(rx) = &mut self.rx {
            match rx.try_recv() {
                Ok(v) => self.value = Some(v),
                Err(oneshot::error::TryRecvError::Empty) => return None,
                Err(oneshot::error::TryRecvError::Closed) => self.value = Some(Err("task panicked".into())),
            }
            self.rx = None;
        }
        self.value.as_mut()
    }
}
