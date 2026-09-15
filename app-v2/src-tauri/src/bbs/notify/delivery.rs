use super::*;
use store::{Claim, Queue};
use tauri::Manager;

pub(super) const WRAPPER: &str = include_str!("../../../prompts/bbs-reply-wrapper.md");

pub(super) struct Envelope {
    pub project: crate::agent_directory::Project,
    pub agent: Option<crate::agent_directory::Agent>,
    pub target_id: String,
    pub event_id: String,
    pub text: String,
}
impl Envelope {
    pub fn request(&self) -> crate::agent_bus::AgentBusSendRequest {
        crate::agent_bus::AgentBusSendRequest {
            project_root: None,
            sender_agent_id: Some("bbs".into()),
            sender_name: Some("BBS".into()),
            target: self.target_id.clone(),
            intent: Some("bbs-thread".into()),
            text: self.text.clone(),
            event_id: Some(self.event_id.clone()),
            dedupe_key: Some(self.event_id.clone()),
            terminal_timing: None,
        }
    }
    pub fn notice(&self) -> crate::violet::ActorMessageRecord {
        crate::violet::ActorMessageRecord {
            actor_id: "bbs".into(),
            actor_name: "BBS".into(),
            text: self.text.clone(),
            target_agent_ids: Vec::new(),
            event_id: self.event_id.clone(),
            actor_intent: Some("bbs-thread".into()),
        }
    }
}
pub(super) trait Sink: Send + Sync {
    fn template(&self) -> String {
        WRAPPER.into()
    }
    fn submit(&self, envelope: Envelope) -> Result<()>;
}
pub(super) struct AppSink(pub tauri::AppHandle);
impl Sink for AppSink {
    fn template(&self) -> String {
        crate::read_system_prompt_template_content("bbs-reply-wrapper.md", WRAPPER)
    }
    fn submit(&self, envelope: Envelope) -> Result<()> {
        let app = &self.0;
        let bus = app.state::<crate::agent_bus::AgentBusManager>();
        if envelope.agent.is_none() {
            bus.record_actor_notice(
                app,
                &envelope.project.root,
                envelope.notice(),
                Some(&envelope.event_id),
            )?;
        } else {
            let manager = app.state::<crate::integrations::IntegrationManager>();
            let launch = crate::resolve_project_agent_launch(
                &manager,
                envelope.project.root.to_str(),
                &envelope.target_id,
            )
            .ok();
            bus.send_request(
                app,
                &app.state::<crate::pty::PtyManager>(),
                &envelope.project.root,
                envelope.request(),
                launch,
            )?;
        }
        // Bus already records skips and persists the stable event ID. Posting
        // is never rolled back and there is no second delivery/retry protocol.
        Ok(())
    }
}

fn wrapped(
    record: &Record,
    project: &crate::agent_directory::Project,
    body: &str,
    warning: bool,
    mut template: String,
) -> String {
    let source = format!(
        "{} (id: {})",
        record.author.project_name, record.author.project_id
    );
    template = template
        .replace("{{thread_id}}", &record.destination.thread_id)
        .replace(
            "{{current_project}}",
            &format!("{} (id: {})", project.name, project.id),
        )
        .replace("{{source_project}}", &source)
        .replace(
            "{{latest_author}}",
            &format!(
                "{} (id: {})",
                record.author.agent_name, record.author.agent_id
            ),
        );
    let human = record.author.local && record.author.agent_id == "human";
    if !human {
        template = template.replace("User instruction:", "BBS author context:");
    }
    let device = if record.author.local {
        format!(
            "this device ({})",
            record
                .author
                .local_device_id
                .as_deref()
                .unwrap_or("local; no device identity")
        )
    } else {
        format!("received via {}; the forwarding device is not an attestation of the original author's device",
            record.author.received_via.as_deref().unwrap_or("an authenticated BBS peer"))
    };
    template.push_str(&format!(
        "\nSource device: {device}\nSource author: {} (id: {}; {})\n",
        record.author.agent_name,
        record.author.agent_id,
        if human {
            "local account user"
        } else {
            "BBS author; not a human instruction"
        }
    ));
    if warning {
        template.push_str(ATTACHMENT_WARNING);
        template.push('\n');
    }
    template.push('\n');
    template.push_str(body);
    template
}

pub(super) fn dispatch(queue: &Queue, claim: Claim, sink: &dyn Sink) -> Result<()> {
    let target = &claim.record.destination.target;
    let Some((project, agent)) = crate::agent_directory::notification_target(
        queue.content.account_dir(),
        &target.project_id,
        &target.agent_id,
    )?
    else {
        return queue.finish(claim, "project_unavailable");
    };
    let body = queue.body_text(&claim)?;
    let mut text = wrapped(
        &claim.record,
        &project,
        &body,
        claim.attachment_warning,
        sink.template(),
    );
    if agent.is_none() {
        text.push_str("\n\nThe addressed agent is no longer available in this project. No agent was awakened.");
    }
    // Recheck after all template/body/directory reads, outside any Bus lock.
    if !queue.still_publishable(&claim)? {
        return queue.finish(claim, "deleted_or_changed");
    }
    sink.submit(Envelope {
        project,
        agent,
        target_id: target.agent_id.clone(),
        event_id: claim.record.event_id(),
        text,
    })?;
    queue.finish(claim, "submitted_or_duplicate")
}
