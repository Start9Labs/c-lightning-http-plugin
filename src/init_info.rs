use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Receiver;
use tokio::sync::RwLock;

#[derive(Clone, Debug)]
pub struct InitInfo {
    pub socket_path: PathBuf,
    pub auth_header: Option<String>,
    pub http_bind: SocketAddr,
}

#[derive(Clone, Debug)]
pub enum InitInfoState {
    Waiting(Receiver<InitInfo>),
    Resolved(Arc<InitInfo>),
}

#[derive(Clone, Debug)]
pub struct InitInfoArc {
    state: Arc<RwLock<InitInfoState>>,
}

impl InitInfoArc {
    pub fn new(r: Receiver<InitInfo>) -> Self {
        InitInfoArc {
            state: Arc::new(RwLock::new(InitInfoState::Waiting(r))),
        }
    }
    pub async fn wait_for_info(self) -> Arc<InitInfo> {
        loop {
            let guard = self.state.read().await;
            match &*guard {
                InitInfoState::Resolved(ref path) => return path.clone(),
                InitInfoState::Waiting(receiver) => match receiver.try_recv() {
                    Ok(ii) => {
                        let arc_ii = Arc::new(ii);
                        drop(guard); // turns out this is important
                        let mut guard = self.state.write().await;
                        *guard = InitInfoState::Resolved(arc_ii.clone());
                        return arc_ii;
                    }
                    Err(_) => (),
                },
            }
        }
    }
}
