// Copyright 2023 Greptime Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use api::v1::meta::{BatchPutRequest, HeartbeatRequest, KeyValue, Role};
use common_telemetry::{trace, warn};
use common_time::util as time_util;
use tokio::sync::mpsc::{self, Sender};

use crate::error::Result;
use crate::handler::{HeartbeatAccumulator, HeartbeatHandler};
use crate::keys::{LeaseKey, LeaseValue};
use crate::metasrv::Context;
use crate::service::store::kv::KvStoreRef;

pub struct KeepLeaseHandler {
    tx: Sender<KeyValue>,
}

impl KeepLeaseHandler {
    pub fn new(kv_store: KvStoreRef) -> Self {
        let (tx, mut rx) = mpsc::channel(1024);
        common_runtime::spawn_bg(async move {
            while let Some(kv) = rx.recv().await {
                let mut kvs = vec![kv];

                while let Ok(kv) = rx.try_recv() {
                    kvs.push(kv);
                }

                let batch_put = BatchPutRequest {
                    kvs,
                    ..Default::default()
                };

                if let Err(err) = kv_store.batch_put(batch_put).await {
                    warn!("Failed to write lease KVs, {err}");
                }
            }
        });

        Self { tx }
    }
}

#[async_trait::async_trait]
impl HeartbeatHandler for KeepLeaseHandler {
    fn is_acceptable(&self, role: Role) -> bool {
        role == Role::Datanode
    }

    async fn handle(
        &self,
        req: &HeartbeatRequest,
        _ctx: &mut Context,
        _acc: &mut HeartbeatAccumulator,
    ) -> Result<()> {
        let HeartbeatRequest { header, peer, .. } = req;
        if let Some(peer) = &peer {
            let key = LeaseKey {
                cluster_id: header.as_ref().map_or(0, |h| h.cluster_id),
                node_id: peer.id,
            };
            let value = LeaseValue {
                timestamp_millis: time_util::current_time_millis(),
                node_addr: peer.addr.clone(),
            };

            trace!("Receive a heartbeat: {key:?}, {value:?}");

            let key = key.try_into()?;
            let value = value.try_into()?;

            if let Err(err) = self.tx.send(KeyValue { key, value }).await {
                warn!("Failed to send lease KV to writer, peer: {peer:?}, {err}");
            }
        }

        Ok(())
    }
}
