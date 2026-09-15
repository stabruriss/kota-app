//! Roster metadata follows content manifests; its pictures remain ordinary
//! priority-three file jobs in the same round and the same account budget.
use super::*;
use roster::{reference::Peer, wire};

impl Link {
    pub(super) fn roster_response(
        &self,
        round: &str,
        after: Option<&str>,
        known: Option<&str>,
    ) -> Result<Answer> {
        self.catalog_for(round)?;
        if let Some(version) = known {
            crate::bbs_sync::valid_hash(version).map_err(|_| Error::Protocol)?;
        }
        let source = self
            .session
            .lock()
            .map_err(|_| Error::Runtime)?
            .roster
            .clone();
        let Some(source) = source else {
            return Ok(Answer::Rejected {
                code: "agent_directory_unavailable".into(),
            });
        };
        if after.is_none() && known == Some(source.version.as_str()) {
            return Ok(Answer::RosterUnchanged {
                version: source.version.clone(),
            });
        }
        if after.is_some() && known != Some(source.version.as_str()) {
            return Err(Error::Protocol);
        }
        let page = source.page(after).map_err(|_| Error::Protocol)?;
        Ok(Answer::Roster {
            version: page.version,
            items: page.items,
            next: page.next,
        })
    }
    pub(super) async fn pull_roster(
        &self,
        round: &str,
        _outcome: &mut Outcome,
    ) -> Result<Option<Peer>> {
        let store = self.engine.store.clone();
        let peer = self.remote.clone();
        let known = self
            .engine
            .context
            .io
            .run_in_round(&self.stop, move |io| {
                Peer::load_on_file_thread(&store.state, &peer, io)
            })
            .await?
            .unwrap_or_else(|_| {
                crate::kota_debug_log(
                    "[bbs-roster] invalid_peer_cache; requesting_complete_roster",
                );
                None
            });
        let mut version = known.as_ref().map(|p| p.roster.version.clone());
        let mut after = None;
        let mut receiver: Option<wire::Receiver> = None;
        loop {
            let response = self
                .rpc(Request::Roster {
                    round: round.into(),
                    after: after.clone(),
                    known_version: version.clone(),
                })
                .await?;
            self.connection.recheck_membership()?;
            match response.answer {
                Answer::RosterUnchanged { version: unchanged } => {
                    self.roster_unavailable.store(false, Ordering::Release);
                    if after.is_some()
                        || known.as_ref().is_none_or(|p| p.roster.version != unchanged)
                    {
                        return Err(Error::Protocol);
                    }
                    let current = known.clone().unwrap();
                    let store = self.engine.store.clone();
                    self.engine
                        .context
                        .io
                        .run_in_round(&self.stop, move |_| current.check(&store.state))
                        .await?
                        .map_err(|_| Error::Unauthorized)?;
                    return Ok(known);
                }
                Answer::Roster {
                    version: received,
                    items,
                    next,
                } => {
                    if receiver.is_some() && version.as_deref() != Some(&received) {
                        return Err(Error::Protocol);
                    }
                    let page = wire::Page {
                        version: received.clone(),
                        items,
                        next: next.clone(),
                    };
                    let store = self.engine.store.clone();
                    let peer = self.remote.clone();
                    let files = self.engine.context.io.clone();
                    let connection = self.connection.clone();
                    let (result, stage) = self
                        .engine
                        .context
                        .io
                        .run_in_round(&self.stop, move |io| {
                            let mut stage = receiver;
                            let result = (|| -> anyhow::Result<bool> {
                                if stage.is_none() {
                                    stage = Some(wire::Receiver::begin(
                                        &store.state,
                                        &peer,
                                        &page.version,
                                        files,
                                    )?);
                                }
                                stage.as_mut().unwrap().append(&store.state, &page, io, || {
                                    connection.recheck_membership().is_ok()
                                })
                            })();
                            (result, stage)
                        })
                        .await?;
                    receiver = stage;
                    let complete = result.map_err(|_| Error::Integrity)?;
                    version = Some(received);
                    after = next;
                    drop(response._delivery);
                    if complete {
                        self.connection.recheck_membership()?;
                        let store = self.engine.store.clone();
                        let peer = self.remote.clone();
                        let cached = self
                            .engine
                            .context
                            .io
                            .run_in_round(&self.stop, move |io| {
                                Peer::load_on_file_thread(&store.state, &peer, io)
                            })
                            .await?
                            .map_err(|_| Error::Integrity)?
                            .ok_or(Error::Unauthorized)?;
                        if cached.roster.version != version.unwrap() {
                            return Err(Error::Protocol);
                        }
                        if let Some(runtime) = &self.engine.roster {
                            runtime.peer_changed(&self.remote);
                        }
                        self.roster_unavailable.store(false, Ordering::Release);
                        return Ok(Some(cached));
                    }
                }
                Answer::Rejected { code }
                    if after.is_none() && code == "agent_directory_unavailable" =>
                {
                    // This is not a failed content job. Preserve the previous
                    // complete directory and report once per connection/error
                    // transition without turning every content round Partial.
                    if !self.roster_unavailable.swap(true, Ordering::AcqRel) {
                        crate::kota_debug_log("[bbs-roster] peer_agent_directory_unavailable");
                    }
                    return Ok(None);
                }
                _ => return Err(Error::Protocol),
            }
        }
    }
    pub(super) async fn roster_avatar_jobs(
        &self,
        peer: Option<&Peer>,
        jobs: &mut Vec<Job>,
        outcome: &mut Outcome,
    ) -> Result<()> {
        let Some(peer) = peer.cloned() else {
            return Ok(());
        };
        let store = self.engine.store.clone();
        let runtime = self.engine.roster.clone();
        let existing = jobs
            .iter()
            .filter_map(|j| {
                matches!(j.resource.identity, ResourceKind::Avatar { .. })
                    .then_some(j.resource.sha256.clone())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let slots = MAX_JOBS.saturating_sub(jobs.len());
        let (extra, more) = self
            .engine
            .context
            .io
            .run_in_round(&self.stop, move |io| -> anyhow::Result<_> {
                peer.check(&store.state)?;
                let mut seen = existing;
                let mut extra = Vec::new();
                for avatar in peer
                    .roster
                    .projects
                    .iter()
                    .flat_map(|p| &p.agents)
                    .map(|a| &a.avatar)
                {
                    io.check()?;
                    let Some(resource) = avatar.resource() else {
                        continue;
                    };
                    if !seen.insert(resource.sha256.clone()) {
                        continue;
                    }
                    let available = match &runtime {
                        Some(runtime) => runtime.image_available(avatar, io),
                        None => store
                            .verify_roster_avatar(&resource, io)
                            .is_ok_and(|p| p.is_some()),
                    };
                    if available {
                        continue;
                    }
                    if extra.len() == slots {
                        return Ok((extra, true));
                    }
                    extra.push(resource);
                }
                Ok((extra, false))
            })
            .await?
            .map_err(|_| Error::Unauthorized)?;
        jobs.extend(extra.into_iter().map(|resource| Job {
            resource,
            offered: None,
        }));
        jobs.sort_by_key(Job::priority);
        outcome.more |= more;
        Ok(())
    }
    pub(super) fn avatar_installed(&self, resource: &Resource) {
        if matches!(resource.identity, ResourceKind::Avatar { .. }) {
            if let Some(runtime) = &self.engine.roster {
                runtime.avatar_changed(&resource.sha256);
            }
        }
    }
}
