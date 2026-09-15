use super::{
    control_channel::{ControlChannel, Delivery},
    data_channel::{DataPipe, IncomingFile},
    network_policy::{NetworkPolicy, PolicyRuntime, STUN_URLS},
    runtime_host::{require_network_thread, Context},
    signaling, Cancellation, Error, MembershipCheck, PeerIdentity, Resource, Result, SessionRole,
    SignedDescription, MAX_FRAME,
};
use crate::bbs_sync::DeviceIdentity;
use rtc::peer_connection::configuration::setting_engine::SctpMaxMessageSize;
use rtc_ice::{mdns::MulticastDnsMode, network_type::NetworkType};
use serde::Serialize;
use std::{
    path::Path,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, watch, Mutex};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, RTCStatsReportEntry,
        SettingEngine, StatsSelector,
    },
};

#[derive(Clone, Copy)]
pub(crate) enum NetworkMode {
    Direct,
    #[cfg(test)]
    Loopback,
}
impl NetworkMode {
    fn loopback(&self) -> bool {
        match self {
            Self::Direct => false,
            #[cfg(test)]
            Self::Loopback => true,
        }
    }
}
struct Handler {
    gathered: watch::Sender<bool>,
    cancel: Cancellation,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.send_replace(true);
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if matches!(
            state,
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
        ) {
            self.cancel.cancel();
        }
    }
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        // Both channels use fixed negotiated IDs. A peer cannot add streams.
        self.cancel.cancel();
        let _ = channel.close().await;
    }
}
pub(crate) struct PendingConnection {
    pc: Arc<dyn PeerConnection>,
    control: Arc<dyn DataChannel>,
    data: Arc<dyn DataChannel>,
    gathered: watch::Receiver<bool>,
    peer: PeerIdentity,
    role: SessionRole,
    nonce: String,
    deadline: tokio::time::Instant,
    policy: Arc<NetworkPolicy>,
    cancel: Cancellation,
    authorized: MembershipCheck,
    context: Context,
    remote_set: bool,
    established: bool,
}
impl Drop for PendingConnection {
    fn drop(&mut self) {
        if !self.established {
            self.cancel.cancel();
        }
    }
}
impl PendingConnection {
    #[cfg(test)]
    pub(super) fn debug_sent_bytes(&self) -> u64 {
        self.policy
            .sent_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    pub(crate) async fn new(
        context: Context,
        peer: PeerIdentity,
        role: SessionRole,
        nonce: String,
        authorized: MembershipCheck,
        mode: NetworkMode,
    ) -> Result<Self> {
        require_network_thread()?;
        if !(authorized)() || context.shutdown.is_cancelled() {
            return Err(Error::Unauthorized);
        }
        signaling::validate_nonce(&nonce)?;
        let permit = context.limits.connection()?;
        let cancel = Cancellation::default();
        let policy = NetworkPolicy::new(mode.loopback());
        let (gathered, rx) = watch::channel(false);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut settings = SettingEngine::default();
        settings.set_multicast_dns_mode(MulticastDnsMode::Disabled);
        settings.set_network_types(if mode.loopback() {
            vec![NetworkType::Udp4]
        } else {
            vec![NetworkType::Udp4, NetworkType::Udp6]
        });
        settings.set_include_loopback_candidate(mode.loopback());
        settings.set_sctp_max_message_size(SctpMaxMessageSize::Bounded(MAX_FRAME as u32));
        settings.set_ice_timeouts(
            Some(Duration::from_secs(10)),
            Some(Duration::from_secs(20)),
            Some(Duration::from_secs(5)),
        );
        let configuration = RTCConfigurationBuilder::new()
            .with_ice_servers(if mode.loopback() {
                vec![]
            } else {
                STUN_URLS
                    .iter()
                    .map(|url| RTCIceServer {
                        urls: vec![(*url).into()],
                        ..Default::default()
                    })
                    .collect()
            })
            .build();
        let pc = tokio::time::timeout_at(
            deadline,
            PeerConnectionBuilder::new()
                .with_setting_engine(settings)
                .with_configuration(configuration)
                .with_handler(Arc::new(Handler {
                    gathered,
                    cancel: cancel.clone(),
                }))
                .with_runtime(Arc::new(PolicyRuntime(policy.clone())))
                .with_udp_addrs(if mode.loopback() {
                    vec!["127.0.0.1:0".to_string()]
                } else {
                    vec!["0.0.0.0:0".to_string(), "[::]:0".to_string()]
                })
                .with_dedicated_reactor_thread(false)
                .with_sctp_receive_buffer_size(1024 * 1024)
                .with_data_channel_send_buffer_limit(512 * 1024)
                .build(),
        )
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Runtime)?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);
        // Start cleanup as soon as the socket exists, including construction failures.
        let closing_pc = pc.clone();
        let close = cancel.clone();
        let parent = context.shutdown.clone();
        let channels = Arc::new(StdMutex::new(Vec::<Arc<dyn DataChannel>>::new()));
        let closing_channels = channels.clone();
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {_=close.cancelled()=>{},_=parent.cancelled()=>{close.cancel();}}
            let channels = closing_channels
                .lock()
                .map(|mut c| std::mem::take(&mut *c))
                .unwrap_or_default();
            let _ = tokio::time::timeout(Duration::from_millis(750), async {
                for channel in &channels {
                    let _ = channel.close().await;
                }
                tokio::task::yield_now().await;
                for channel in &channels {
                    while let Some(event) = channel.poll().await {
                        if matches!(event, DataChannelEvent::OnClose) {
                            break;
                        }
                    }
                }
            })
            .await;
            let _ = tokio::time::timeout(Duration::from_millis(500), closing_pc.close()).await;
        });
        struct Creating(Option<Cancellation>);
        impl Drop for Creating {
            fn drop(&mut self) {
                if let Some(cancel) = &self.0 {
                    cancel.cancel();
                }
            }
        }
        let mut guard = Creating(Some(cancel.clone()));
        let control = tokio::time::timeout_at(
            deadline,
            pc.create_data_channel(
                "kota-bbs-control-v1",
                Some(RTCDataChannelInit {
                    ordered: true,
                    negotiated: Some(0),
                    ..Default::default()
                }),
            ),
        )
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Runtime)?;
        channels
            .lock()
            .map_err(|_| Error::Closed)?
            .push(control.clone());
        let data = tokio::time::timeout_at(
            deadline,
            pc.create_data_channel(
                "kota-bbs-data-v1",
                Some(RTCDataChannelInit {
                    ordered: true,
                    negotiated: Some(1),
                    ..Default::default()
                }),
            ),
        )
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Runtime)?;
        channels
            .lock()
            .map_err(|_| Error::Closed)?
            .push(data.clone());
        let result = Self {
            pc,
            control,
            data,
            gathered: rx,
            peer,
            role,
            nonce,
            deadline,
            policy,
            cancel,
            authorized,
            context,
            remote_set: false,
            established: false,
        };
        guard.0.take();
        Ok(result)
    }
    fn check(&self) -> Result<()> {
        require_network_thread()?;
        if !(self.authorized)() {
            self.cancel.cancel();
            return Err(Error::Unauthorized);
        }
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
    pub(crate) async fn local_description(
        &mut self,
        identity: &DeviceIdentity,
    ) -> Result<SignedDescription> {
        self.check()?;
        let description = match self.role {
            SessionRole::Offer => self.pc.create_offer(None).await,
            SessionRole::Answer if self.remote_set => self.pc.create_answer(None).await,
            _ => return Err(Error::Protocol),
        }
        .map_err(|_| Error::InvalidSignal)?;
        self.pc
            .set_local_description(description)
            .await
            .map_err(|_| Error::InvalidSignal)?;
        let local = self
            .pc
            .local_description()
            .await
            .ok_or(Error::InvalidSignal)?;
        self.policy.local_description(&local.sdp)?;
        tokio::select! {
            _=self.cancel.cancelled()=>return Err(Error::Cancelled),
            result=tokio::time::timeout_at(self.deadline, async {
                while !*self.gathered.borrow_and_update() {self.gathered.changed().await.map_err(|_| Error::Closed)?;}
                Ok::<_,Error>(())
            }) => result.map_err(|_| Error::Timeout)??,
        }
        self.check()?;
        let local = self
            .pc
            .local_description()
            .await
            .ok_or(Error::InvalidSignal)?;

        SignedDescription::create(
            identity,
            &self.peer,
            self.role,
            &self.nonce,
            &local,
            now_ms()?,
        )
    }
    pub(crate) async fn remote_description(&mut self, signed: &SignedDescription) -> Result<()> {
        self.check()?;
        if self.remote_set {
            return Err(Error::Protocol);
        }
        let role = match self.role {
            SessionRole::Offer => SessionRole::Answer,
            SessionRole::Answer => SessionRole::Offer,
        };
        // Every authorization, SDP and candidate check precedes the native API.
        let description = signed.verify(&self.peer, role, &self.nonce, now_ms()?)?;
        let info = signaling::inspect_sdp(&description.sdp)?;
        self.policy.peers(info.candidates, info.candidate_types)?;
        self.pc
            .set_remote_description(description)
            .await
            .map_err(|_| Error::InvalidSignal)?;
        self.remote_set = true;
        Ok(())
    }
    pub(crate) async fn connect(mut self) -> Result<Connection> {
        self.check()?;
        if !self.remote_set {
            return Err(Error::Protocol);
        }
        tokio::select! {
            _=self.cancel.cancelled()=>return Err(Error::Cancelled),
            result=tokio::time::timeout_at(self.deadline, async {
                tokio::try_join!(wait_open(&self.control),wait_open(&self.data))?;
                Ok::<_,Error>(())
            })=>result.map_err(|_| Error::Timeout)??,
        }
        self.check()?;
        let (control, incoming) = ControlChannel::start(
            self.control.clone(),
            self.context.limits.clone(),
            self.cancel.clone(),
            self.authorized.clone(),
        );
        let data = DataPipe::start(
            self.data.clone(),
            self.context.limits.clone(),
            self.context.io.clone(),
            self.cancel.clone(),
            self.authorized.clone(),
        );
        self.established = true;
        Ok(Connection {
            pc: self.pc.clone(),
            control,
            incoming: Mutex::new(incoming),
            data,
            cancel: self.cancel.clone(),
            authorized: self.authorized.clone(),
            policy: self.policy.clone(),
        })
    }
}
async fn wait_open(dc: &Arc<dyn DataChannel>) -> Result<()> {
    loop {
        match dc.poll().await {
            Some(DataChannelEvent::OnOpen) => return Ok(()),
            Some(
                DataChannelEvent::OnMessage(_)
                | DataChannelEvent::OnClose
                | DataChannelEvent::OnClosing
                | DataChannelEvent::OnError,
            )
            | None => return Err(Error::Protocol),
            _ => {}
        }
    }
}
pub(crate) struct Connection {
    pc: Arc<dyn PeerConnection>,
    control: Arc<ControlChannel>,
    incoming: Mutex<mpsc::Receiver<Delivery>>,
    data: Arc<DataPipe>,
    cancel: Cancellation,
    authorized: MembershipCheck,
    pub(super) policy: Arc<NetworkPolicy>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Connection {
    #[cfg(test)]
    pub(super) async fn debug_queued(&self) -> (usize, usize) {
        (
            self.incoming.lock().await.len(),
            self.data.debug_queued().await,
        )
    }
    #[cfg(test)]
    pub(super) async fn pause_file(&self, rx: tokio::sync::oneshot::Receiver<()>) {
        *self.data.pause_after_ready.lock().await = Some(rx);
    }
    #[cfg(test)]
    pub(crate) async fn pause_after_first_file_write(
        &self,
        started: tokio::sync::oneshot::Sender<u64>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        *self.data.pause_after_write.lock().await = Some((started, release));
    }
    pub(crate) fn cancellation(&self) -> Cancellation {
        self.cancel.clone()
    }
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }
    pub(crate) fn recheck_membership(&self) -> Result<()> {
        if !(self.authorized)() {
            self.cancel.cancel();
            return Err(Error::Unauthorized);
        }
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
    pub(crate) async fn send_control(&self, payload: &[u8]) -> Result<()> {
        require_network_thread()?;
        self.recheck_membership()?;
        self.control.send(payload).await
    }
    pub(crate) async fn next_control(&self) -> Result<Delivery> {
        require_network_thread()?;
        self.recheck_membership()?;
        tokio::select! {
            _=self.cancel.cancelled()=>Err(Error::Cancelled),
            next=async {self.incoming.lock().await.recv().await}=>{self.recheck_membership()?;next.ok_or(Error::Closed)}
        }
    }
    pub(crate) async fn send_file(&self, resource: Resource, source: &Path) -> Result<()> {
        require_network_thread()?;
        self.recheck_membership()?;
        self.data.send_file(resource, source).await
    }
    pub(crate) async fn expect_file(
        &self,
        resource: Resource,
        staging: &Path,
    ) -> Result<IncomingFile> {
        require_network_thread()?;
        self.recheck_membership()?;
        self.data.expect_file(resource, staging).await
    }
    pub(crate) async fn diagnostics(&self) -> Result<Diagnostics> {
        require_network_thread()?;
        let report = self.pc.get_stats(Instant::now(), StatsSelector::None).await;
        let selected = report.iter().find_map(|entry| match entry {
            RTCStatsReportEntry::Transport(t) => Some(t.selected_candidate_pair_id.clone()),
            _ => None,
        });
        let pair = report.iter().find_map(|entry| match entry {
            RTCStatsReportEntry::IceCandidatePair(p) if Some(&p.stats.id) == selected.as_ref() => {
                Some(p)
            }
            _ => None,
        });
        let mut result = Diagnostics {
            local_candidate_type: None,
            remote_candidate_type: None,
            remote_candidate_type_basis: None,
            remote_address: None,
            sent_bytes: self
                .policy
                .sent_bytes
                .load(std::sync::atomic::Ordering::Relaxed),
            denied_datagrams: self
                .policy
                .denied
                .load(std::sync::atomic::Ordering::Relaxed),
        };
        if let Some(pair) = pair {
            for entry in report.iter() {
                match entry {
                    RTCStatsReportEntry::LocalCandidate(c)
                        if c.stats.id
                            == format!("RTCLocalIceCandidate_{}", pair.local_candidate_id) =>
                    {
                        result.local_candidate_type = Some(c.candidate_type.to_string())
                    }
                    RTCStatsReportEntry::RemoteCandidate(c)
                        if c.stats.id
                            == format!("RTCRemoteIceCandidate_{}", pair.remote_candidate_id) =>
                    {
                        result.remote_candidate_type = Some(c.candidate_type.to_string());
                        result.remote_candidate_type_basis = Some("native-stats".into());
                    }
                    _ => {}
                }
            }
        }
        // Pinned rtc uses raw pair IDs / prefixed candidate stats IDs. Full SDP
        // installation omits remote stats rows entirely (trickle registers them).
        // Fall back to the destination actually used by the native DTLS socket
        // and its signed candidate type; ambiguous types remain unavailable.
        if let Some((address, kind, basis)) = self.policy.observed_peer() {
            result.remote_address = Some(address.to_string());
            if result.remote_candidate_type.is_none() {
                result.remote_candidate_type = Some(kind);
                result.remote_candidate_type_basis = Some(basis.into());
            }
        }
        Ok(result)
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diagnostics {
    pub(crate) local_candidate_type: Option<String>,
    pub(crate) remote_candidate_type: Option<String>,
    pub(crate) remote_candidate_type_basis: Option<String>,
    pub(crate) remote_address: Option<String>,
    pub(crate) sent_bytes: u64,
    pub(crate) denied_datagrams: u64,
}
fn now_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::StaleSignature)?
        .as_millis()
        .try_into()
        .map_err(|_| Error::StaleSignature)
}
