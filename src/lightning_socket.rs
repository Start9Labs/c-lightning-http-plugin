use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Receiver;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use tokio::sync::RwLock;

#[derive(Clone, Debug)]
pub enum LightningSocketState {
    Waiting(Receiver<PathBuf>),
    Resolved(Arc<PathBuf>),
}

#[derive(Clone, Debug)]
pub struct LightningSocketArc {
    state: Arc<RwLock<LightningSocketState>>,
}

impl LightningSocketArc {
    pub fn new(r: Receiver<PathBuf>) -> Self {
        LightningSocketArc {
            state: Arc::new(RwLock::new(LightningSocketState::Waiting(r))),
        }
    }
    pub fn wait_for_path(self) -> BoxFuture<'static, Arc<PathBuf>> {
        async move {
            let guard = self.state.read().await;
            match &*guard {
                LightningSocketState::Resolved(ref path) => path.clone(),
                LightningSocketState::Waiting(receiver) => match receiver.try_recv() {
                    Ok(pb) => {
                        let arc_pb = Arc::new(pb);
                        drop(guard);
                        let mut guard = self.state.write().await;
                        *guard = LightningSocketState::Resolved(arc_pb.clone());
                        arc_pb
                    }
                    Err(_) => self.clone().wait_for_path().await,
                },
            }
        }
        .boxed()
    }
}
